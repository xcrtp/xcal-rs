//! 跨语言值（`PodValue`）与类型转换（`PodType`）。
//!
//! # 硬约束：只在跨语言边界出现
//!
//! 热路径（`FunctionX2Y` 每帧采样）继续走原生类型 + `&dyn Any`；
//! [`PodValue`] 只出现在**绑定层的转换处**（计划 §2.1 / §5.2）。
//!
//! # 方向
//!
//! ```text
//! 原生值 ──PodType::to_pod_value──> PodValue ──绑定层──> 脚本值
//! 脚本值 ──绑定层──> PodValue ──PodType::from_pod_value──> 原生值
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
    U64(u64),
    U8(u8),
    Bool(bool),
    String(String),
}

/// 跨语言值。
///
/// 结构体 / 枚举保留**类型名**与**字段名**（都是 schema 里的 `&'static str`），
/// 因此绑定层可以按名字做检查，而不是靠位置猜。
#[derive(Clone, Debug, PartialEq)]
pub enum PodValue {
    Scalar(ScalarValue),
    Struct(&'static str, Vec<(&'static str, PodValue)>),
    Enum(&'static str, &'static str, Vec<(&'static str, PodValue)>),
    Array(Vec<PodValue>),
    Opaque(Handle),
    /// **bulk 字节块**（B3-0.5a / D4.3）：顶点 / uniform 等大块数据的契约表示。
    ///
    /// 用 [`PodBytes`] newtype 而不是特化 `Vec<u8>`：`Vec<u8>` 的既有语义是
    /// `Array(U8)`（逐字节），两者若同时存在会产生"到底走哪条"的歧义。
    Bytes(PodBytes),
}

/// bulk 字节块（`PodValue::Bytes` 的载荷）。
///
/// 契约类型（用于 `signatures()` / 绑定层）；**bulk 传输的优化路径**是 typed
/// 快路径（`Pod::resolve()` 直接拿 `&[u8]`），本类型不参与逐元素转换。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PodBytes(pub Vec<u8>);

impl PodBytes {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl PodType for PodBytes {
    /// schema 上与 `Array<u8>` 同构（字节串）；区别在**值层**（`PodValue::Bytes`）。
    const SPEC: &'static TypeSpec = &TypeSpec::Array {
        elem: &TypeSpec::Scalar(ScalarKind::U8),
    };

    fn to_pod_value(&self) -> PodValue {
        PodValue::Bytes(self.clone())
    }

    fn from_pod_value(value: &PodValue) -> Result<Self, PodValueError> {
        match value {
            PodValue::Bytes(bytes) => Ok(bytes.clone()),
            other => Err(mismatch("字节块（PodBytes）", other)),
        }
    }
}

/// 值 / 类型不匹配的明确错误（不 panic）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PodValueError {
    message: String,
}

impl PodValueError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for PodValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PodValueError {}

/// 原生类型 ⇄ [`PodValue`] 的编译期映射。
///
/// `#[derive(PodType)]` 为 struct / enum 生成实现；标量 / `Handle` / `Vec<T>` / `()`
/// 由本模块内建。类型名同时用于 [`crate::IntoPodTypeName`]（derive 一并生成），
/// 保证 `signatures()` 与 schema 不漂移。
pub trait PodType: Sized + 'static {
    /// 类型描述（编译期常量）。
    const SPEC: &'static TypeSpec;

    /// 原生值 → 跨语言值。
    fn to_pod_value(&self) -> PodValue;

    /// 跨语言值 → 原生值（类型不符 ⇒ 明确错误）。
    fn from_pod_value(value: &PodValue) -> Result<Self, PodValueError>;
}

/// 取结构体字段（按名字；缺失 ⇒ 明确错误）。
pub fn field<'a>(
    fields: &'a [(&'static str, PodValue)],
    name: &'static str,
) -> Result<&'a PodValue, PodValueError> {
    fields
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, v)| v)
        .ok_or_else(|| PodValueError::new(format!("缺少字段 `{name}`")))
}

/// 断言是结构体且类型名一致，返回字段表。
pub fn expect_struct<'a>(
    value: &'a PodValue,
    name: &'static str,
) -> Result<&'a [(&'static str, PodValue)], PodValueError> {
    match value {
        PodValue::Struct(actual, fields) if *actual == name => Ok(fields),
        other => Err(PodValueError::new(format!(
            "期望结构体 `{name}`，实际是 {}",
            describe(other)
        ))),
    }
}

/// 断言是枚举且类型名一致，返回 `(变体名, 字段表)`。
pub fn expect_enum<'a>(
    value: &'a PodValue,
    name: &'static str,
) -> Result<(&'static str, &'a [(&'static str, PodValue)]), PodValueError> {
    match value {
        PodValue::Enum(actual, variant, fields) if *actual == name => Ok((*variant, fields)),
        other => Err(PodValueError::new(format!(
            "期望枚举 `{name}`，实际是 {}",
            describe(other)
        ))),
    }
}

/// 值的可读类型描述（错误信息用）。
pub fn describe(value: &PodValue) -> String {
    match value {
        PodValue::Scalar(s) => format!("标量 {s:?}"),
        PodValue::Struct(name, _) => format!("结构体 `{name}`"),
        PodValue::Enum(name, variant, _) => format!("枚举 `{name}::{variant}`"),
        PodValue::Array(_) => "数组".to_string(),
        PodValue::Opaque(_) => "不透明句柄".to_string(),
        PodValue::Bytes(bytes) => format!("字节块（{} 字节）", bytes.len()),
    }
}

fn mismatch(expected: &str, actual: &PodValue) -> PodValueError {
    PodValueError::new(format!("期望 {expected}，实际是 {}", describe(actual)))
}

macro_rules! impl_pod_type_scalar {
    ($ty:ty, $kind:ident, $variant:ident, $label:literal) => {
        impl PodType for $ty {
            const SPEC: &'static TypeSpec = &TypeSpec::Scalar(ScalarKind::$kind);

            fn to_pod_value(&self) -> PodValue {
                PodValue::Scalar(ScalarValue::$variant(*self))
            }

            fn from_pod_value(value: &PodValue) -> Result<Self, PodValueError> {
                match value {
                    PodValue::Scalar(ScalarValue::$variant(v)) => Ok(*v),
                    other => Err(mismatch($label, other)),
                }
            }
        }
    };
}

impl_pod_type_scalar!(f32, F32, F32, "f32");
impl_pod_type_scalar!(f64, F64, F64, "f64");
impl_pod_type_scalar!(i32, I32, I32, "i32");
impl_pod_type_scalar!(u32, U32, U32, "u32");
impl_pod_type_scalar!(u64, U64, U64, "u64");
impl_pod_type_scalar!(u8, U8, U8, "u8");
impl_pod_type_scalar!(bool, Bool, Bool, "bool");

impl PodType for String {
    const SPEC: &'static TypeSpec = &TypeSpec::Scalar(ScalarKind::String);

    fn to_pod_value(&self) -> PodValue {
        PodValue::Scalar(ScalarValue::String(self.clone()))
    }

    fn from_pod_value(value: &PodValue) -> Result<Self, PodValueError> {
        match value {
            PodValue::Scalar(ScalarValue::String(s)) => Ok(s.clone()),
            other => Err(mismatch("String", other)),
        }
    }
}

impl PodType for Handle {
    const SPEC: &'static TypeSpec = HANDLE_SPEC;

    fn to_pod_value(&self) -> PodValue {
        PodValue::Opaque(*self)
    }

    fn from_pod_value(value: &PodValue) -> Result<Self, PodValueError> {
        match value {
            PodValue::Opaque(h) => Ok(*h),
            other => Err(mismatch("不透明句柄", other)),
        }
    }
}

impl PodType for () {
    const SPEC: &'static TypeSpec = UNIT_SPEC;

    fn to_pod_value(&self) -> PodValue {
        PodValue::Struct("()", Vec::new())
    }

    fn from_pod_value(value: &PodValue) -> Result<Self, PodValueError> {
        match value {
            PodValue::Struct("()", _) => Ok(()),
            other => Err(mismatch("()", other)),
        }
    }
}

impl<T: PodType> PodType for Vec<T> {
    const SPEC: &'static TypeSpec = &TypeSpec::Array { elem: T::SPEC };

    fn to_pod_value(&self) -> PodValue {
        PodValue::Array(self.iter().map(PodType::to_pod_value).collect())
    }

    fn from_pod_value(value: &PodValue) -> Result<Self, PodValueError> {
        match value {
            PodValue::Array(items) => items.iter().map(T::from_pod_value).collect(),
            other => Err(mismatch("数组", other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **已知向量**（B3-0.5a）：`PodBytes` 的契约表示是 `Bytes`，字节原样保留。
    #[test]
    fn pod_bytes_known_vector() {
        let bytes = PodBytes::new(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        let value = bytes.to_pod_value();
        match &value {
            PodValue::Bytes(v) => assert_eq!(v.as_slice(), &[0xDE, 0xAD, 0xBE, 0xEF]),
            other => panic!("PodBytes 必须走 Bytes 变体，实际 {}", describe(other)),
        }
        assert_eq!(PodBytes::from_pod_value(&value).unwrap(), bytes);
    }

    /// 往返：`from_pod_value(to_pod_value(x)) == x`。
    #[test]
    fn pod_bytes_roundtrip() {
        for len in [0usize, 1, 24, 32, 4096] {
            let bytes = PodBytes::new(vec![0xA5u8; len]);
            let back = PodBytes::from_pod_value(&bytes.to_pod_value()).unwrap();
            assert_eq!(back, bytes);
        }
    }

    /// **防回归歧义**（D4.3）：`Vec<u8>` 仍是 `Array(U8)`，不得变成 `Bytes`。
    #[test]
    fn vec_u8_still_uses_array() {
        let value = vec![1u8, 2, 3].to_pod_value();
        match &value {
            PodValue::Array(items) => {
                assert_eq!(items.len(), 3);
                assert!(items.iter().all(|v| matches!(v, PodValue::Scalar(ScalarValue::U8(_)))));
            }
            other => panic!("Vec<u8> 必须走 Array(U8)，实际 {}", describe(other)),
        }
        // 反向：Array 不能被当成 PodBytes 解出（显式错误）。
        assert!(PodBytes::from_pod_value(&value).is_err());
    }

    /// schema 层与 `Array<u8>` 同构（绑定生成器不需要新分支）。
    #[test]
    fn pod_bytes_spec_is_array_of_u8() {
        assert_eq!(PodBytes::SPEC, <Vec<u8> as PodType>::SPEC);
    }
}
