// binding.rs — **由 `SceneApi::calls()` + `TypeSpec` 生成 Lua 绑定**（V2/V4）。
//
// 这里没有"每个函数一个手写闭包"：
//
// ```text
// SceneApi::calls()  →  name + FnIo（形参名 / TypeSpec） + 类型擦除 call
//        │
//        ├─ 每个函数 → 一个**通用** Lua 闭包（按 TypeSpec 转换参数/返回值）
//        └─ 每个命名类型 → 自动注册构造器：
//              struct + 「单参数 + 返回句柄」的构造函数 ⇒ `Circle { ... }` 直接得句柄
//              其它 struct                                  ⇒ `Color { ... }` 得值
//              enum                                          ⇒ `Curve.Phase { ... }`
// ```
//
// 新增一个 capability 函数 ⇒ Lua 侧自动可用（无需改本文件）。
// 新增一个类型 ⇒ 构造器自动出现（由 schema 驱动）。

use std::collections::BTreeMap;
use std::sync::Arc;

use mlua::{AnyUserData, Lua, MultiValue, Table, UserData, Value};
use xc_vm::{
    Capability, ErasedFn, Field, Handle, ScalarKind, ScalarValue, TypeSpec, Vm, VmValue,
};

use crate::api::SceneApi;

/// Lua 侧的**不透明句柄**令牌。
#[derive(Clone, Copy, Debug)]
pub struct LuaHandle(pub Handle);

impl UserData for LuaHandle {}

/// Lua 侧的结构体 / 枚举值令牌（`Color { ... }` / `Curve.Phase { ... }` 的产物）。
#[derive(Clone, Debug)]
pub struct LuaVmValue(pub VmValue);

impl UserData for LuaVmValue {}

/// 注册 `scene` 全局表与全部类型构造器。
pub(crate) fn bind(lua: &Lua, service: Arc<dyn Vm>) -> mlua::Result<()> {
    let calls = SceneApi::calls();
    let table = lua.create_table()?;
    for item in calls {
        register_function(lua, &table, *item, Arc::clone(&service))?;
    }
    lua.globals().set("scene", table)?;
    register_types(lua, calls, service)?;
    Ok(())
}

fn register_function(
    lua: &Lua,
    table: &Table,
    item: ErasedFn,
    service: Arc<dyn Vm>,
) -> mlua::Result<()> {
    let name = item.signature.name;
    let io = item.io;
    let call = item.call;

    let func = lua.create_function(move |lua, args: MultiValue| {
        if args.len() != io.params.len() {
            return Err(mlua::Error::RuntimeError(format!(
                "`{name}` 需要 {} 个参数，实际 {}",
                io.params.len(),
                args.len()
            )));
        }
        let mut values = Vec::with_capacity(args.len());
        for (value, param) in args.iter().zip(io.params) {
            values.push(lua_to_vm(value, param.ty)?);
        }
        let out = call(&*service, &values).map_err(mlua::Error::external)?;
        vm_to_lua(lua, &out)
    })?;
    table.set(name, func)
}

/// 自动注册所有**可达类型**的构造器（从 `calls()` 的形参 / 返回类型递归收集）。
fn register_types(
    lua: &Lua,
    calls: &'static [ErasedFn],
    service: Arc<dyn Vm>,
) -> mlua::Result<()> {
    let mut types: BTreeMap<&'static str, &'static TypeSpec> = BTreeMap::new();
    for item in calls {
        for param in item.io.params {
            collect_types(param.ty, &mut types);
        }
        collect_types(item.io.ret, &mut types);
    }

    for (full_name, spec) in types {
        if full_name == "()" {
            continue;
        }
        let lua_name = short_name(full_name);
        match spec {
            TypeSpec::Struct { name, fields } => {
                let fields = *fields;
                let type_name = *name;
                match component_ctor(calls, type_name) {
                    // 组件：`Circle { ... }` ⇒ 调用宿主构造函数 ⇒ 句柄。
                    Some(ctor) => {
                        let call = ctor.call;
                        let svc = Arc::clone(&service);
                        let func = lua.create_function(move |lua, spec: Table| {
                            let value = table_to_vm(&spec, fields, type_name, None)?;
                            let out =
                                call(&*svc, &[value]).map_err(mlua::Error::external)?;
                            vm_to_lua(lua, &out)
                        })?;
                        lua.globals().set(lua_name, func)?;
                    }
                    // 纯值结构体：`Color { ... }` ⇒ 值令牌。
                    None => {
                        let func = lua.create_function(move |_, spec: Table| {
                            table_to_vm(&spec, fields, type_name, None).map(LuaVmValue)
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
                    let func = lua.create_function(move |_, spec: Table| {
                        table_to_vm(&spec, fields, type_name, Some(variant_name))
                            .map(LuaVmValue)
                    })?;
                    table.set(variant_name, func)?;
                }
                lua.globals().set(lua_name, table)?;
            }
            _ => {}
        }
    }
    Ok(())
}

/// 「单参数是该结构体 + 返回不透明句柄」的 capability 视为组件构造器。
pub(crate) fn component_ctor(
    calls: &'static [ErasedFn],
    type_name: &'static str,
) -> Option<ErasedFn> {
    calls.iter().copied().find(|item| {
        item.io.params.len() == 1
            && matches!(
                item.io.params[0].ty,
                TypeSpec::Struct { name, .. } if *name == type_name
            )
            && item.io.ret.is_opaque()
    })
}

/// 递归收集命名类型（struct / enum）。
pub(crate) fn collect_types(
    spec: &'static TypeSpec,
    out: &mut BTreeMap<&'static str, &'static TypeSpec>,
) {
    match spec {
        TypeSpec::Struct { name, fields } => {
            if *name != "()" {
                out.insert(*name, spec);
            }
            for field in *fields {
                collect_types(field.ty, out);
            }
        }
        TypeSpec::Enum { name, variants } => {
            out.insert(*name, spec);
            for variant in *variants {
                for field in variant.fields {
                    collect_types(field.ty, out);
                }
            }
        }
        TypeSpec::Array { elem } => collect_types(elem, out),
        TypeSpec::Scalar(_) | TypeSpec::Opaque(_) => {}
    }
}

/// `"xcal.Color"` → `"Color"`（Lua 侧全局名用短名）。
pub(crate) fn short_name(full: &'static str) -> &'static str {
    full.rsplit(|c| c == '.' || c == ':').next().unwrap_or(full)
}

fn table_to_vm(
    table: &Table,
    fields: &'static [Field],
    type_name: &'static str,
    variant: Option<&'static str>,
) -> mlua::Result<VmValue> {
    let mut values = Vec::with_capacity(fields.len());
    for field in fields {
        let value: Value = table.get(field.name)?;
        if value.is_nil() {
            return Err(mlua::Error::RuntimeError(format!(
                "`{type_name}` 缺少字段 `{}`",
                field.name
            )));
        }
        values.push((field.name, lua_to_vm(&value, field.ty)?));
    }
    Ok(match variant {
        Some(variant) => VmValue::Enum(type_name, variant, values),
        None => VmValue::Struct(type_name, values),
    })
}

fn lua_to_vm(value: &Value, spec: &'static TypeSpec) -> mlua::Result<VmValue> {
    match spec {
        TypeSpec::Scalar(kind) => scalar_from_lua(value, *kind),
        TypeSpec::Opaque(_) => match value {
            Value::UserData(ud) => Ok(VmValue::Opaque(handle_of(ud)?)),
            other => Err(type_error("句柄（userdata）", other)),
        },
        TypeSpec::Array { elem } => match value {
            Value::Table(table) => {
                let mut values = Vec::new();
                for item in table.sequence_values::<Value>() {
                    values.push(lua_to_vm(&item?, elem)?);
                }
                Ok(VmValue::Array(values))
            }
            other => Err(type_error("table（数组）", other)),
        },
        TypeSpec::Struct { name, fields } => {
            if let Some(value) = wrapped_value(value) {
                return match &value {
                    VmValue::Struct(actual, _) if actual == name => Ok(value),
                    other => Err(mlua::Error::RuntimeError(format!(
                        "期望结构体 `{name}`，实际是 {}",
                        xc_vm::describe(other)
                    ))),
                };
            }
            match value {
                Value::Table(table) => table_to_vm(table, fields, name, None),
                other => Err(type_error("table（结构体）", other)),
            }
        }
        TypeSpec::Enum { name, .. } => {
            if let Some(value) = wrapped_value(value) {
                return match &value {
                    VmValue::Enum(actual, _, _) if actual == name => Ok(value),
                    other => Err(mlua::Error::RuntimeError(format!(
                        "期望枚举 `{name}`，实际是 {}",
                        xc_vm::describe(other)
                    ))),
                };
            }
            Err(mlua::Error::RuntimeError(format!(
                "枚举 `{name}` 请用构造器创建（如 `Curve.Phase {{ ... }}`）"
            )))
        }
    }
}

/// 若 Lua 值是 `LuaVmValue` 包装，取出内部值。
fn wrapped_value(value: &Value) -> Option<VmValue> {
    let Value::UserData(ud) = value else {
        return None;
    };
    ud.borrow::<LuaVmValue>()
        .ok()
        .map(|wrapped| wrapped.0.clone())
}

fn handle_of(value: &AnyUserData) -> mlua::Result<Handle> {
    value
        .borrow::<LuaHandle>()
        .map(|handle| handle.0)
        .map_err(|_| mlua::Error::RuntimeError("期望 xcal 场景句柄（userdata）".to_string()))
}

fn scalar_from_lua(value: &Value, kind: ScalarKind) -> mlua::Result<VmValue> {
    let scalar = match kind {
        ScalarKind::F32 => ScalarValue::F32(number(value)? as f32),
        ScalarKind::F64 => ScalarValue::F64(number(value)?),
        ScalarKind::I32 => ScalarValue::I32(integer(value)? as i32),
        ScalarKind::U32 => ScalarValue::U32(integer(value)? as u32),
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
    Ok(VmValue::Scalar(scalar))
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
        Value::Number(n) if n.fract() == 0.0 => Ok(*n as i64),
        other => Err(type_error("integer", other)),
    }
}

fn vm_to_lua(lua: &Lua, value: &VmValue) -> mlua::Result<Value> {
    match value {
        VmValue::Scalar(scalar) => Ok(match scalar {
            ScalarValue::F32(v) => Value::Number(*v as f64),
            ScalarValue::F64(v) => Value::Number(*v),
            ScalarValue::I32(v) => Value::Integer(*v as i64),
            ScalarValue::U32(v) => Value::Integer(*v as i64),
            ScalarValue::U8(v) => Value::Integer(*v as i64),
            ScalarValue::Bool(v) => Value::Boolean(*v),
            ScalarValue::String(v) => Value::String(lua.create_string(v)?),
        }),
        VmValue::Opaque(handle) => {
            Ok(Value::UserData(lua.create_userdata(LuaHandle(*handle))?))
        }
        VmValue::Struct("()", _) => Ok(Value::Nil),
        VmValue::Struct(..) | VmValue::Enum(..) => Ok(Value::UserData(
            lua.create_userdata(LuaVmValue(value.clone()))?,
        )),
        VmValue::Array(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0)?;
            for (index, item) in items.iter().enumerate() {
                table.set(index + 1, vm_to_lua(lua, item)?)?;
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
