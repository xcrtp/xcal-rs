//! VM 抽象的错误类型（手写，不引入 thiserror —— 零依赖）。

use std::fmt;

use crate::type_name::VmTypeName;

/// VM / capability / 句柄相关的统一错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VmError {
    /// 函数集合里没有这个完整函数名。
    UnknownFunction(String),
    /// `QualifiedName` 解析失败。
    BadQualifiedName(String),
    /// 慢路径实参个数不匹配。
    ArityMismatch { expected: usize, got: usize },
    /// 慢路径实参类型不匹配（`index` 从 0 起）。
    ArgTypeMismatch { index: usize, expected: VmTypeName },
    /// 句柄已 release（或该槽位从未有效 / 已被复用）。
    HandleReleased { slot: u64 },
    /// 句柄属于另一个 VM —— 第一版明确禁止跨 VM 传递。
    CrossVmHandle { expected: u64, actual: u64 },
    /// VM 内部调用失败（自定义 VM 的扩展点）。
    CallFailed(String),
}

impl fmt::Display for VmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownFunction(name) => write!(f, "未知函数：{name}"),
            Self::BadQualifiedName(input) => write!(f, "无法解析完整函数名：{input}"),
            Self::ArityMismatch { expected, got } => {
                write!(f, "参数个数不匹配：期望 {expected}，实际 {got}")
            }
            Self::ArgTypeMismatch { index, expected } => {
                write!(f, "第 {index} 个参数类型不匹配：期望 {expected}")
            }
            Self::HandleReleased { slot } => write!(f, "句柄已释放（槽位 {slot}）"),
            Self::CrossVmHandle { expected, actual } => {
                write!(f, "跨 VM 句柄：句柄属于 VM {actual}，当前 VM 是 {expected}")
            }
            Self::CallFailed(msg) => write!(f, "调用失败：{msg}"),
        }
    }
}

impl std::error::Error for VmError {}
