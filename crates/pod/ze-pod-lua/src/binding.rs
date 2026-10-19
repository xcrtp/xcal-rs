// binding.rs — **Lua 适配层**：通用驱动（`ze-pod-binding`）+ 只属于 Lua 的 codec。
//
// ```text
// C::calls() ──► BindingPlan（语言无关）
//                      │
//                      ├─ 每个函数 → Lua 闭包 → invoke(&LuaCodec, ...)
//                      └─ 每个类型 → Lua 构造器（组件 / 值 / 枚举）
// ```
//
// 语言无关的部分（计划、类型收集、调用编排、JSON 文档）在 `ze-pod-binding`；
// 本文件只保留 `lua_to_pod` / `pod_to_lua` 与 Lua 闭包创建。

use std::sync::Arc;

use mlua::{AnyUserData, Lua, MultiValue, Table, UserData, Value};
use ze_pod::{Capability, ErasedFn, Field, Pod, PodValue, ScalarKind, ScalarValue, TypeSpec};
use ze_pod_binding::{BindingPlan, ValueCodec, short_name};

/// Lua 侧的不透明句柄令牌。
#[derive(Clone, Copy, Debug)]
pub struct LuaHandle(pub ze_pod::Handle);

impl UserData for LuaHandle {}

/// Lua 侧的结构体 / 枚举值令牌（`Color { ... }` / `Curve.Phase { ... }` 的产物）。
#[derive(Clone, Debug)]
pub struct LuaPodValue(pub PodValue);

impl UserData for LuaPodValue {}

/// 注册一个 capability 的全局表（`scene` / `content` / `python`）与类型构造器。
pub(crate) fn bind<C: Capability<'static>>(
    lua: &Lua,
    global_name: &str,
    service: Arc<dyn Pod>,
) -> mlua::Result<()> {
    let plan = BindingPlan::new(C::calls());
    let table = lua.create_table()?;
    for item in plan.calls {
        register_function(lua, &table, *item, Arc::clone(&service))?;
    }
    lua.globals().set(global_name, table)?;
    register_types(lua, &plan, service)?;
    Ok(())
}

/// Lua 值 ↔ `PodValue`（**本文件唯一的语言特定逻辑**）。
struct LuaCodec<'a> {
    lua: &'a Lua,
}

impl ValueCodec for LuaCodec<'_> {
    type Value = Value;
    type Error = mlua::Error;

    fn to_pod(&self, value: &Value, spec: &'static TypeSpec) -> mlua::Result<PodValue> {
        lua_to_pod(value, spec)
    }

    fn from_pod(&self, value: &PodValue) -> mlua::Result<Value> {
        pod_to_lua(self.lua, value)
    }

    fn error(&self, message: String) -> mlua::Error {
        mlua::Error::RuntimeError(message)
    }
}

fn register_function(
    lua: &Lua,
    table: &Table,
    item: ErasedFn,
    service: Arc<dyn Pod>,
) -> mlua::Result<()> {
    let name = item.signature.name;

    let func = lua.create_function(move |lua, args: MultiValue| {
        let values: Vec<Value> = args.into_iter().collect();
        let codec = LuaCodec { lua };
        ze_pod_binding::invoke(&codec, &*service, item, &values)
    })?;
    table.set(name, func)
}

/// 自动注册所有可达类型的构造器（组件 / 值 / 枚举）。
fn register_types(lua: &Lua, plan: &BindingPlan, service: Arc<dyn Pod>) -> mlua::Result<()> {
    for (&full_name, &spec) in &plan.types {
        let lua_name = short_name(full_name);
        match spec {
            TypeSpec::Struct { name, fields } => {
                let fields = *fields;
                let type_name = *name;
                match plan.component_ctor(type_name) {
                    // 组件：`Circle { ... }` ⇒ 调用宿主构造函数 ⇒ 句柄。
                    Some(ctor) => {
                        let call = ctor.call;
                        let svc = Arc::clone(&service);
                        let func = lua.create_function(move |lua, spec: Table| {
                            let value = table_to_pod(&spec, fields, type_name, None)?;
                            let out = call(&*svc, &[value]).map_err(mlua::Error::external)?;
                            pod_to_lua(lua, &out)
                        })?;
                        lua.globals().set(lua_name, func)?;
                    }
                    // 纯值结构体：`Color { ... }` ⇒ 值令牌。
                    None => {
                        let func = lua.create_function(move |_, spec: Table| {
                            table_to_pod(&spec, fields, type_name, None).map(LuaPodValue)
                        })?;
                        lua.globals().set(lua_name, func)?;
                    }
                }
            }
            TypeSpec::Enum { name, variants } => {
                let table = lua.create_table()?;
                for variant in *variants {
                    let type_name = *name;
                    let variant_name = variant.name;
                    let fields = variant.fields;
                    if fields.is_empty() {
                        table.set(
                            variant_name,
                            LuaPodValue(PodValue::Enum(type_name, variant_name, Vec::new())),
                        )?;
                    } else {
                        let func = lua.create_function(move |_, spec: Table| {
                            table_to_pod(&spec, fields, type_name, Some(variant_name))
                                .map(LuaPodValue)
                        })?;
                        table.set(variant_name, func)?;
                    }
                }
                lua.globals().set(lua_name, table)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn table_to_pod(
    table: &Table,
    fields: &'static [Field],
    type_name: &'static str,
    variant: Option<&'static str>,
) -> mlua::Result<PodValue> {
    let mut values = Vec::with_capacity(fields.len());
    for field in fields {
        let value: Value = table.get(field.name)?;
        if value.is_nil() {
            return Err(mlua::Error::RuntimeError(format!(
                "`{type_name}` 缺少字段 `{}`",
                field.name
            )));
        }
        values.push((field.name, lua_to_pod(&value, field.ty)?));
    }
    Ok(match variant {
        Some(variant) => PodValue::Enum(type_name, variant, values),
        None => PodValue::Struct(type_name, values),
    })
}

fn lua_to_pod(value: &Value, spec: &'static TypeSpec) -> mlua::Result<PodValue> {
    match spec {
        TypeSpec::Scalar(kind) => scalar_from_lua(value, *kind),
        TypeSpec::Opaque(_) => match value {
            Value::UserData(ud) => Ok(PodValue::Opaque(handle_of(ud)?)),
            other => Err(type_error("句柄（userdata）", other)),
        },
        TypeSpec::Array { elem } => match value {
            Value::Table(table) => {
                let mut values = Vec::new();
                for item in table.sequence_values::<Value>() {
                    values.push(lua_to_pod(&item?, elem)?);
                }
                Ok(PodValue::Array(values))
            }
            other => Err(type_error("table（数组）", other)),
        },
        TypeSpec::Struct { name, fields } => {
            if let Some(value) = wrapped_value(value) {
                return match &value {
                    PodValue::Struct(actual, _) if actual == name => Ok(value),
                    other => Err(mlua::Error::RuntimeError(format!(
                        "期望结构体 `{name}`，实际是 {}",
                        ze_pod::describe(other)
                    ))),
                };
            }
            match value {
                Value::Table(table) => table_to_pod(table, fields, name, None),
                other => Err(type_error("table（结构体）", other)),
            }
        }
        TypeSpec::Enum { name, .. } => {
            if let Some(value) = wrapped_value(value) {
                return match &value {
                    PodValue::Enum(actual, _, _) if actual == name => Ok(value),
                    other => Err(mlua::Error::RuntimeError(format!(
                        "期望枚举 `{name}`，实际是 {}",
                        ze_pod::describe(other)
                    ))),
                };
            }
            Err(mlua::Error::RuntimeError(format!(
                "枚举 `{name}` 请用构造器创建（如 `Curve.Phase {{ ... }}`）"
            )))
        }
    }
}

/// 若 Lua 值是 `LuaPodValue` 包装，取出内部值。
fn wrapped_value(value: &Value) -> Option<PodValue> {
    let Value::UserData(ud) = value else {
        return None;
    };
    ud.borrow::<LuaPodValue>()
        .ok()
        .map(|wrapped| wrapped.0.clone())
}

fn handle_of(value: &AnyUserData) -> mlua::Result<ze_pod::Handle> {
    value
        .borrow::<LuaHandle>()
        .map(|handle| handle.0)
        .map_err(|_| mlua::Error::RuntimeError("期望 xcal 场景句柄（userdata）".to_string()))
}

fn scalar_from_lua(value: &Value, kind: ScalarKind) -> mlua::Result<PodValue> {
    let scalar = match kind {
        ScalarKind::F32 => ScalarValue::F32(number(value)? as f32),
        ScalarKind::F64 => ScalarValue::F64(number(value)?),
        ScalarKind::I32 => ScalarValue::I32(integer(value)? as i32),
        ScalarKind::U32 => ScalarValue::U32(integer(value)? as u32),
        ScalarKind::U64 => {
            let n = integer(value)?;
            let n = u64::try_from(n).map_err(|_| {
                mlua::Error::RuntimeError(format!("u64 必须是非负整数，实际 {n}"))
            })?;
            ScalarValue::U64(n)
        }
        ScalarKind::U8 => {
            let n = integer(value)?;
            if !(0..=255).contains(&n) {
                return Err(mlua::Error::RuntimeError(format!(
                    "颜色分量必须在 0..=255，实际 {n}"
                )));
            }
            ScalarValue::U8(n as u8)
        }
        ScalarKind::Bool => match value {
            Value::Boolean(b) => ScalarValue::Bool(*b),
            other => return Err(type_error("boolean", other)),
        },
        ScalarKind::String => match value {
            Value::String(s) => ScalarValue::String(s.to_string_lossy().to_string()),
            other => return Err(type_error("string", other)),
        },
    };
    Ok(PodValue::Scalar(scalar))
}

fn number(value: &Value) -> mlua::Result<f64> {
    match value {
        Value::Number(n) => Ok(*n),
        Value::Integer(i) => Ok(*i as f64),
        other => Err(type_error("number", other)),
    }
}

fn integer(value: &Value) -> mlua::Result<i64> {
    match value {
        Value::Integer(i) => Ok(*i),
        // 精确零语义：判断的是"这个 f64 的数学值是否为整数"，不是浮点计算结果
        // 的近似比较；`fract() == 0.0` 对 Lua 传入的整数值是精确成立的。
        Value::Number(n) if n.fract() == 0.0 => Ok(*n as i64),
        other => Err(type_error("integer", other)),
    }
}

fn pod_to_lua(lua: &Lua, value: &PodValue) -> mlua::Result<Value> {
    match value {
        PodValue::Scalar(scalar) => Ok(match scalar {
            ScalarValue::F32(v) => Value::Number(*v as f64),
            ScalarValue::F64(v) => Value::Number(*v),
            ScalarValue::I32(v) => Value::Integer(*v as i64),
            ScalarValue::U32(v) => Value::Integer(*v as i64),
            ScalarValue::U64(v) => Value::Integer(i64::try_from(*v).map_err(|_| {
                mlua::Error::RuntimeError(format!("u64 超出 Lua 整数范围：{v}"))
            })?),
            ScalarValue::U8(v) => Value::Integer(*v as i64),
            ScalarValue::Bool(v) => Value::Boolean(*v),
            ScalarValue::String(v) => Value::String(lua.create_string(v)?),
        }),
        PodValue::Opaque(handle) => Ok(Value::UserData(lua.create_userdata(LuaHandle(*handle))?)),
        PodValue::Struct("()", _) => Ok(Value::Nil),
        PodValue::Struct(..) | PodValue::Enum(..) => Ok(Value::UserData(
            lua.create_userdata(LuaPodValue(value.clone()))?,
        )),
        PodValue::Array(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0)?;
            for (index, item) in items.iter().enumerate() {
                table.set(index + 1, pod_to_lua(lua, item)?)?;
            }
            Ok(Value::Table(table))
        }
    }
}

fn type_error(expected: &str, actual: &Value) -> mlua::Error {
    mlua::Error::RuntimeError(format!(
        "参数类型不匹配：期望 {expected}，实际 {}",
        actual.type_name()
    ))
}
