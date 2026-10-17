//! 跨语言值（`VmValue`）与类型转换（`VmType`）。
//!
//! # 硬约束：只在跨语言边界出现
//!
//! 热路径（`FunctionX2Y` 每帧采样）继续走原生类型 + `&dyn Any`；
//! [`VmValue`] 只出现在**绑定层的转换处**（计划 §2.1 / §5.2）。
//!
//! # 方向
//!
//! ```text
//! 原生值 ──VmType::to_vm_value──> VmValue ──绑定层──> 脚本值
//! 脚本值 ──绑定层──> VmValue ──VmType::from_vm_value──> 原生值
//! ```

use std::fmt;

use crate::handle::Handle;
use crate::typespec::{HANDLE_SPEC, ScalarKind, TypeSpec, UNIT_SPEC};

/// 标量值。
#[derive(Clone, Debug, PartialEq)]
pub enum ScalarValue {
    F32(f32),
    F64(f64),
    I32(i32),
    U32(u32),
    U8(u8),
    Bool(bool),
    String(String),
}

/// 跨语言值。
///
/// 结构体 / 枚举保留**类型名**与**字段名**（都是 schema 里的 `&'static str`），
/// 因此绑定层可以按名字做检查，而不是靠位置猜。
#[derive(Clone, Debug, PartialEq)]
pub enum VmValue {
    Scalar(ScalarValue),
    Struct(&'static str, Vec<(&'static str, VmValue)>),
    Enum(&'static str, &'static str, Vec<(&'static str, VmValue)>),
    Array(Vec<VmValue>),
    Opaque(Handle),
}

/// 值 / 类型不匹配的明确错误（不 panic）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VmValueError {
    message: String,
}

impl VmValueError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for VmValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for VmValueError {}

/// 原生类型 ⇄ [`VmValue`] 的编译期映射。
///
/// `#[derive(VmType)]` 为 struct / enum 生成实现；标量 / `Handle` / `Vec<T>` / `()`
/// 由本模块内建。类型名同时用于 [`crate::IntoVmTypeName`]（derive 一并生成），
/// 保证 `signatures()` 与 schema 不漂移。
pub trait VmType: Sized + 'static {
    /// 类型描述（编译期常量）。
    const SPEC: &'static TypeSpec;

    /// 原生值 → 跨语言值。
    fn to_vm_value(&self) -> VmValue;

    /// 跨语言值 → 原生值（类型不符 ⇒ 明确错误）。
    fn from_vm_value(value: &VmValue) -> Result<Self, VmValueError>;
}

/// 取结构体字段（按名字；缺失 ⇒ 明确错误）。
pub fn field<'a>(
    fields: &'a [(&'static str, VmValue)],
    name: &'static str,
) -> Result<&'a VmValue, VmValueError> {
    fields
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v)
        .ok_or_else(|| VmValueError::new(format!("缺少字段 `{name}`")))
}

/// 断言是结构体且类型名一致，返回字段表。
pub fn expect_struct<'a>(
    value: &'a VmValue,
    name: &'static str,
) -> Result<&'a [(&'static str, VmValue)], VmValueError> {
    match value {
        VmValue::Struct(actual, fields) if *actual == name => Ok(fields),
        other => Err(VmValueError::new(format!(
            "期望结构体 `{name}`，实际是 {}",
            describe(other)
        ))),
    }
}

/// 断言是枚举且类型名一致，返回 `(变体名, 字段表)`。
pub fn expect_enum<'a>(
    value: &'a VmValue,
    name: &'static str,
) -> Result<(&'static str, &'a [(&'static str, VmValue)]), VmValueError> {
    match value {
        VmValue::Enum(actual, variant, fields) if *actual == name => Ok((*variant, fields)),
        other => Err(VmValueError::new(format!(
            "期望枚举 `{name}`，实际是 {}",
            describe(other)
        ))),
    }
}

/// 值的可读类型描述（错误信息用）。
pub fn describe(value: &VmValue) -> String {
    match value {
        VmValue::Scalar(s) => format!("标量 {s:?}"),
        VmValue::Struct(name, _) => format!("结构体 `{name}`"),
        VmValue::Enum(name, variant, _) => format!("枚举 `{name}::{variant}`"),
        VmValue::Array(_) => "数组".to_string(),
        VmValue::Opaque(_) => "不透明句柄".to_string(),
    }
}

fn mismatch(expected: &str, actual: &VmValue) -> VmValueError {
    VmValueError::new(format!("期望 {expected}，实际是 {}", describe(actual)))
}

macro_rules! impl_vm_type_scalar {
    ($ty:ty, $kind:ident, $variant:ident, $label:literal) => {
        impl VmType for $ty {
            const SPEC: &'static TypeSpec = &TypeSpec::Scalar(ScalarKind::$kind);

            fn to_vm_value(&self) -> VmValue {
                VmValue::Scalar(ScalarValue::$variant(*self))
            }

            fn from_vm_value(value: &VmValue) -> Result<Self, VmValueError> {
                match value {
                    VmValue::Scalar(ScalarValue::$variant(v)) => Ok(*v),
                    other => Err(mismatch($label, other)),
                }
            }
        }
    };
}

impl_vm_type_scalar!(f32, F32, F32, "f32");
impl_vm_type_scalar!(f64, F64, F64, "f64");
impl_vm_type_scalar!(i32, I32, I32, "i32");
impl_vm_type_scalar!(u32, U32, U32, "u32");
impl_vm_type_scalar!(u8, U8, U8, "u8");
impl_vm_type_scalar!(bool, Bool, Bool, "bool");

impl VmType for String {
    const SPEC: &'static TypeSpec = &TypeSpec::Scalar(ScalarKind::String);

    fn to_vm_value(&self) -> VmValue {
        VmValue::Scalar(ScalarValue::String(self.clone()))
    }

    fn from_vm_value(value: &VmValue) -> Result<Self, VmValueError> {
        match value {
            VmValue::Scalar(ScalarValue::String(s)) => Ok(s.clone()),
            other => Err(mismatch("String", other)),
        }
    }
}

impl VmType for Handle {
    const SPEC: &'static TypeSpec = HANDLE_SPEC;

    fn to_vm_value(&self) -> VmValue {
        VmValue::Opaque(*self)
    }

    fn from_vm_value(value: &VmValue) -> Result<Self, VmValueError> {
        match value {
            VmValue::Opaque(h) => Ok(*h),
            other => Err(mismatch("不透明句柄", other)),
        }
    }
}

impl VmType for () {
    const SPEC: &'static TypeSpec = UNIT_SPEC;

    fn to_vm_value(&self) -> VmValue {
        VmValue::Struct("()", Vec::new())
    }

    fn from_vm_value(value: &VmValue) -> Result<Self, VmValueError> {
        match value {
            VmValue::Struct("()", _) => Ok(()),
            other => Err(mismatch("()", other)),
        }
    }
}

impl<T: VmType> VmType for Vec<T> {
    const SPEC: &'static TypeSpec = &TypeSpec::Array { elem: T::SPEC };

    fn to_vm_value(&self) -> VmValue {
        VmValue::Array(self.iter().map(VmType::to_vm_value).collect())
    }

    fn from_vm_value(value: &VmValue) -> Result<Self, VmValueError> {
        match value {
            VmValue::Array(items) => items.iter().map(T::from_vm_value).collect(),
            other => Err(mismatch("数组", other)),
        }
    }
}
