// binding.rs — **JS 适配层**：通用驱动（`ze-pod-binding`）+ 只属于 JS 的 codec。
//
// ```text
// C::calls() ──► BindingPlan（语言无关）
//                      │
//                      ├─ 每个函数 → JS 函数对象 → invoke(&JsCodec, ...)
//                      └─ 每个类型 → JS 构造器（组件 / 值 / 枚举）
// ```
//
// 语言无关的部分（计划、类型收集、调用编排、JSON 文档）在 `ze-pod-binding`；
// 本文件只保留 `js_to_pod` / `pod_to_js` 与 JS 函数对象创建。
//
// # 与 Lua 侧的差异（显式口径）
//
// | 概念 | Lua | JS |
// |---|---|---|
// | 句柄 / 值 / 枚举令牌 | `userdata` | 带 `__zela_pod` 隐藏属性的对象（令牌表下标） |
// | 结构体入参 | table | 普通对象（也接受令牌） |
// | `()` 返回值 | `nil` | `undefined` |
// | 整数 | `i64` 原生 | **只有 f64** ⇒ 按 `ScalarKind` 显式校验 / 转换（越界报错） |

use std::cell::RefCell;
use std::sync::Arc;

use boa_engine::{
    Context, JsError, JsNativeError, JsObject, JsResult, JsString, JsValue, NativeFunction,
    object::{FunctionObjectBuilder, ObjectInitializer, builtins::JsArray},
    property::Attribute,
};
use ze_pod_binding::{BindingPlan, ValueCodec, short_name};
use ze_pod_core::{Capability, ErasedFn, Field, Pod, PodError, PodValue, ScalarKind, ScalarValue, TypeSpec};

/// JS 侧令牌的隐藏属性名（不参与用户可见 API）。
const TOKEN_KEY: &str = "__zela_pod";

/// 每个 `JsPod` 一份的宿主侧状态（挂在 `Context` 的 host-defined 数据里）。
///
/// 为什么放这里：`NativeFunction::from_copy_closure` 的闭包必须是 `Copy`，
/// 捕获不了 `Arc<dyn Pod>`；而 `Context::get_data` 让闭包只捕获一个 **下标**（`usize`）。
pub(crate) struct HostState {
    /// capability 的 Service Pod（按注册顺序；闭包捕获下标）。
    services: RefCell<Vec<Arc<dyn Pod>>>,
    /// JS 侧不透明值（句柄 / 结构体 / 枚举）令牌表。
    tokens: RefCell<Vec<PodValue>>,
}

impl HostState {
    pub(crate) fn new() -> Self {
        Self {
            services: RefCell::new(Vec::new()),
            tokens: RefCell::new(Vec::new()),
        }
    }

    fn push_service(&self, pod: Arc<dyn Pod>) -> usize {
        let mut services = self.services.borrow_mut();
        services.push(pod);
        services.len() - 1
    }

    fn service(&self, index: usize) -> Option<Arc<dyn Pod>> {
        self.services.borrow().get(index).cloned()
    }

    fn push_token(&self, value: PodValue) -> usize {
        let mut tokens = self.tokens.borrow_mut();
        tokens.push(value);
        tokens.len() - 1
    }

    fn token(&self, id: usize) -> Option<PodValue> {
        self.tokens.borrow().get(id).cloned()
    }
}

pub(crate) fn host_state(ctx: &Context) -> &HostState {
    ctx.get_data::<HostState>()
        .expect("JsPod 构造时必须插入 HostState")
}

/// 注册一个 capability 的全局命名空间（`scene` / `content`）与类型构造器。
pub(crate) fn bind<C: Capability<'static>>(
    ctx: &mut Context,
    global_name: &str,
    service: Arc<dyn Pod>,
) -> Result<(), PodError> {
    let plan = BindingPlan::new(C::calls());
    let index = host_state(ctx).push_service(service);

    let namespace = ObjectInitializer::new(ctx).build();
    for item in plan.calls {
        let item = *item;
        let closure = move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
            dispatch(index, item, args, ctx)
        };
        let function = FunctionObjectBuilder::new(
            ctx.realm(),
            NativeFunction::from_copy_closure(closure),
        )
        .name(item.signature.name)
        .length(item.io.params.len())
        .build();
        namespace
            .set(JsString::from(item.signature.name), function, false, ctx)
            .map_err(js_err_to_pod)?;
    }
    ctx.global_object()
        .set(JsString::from(global_name), namespace, false, ctx)
        .map_err(js_err_to_pod)?;

    register_types(ctx, &plan, index)?;
    Ok(())
}

/// 一次 capability 调用的 JS 侧入口（函数对象 → 通用 `invoke`）。
fn dispatch(
    service_index: usize,
    item: ErasedFn,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let service = host_state(ctx).service(service_index).ok_or_else(|| {
        JsError::from_native(
            JsNativeError::typ().with_message(format!("能力 `{}` 未注册", item.signature.name)),
        )
    })?;
    let codec = JsCodec {
        ctx: RefCell::new(ctx),
    };
    ze_pod_binding::invoke(&codec, &*service, item, args).map_err(|e| {
        JsError::from_native(JsNativeError::typ().with_message(e.to_string()))
    })
}

/// JS 值 ↔ `PodValue`（**本文件唯一的语言特定逻辑**）。
struct JsCodec<'a> {
    ctx: RefCell<&'a mut Context>,
}

impl ValueCodec for JsCodec<'_> {
    type Value = JsValue;
    type Error = PodError;

    fn to_pod(&self, value: &JsValue, spec: &'static TypeSpec) -> Result<PodValue, PodError> {
        let mut ctx = self.ctx.borrow_mut();
        js_to_pod(&mut ctx, value, spec)
    }

    fn from_pod(&self, value: &PodValue) -> Result<JsValue, PodError> {
        let mut ctx = self.ctx.borrow_mut();
        pod_to_js(&mut ctx, value)
    }

    fn error(&self, message: String) -> PodError {
        PodError::CallFailed(message)
    }
}

/// 自动注册所有可达类型的构造器（组件 / 值 / 枚举）。
fn register_types(
    ctx: &mut Context,
    plan: &BindingPlan,
    service_index: usize,
) -> Result<(), PodError> {
    for (&full_name, &spec) in &plan.types {
        let global = short_name(full_name);
        match spec {
            TypeSpec::Struct { name, fields } => {
                let fields = *fields;
                let type_name = *name;
                match plan.component_ctor(type_name) {
                    // 组件：`Circle({...})` ⇒ 调用宿主构造函数 ⇒ 句柄令牌。
                    Some(ctor) => {
                        let closure = move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                            let input = object_arg(args, type_name)?;
                            let value = object_to_pod(ctx, &input, fields, type_name, None)
                                .map_err(|e| js_native(e.to_string()))?;
                            let service = host_state(ctx).service(service_index).ok_or_else(|| {
                                JsError::from_native(
                                    JsNativeError::typ().with_message("能力未注册"),
                                )
                            })?;
                            let out = (ctor.call)(&*service, &[value])
                                .map_err(|e| js_native(e.to_string()))?;
                            pod_to_js_result(ctx, &out)
                        };
                        let function = FunctionObjectBuilder::new(
                            ctx.realm(),
                            NativeFunction::from_copy_closure(closure),
                        )
                        .name(global)
                        .length(1)
                        .build();
                        ctx.global_object()
                            .set(JsString::from(global), function, false, ctx)
                            .map_err(js_err_to_pod)?;
                    }
                    // 纯值结构体：`Color({...})` ⇒ 值令牌。
                    None => {
                        let closure = move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                            let input = object_arg(args, type_name)?;
                            let value = object_to_pod(ctx, &input, fields, type_name, None)
                                .map_err(|e| js_native(e.to_string()))?;
                            pod_to_js_result(ctx, &value)
                        };
                        let function = FunctionObjectBuilder::new(
                            ctx.realm(),
                            NativeFunction::from_copy_closure(closure),
                        )
                        .name(global)
                        .length(1)
                        .build();
                        ctx.global_object()
                            .set(JsString::from(global), function, false, ctx)
                            .map_err(js_err_to_pod)?;
                    }
                }
            }
            TypeSpec::Enum { name, variants } => {
                let namespace = ObjectInitializer::new(ctx).build();
                for variant in *variants {
                    let type_name = *name;
                    let variant_name = variant.name;
                    let fields = variant.fields;
                    if fields.is_empty() {
                        let token = pod_to_js_result(
                            ctx,
                            &PodValue::Enum(type_name, variant_name, Vec::new()),
                        )
                        .map_err(js_err_to_pod)?;
                        namespace
                            .set(JsString::from(variant_name), token, false, ctx)
                            .map_err(js_err_to_pod)?;
                    } else {
                        let closure =
                            move |_this: &JsValue, args: &[JsValue], ctx: &mut Context| {
                                let input = object_arg(args, type_name)?;
                                let value = object_to_pod(
                                    ctx,
                                    &input,
                                    fields,
                                    type_name,
                                    Some(variant_name),
                                )
                                .map_err(|e| js_native(e.to_string()))?;
                                pod_to_js_result(ctx, &value)
                            };
                        let function = FunctionObjectBuilder::new(
                            ctx.realm(),
                            NativeFunction::from_copy_closure(closure),
                        )
                        .name(variant_name)
                        .length(1)
                        .build();
                        namespace
                            .set(JsString::from(variant_name), function, false, ctx)
                            .map_err(js_err_to_pod)?;
                    }
                }
                ctx.global_object()
                    .set(JsString::from(global), namespace, false, ctx)
                    .map_err(js_err_to_pod)?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn js_native(message: impl Into<String>) -> JsError {
    JsError::from_native(JsNativeError::typ().with_message(message.into()))
}

fn js_err_to_pod(error: JsError) -> PodError {
    PodError::CallFailed(format!("JS：{error}"))
}

fn pod_to_js_result(ctx: &mut Context, value: &PodValue) -> JsResult<JsValue> {
    pod_to_js(ctx, value).map_err(|e| js_native(e.to_string()))
}

/// 取第一个实参并断言它是对象（构造器入参）。
fn object_arg(args: &[JsValue], type_name: &str) -> JsResult<JsObject> {
    let value = args.first().cloned().unwrap_or_else(JsValue::undefined);
    if value.is_undefined() || value.is_null() {
        return Err(js_native(format!(
            "`{type_name}` 需要一个对象实参，如 `{type_name}({{ ... }})`"
        )));
    }
    value
        .as_object()
        .ok_or_else(|| js_native(format!("`{type_name}` 的实参必须是对象")))
}

fn object_to_pod(
    ctx: &mut Context,
    object: &JsObject,
    fields: &'static [Field],
    type_name: &'static str,
    variant: Option<&'static str>,
) -> Result<PodValue, PodError> {
    let mut values = Vec::with_capacity(fields.len());
    for field in fields {
        let value = object
            .get(JsString::from(field.name), ctx)
            .map_err(js_err_to_pod)?;
        if value.is_undefined() {
            return Err(PodError::CallFailed(format!(
                "`{type_name}` 缺少字段 `{}`",
                field.name
            )));
        }
        values.push((field.name, js_to_pod(ctx, &value, field.ty)?));
    }
    Ok(match variant {
        Some(variant) => PodValue::Enum(type_name, variant, values),
        None => PodValue::Struct(type_name, values),
    })
}

fn js_to_pod(
    ctx: &mut Context,
    value: &JsValue,
    spec: &'static TypeSpec,
) -> Result<PodValue, PodError> {
    match spec {
        TypeSpec::Scalar(kind) => scalar_from_js(value, *kind),
        TypeSpec::Opaque(_) => match token_of(ctx, value) {
            Some(PodValue::Opaque(handle)) => Ok(PodValue::Opaque(handle)),
            Some(other) => Err(PodError::CallFailed(format!(
                "期望 zela 句柄，实际是 {}",
                ze_pod_core::describe(&other)
            ))),
            None => Err(type_error("zela 句柄", value)),
        },
        TypeSpec::Array { elem } => {
            let object = value
                .as_object()
                .filter(|object| object.is_array())
                .ok_or_else(|| type_error("数组", value))?;
            let length = array_length(ctx, &object)?;
            let mut values = Vec::with_capacity(length);
            for index in 0..length {
                let item = object
                    .get(JsString::from(index.to_string()), ctx)
                    .map_err(js_err_to_pod)?;
                values.push(js_to_pod(ctx, &item, elem)?);
            }
            Ok(PodValue::Array(values))
        }
        TypeSpec::Struct { name, fields } => {
            if let Some(value) = token_of(ctx, value) {
                return match &value {
                    PodValue::Struct(actual, _) if actual == name => Ok(value),
                    other => Err(PodError::CallFailed(format!(
                        "期望结构体 `{name}`，实际是 {}",
                        ze_pod_core::describe(other)
                    ))),
                };
            }
            let object = value
                .as_object()
                .ok_or_else(|| type_error("对象（结构体）", value))?;
            object_to_pod(ctx, &object, fields, name, None)
        }
        TypeSpec::Enum { name, .. } => match token_of(ctx, value) {
            Some(value @ PodValue::Enum(actual, _, _)) if actual == *name => Ok(value),
            Some(other) => Err(PodError::CallFailed(format!(
                "期望枚举 `{name}`，实际是 {}",
                ze_pod_core::describe(&other)
            ))),
            None => Err(PodError::CallFailed(format!(
                "枚举 `{name}` 请用构造器创建（如 `{name}.Variant({{ ... }})`）"
            ))),
        },
    }
}

fn array_length(ctx: &mut Context, object: &JsObject) -> Result<usize, PodError> {
    let length = object
        .get(JsString::from("length"), ctx)
        .map_err(js_err_to_pod)?;
    let length = length
        .as_number()
        .ok_or_else(|| PodError::CallFailed("数组 length 不是数字".to_string()))?;
    if length < 0.0 || length.fract() != 0.0 {
        return Err(PodError::CallFailed(format!("数组 length 非法：{length}")));
    }
    Ok(length as usize)
}

/// 令牌对象 → 内部 `PodValue`（不是令牌 ⇒ `None`）。
fn token_of(ctx: &mut Context, value: &JsValue) -> Option<PodValue> {
    let object = value.as_object()?;
    let id = object.get(JsString::from(TOKEN_KEY), ctx).ok()?;
    let id = id.as_number()?;
    if id < 0.0 || id.fract() != 0.0 {
        return None;
    }
    host_state(ctx).token(id as usize)
}

fn token_object(ctx: &mut Context, id: usize) -> JsValue {
    ObjectInitializer::new(ctx)
        .property(
            JsString::from(TOKEN_KEY),
            JsValue::from(id as f64),
            Attribute::all(),
        )
        .build()
        .into()
}

fn scalar_from_js(value: &JsValue, kind: ScalarKind) -> Result<PodValue, PodError> {
    let scalar = match kind {
        ScalarKind::F32 => ScalarValue::F32(number(value)? as f32),
        ScalarKind::F64 => ScalarValue::F64(number(value)?),
        ScalarKind::I32 => {
            ScalarValue::I32(exact_integer(value, "i32", i32::MIN as f64, i32::MAX as f64)? as i32)
        }
        ScalarKind::U32 => {
            ScalarValue::U32(exact_integer(value, "u32", 0.0, u32::MAX as f64)? as u32)
        }
        // JS 只有 f64：只接受**可精确表示**的整数（2^53 - 1），越界显式报错。
        ScalarKind::U64 => {
            ScalarValue::U64(exact_integer(value, "u64", 0.0, MAX_EXACT_INTEGER)? as u64)
        }
        ScalarKind::U8 => ScalarValue::U8(exact_integer(value, "u8", 0.0, 255.0)? as u8),
        ScalarKind::Bool => match value.as_boolean() {
            Some(value) => ScalarValue::Bool(value),
            None => return Err(type_error("boolean", value)),
        },
        ScalarKind::String => match value.as_string() {
            Some(value) => ScalarValue::String(value.to_std_string_escaped()),
            None => return Err(type_error("string", value)),
        },
    };
    Ok(PodValue::Scalar(scalar))
}

/// JS 数值能精确表示的最大整数（`Number.MAX_SAFE_INTEGER`）。
const MAX_EXACT_INTEGER: f64 = 9_007_199_254_740_991.0;

fn number(value: &JsValue) -> Result<f64, PodError> {
    value
        .as_number()
        .ok_or_else(|| type_error("number", value))
}

/// **显式整数映射**（B3-Pb）：必须是整值、在范围内，否则报错（不静默截断）。
fn exact_integer(
    value: &JsValue,
    label: &str,
    min: f64,
    max: f64,
) -> Result<f64, PodError> {
    let value_number = number(value)?;
    if !value_number.is_finite() || value_number.fract() != 0.0 {
        return Err(PodError::CallFailed(format!(
            "{label} 必须是整数，实际 {value_number}"
        )));
    }
    if value_number < min || value_number > max {
        return Err(PodError::CallFailed(format!(
            "{label} 超出范围 [{min}, {max}]，实际 {value_number}"
        )));
    }
    Ok(value_number)
}

fn pod_to_js(ctx: &mut Context, value: &PodValue) -> Result<JsValue, PodError> {
    match value {
        PodValue::Scalar(scalar) => Ok(match scalar {
            ScalarValue::F32(v) => JsValue::from(*v as f64),
            ScalarValue::F64(v) => JsValue::from(*v),
            ScalarValue::I32(v) => JsValue::from(*v as f64),
            ScalarValue::U32(v) => JsValue::from(*v as f64),
            ScalarValue::U64(v) => {
                if *v as f64 > MAX_EXACT_INTEGER {
                    return Err(PodError::CallFailed(format!(
                        "u64 超出 JS 精确整数范围（2^53 - 1）：{v}"
                    )));
                }
                JsValue::from(*v as f64)
            }
            ScalarValue::U8(v) => JsValue::from(*v as f64),
            ScalarValue::Bool(v) => JsValue::from(*v),
            ScalarValue::String(v) => JsValue::from(JsString::from(v.as_str())),
        }),
        PodValue::Struct("()", _) => Ok(JsValue::undefined()),
        PodValue::Opaque(_) | PodValue::Struct(..) | PodValue::Enum(..) => {
            let id = host_state(ctx).push_token(value.clone());
            Ok(token_object(ctx, id))
        }
        PodValue::Array(items) => {
            let array = JsArray::new(ctx)
                .map_err(|e| PodError::CallFailed(format!("JS：{e}")))?;
            for item in items {
                let value = pod_to_js(ctx, item)?;
                array
                    .push(value, ctx)
                    .map_err(|e| PodError::CallFailed(format!("JS：{e}")))?;
            }
            Ok(array.into())
        }
        // B3-0.5a：bulk 字节块不走脚本绑定（契约在签名层；传输走 typed 快路径）。
        PodValue::Bytes(bytes) => Err(PodError::CallFailed(format!(
            "字节块（{} 字节）不在脚本绑定层传输；请走 typed 快路径（D4.4）",
            bytes.len()
        ))),
    }
}

fn type_error(expected: &str, actual: &JsValue) -> PodError {
    PodError::CallFailed(format!(
        "参数类型不匹配：期望 {expected}，实际 {}",
        actual.type_of()
    ))
}
