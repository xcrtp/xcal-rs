//! 类型描述（IDL schema）：语言无关、零依赖、编译期常量。
//!
//! # 为什么需要它
//!
//! 旧形态里 `PodTypeName = &'static str`，`"FloatList"` / `"ComponentList"` 只是
//! **没有定义的名字** —— 生成器不知道元素类型，文档只能写 `table`，绑定只能手写。
//!
//! [`TypeSpec`] 把"类型"从字符串升级成**结构**：标量 / 结构体 / 枚举 / 数组 / 不透明句柄。
//! 它是**描述**，不是值载体；跨语言的值见 [`crate::PodValue`]。
//!
//! # 与 serde 的分工
//!
//! serde 是序列化协议（推 / 拉），本模块是**类型反射**（生成文档与绑定）。
//! 两者数据模型同构，但用途不同，故不复用（见计划 §4.2）。

use crate::capability::FnSig;
use crate::error::PodError;
use crate::value::PodValue;

/// 标量种类（最小集：场景实际用到的）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarKind {
    F32,
    F64,
    I32,
    U32,
    /// 64 位无符号整数（`u64` / [`crate::PodHandle`] 的寻址号）。
    U64,
    /// 颜色分量（`Color32` 的 r/g/b/a）。
    U8,
    Bool,
    String,
}

/// 类型描述。
///
/// 所有字段都是 `&'static`：schema 由 derive 生成成编译期常量，零运行时开销。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TypeSpec {
    Scalar(ScalarKind),
    Struct {
        name: &'static str,
        fields: &'static [Field],
    },
    Enum {
        name: &'static str,
        variants: &'static [Variant],
    },
    Array {
        elem: &'static TypeSpec,
    },
    /// 不透明句柄（组件 / 对象 / 曲线 / 动作）：值只以 [`crate::Handle`] 过边界。
    Opaque(&'static str),
}

impl TypeSpec {
    /// 命名类型的名字（标量 / 数组 / 匿名句柄返回 `None`）。
    pub const fn type_name(&self) -> Option<&'static str> {
        match self {
            TypeSpec::Struct { name, .. } | TypeSpec::Enum { name, .. } => Some(*name),
            TypeSpec::Opaque(name) => Some(*name),
            TypeSpec::Scalar(_) | TypeSpec::Array { .. } => None,
        }
    }

    /// 是否是不透明句柄。
    pub const fn is_opaque(&self) -> bool {
        matches!(self, TypeSpec::Opaque(_))
    }
}

/// 结构体字段（名字 + 类型）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    pub name: &'static str,
    pub ty: &'static TypeSpec,
}

/// 枚举变体（名字 + 字段；无字段即 unit 变体）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Variant {
    pub name: &'static str,
    pub fields: &'static [Field],
}

/// `()` 的 schema：**空结构体**（不额外引入 `Unit` 变体，保持五变体最小集）。
pub const UNIT_SPEC: &TypeSpec = &TypeSpec::Struct {
    name: "()",
    fields: &[],
};

/// `ze_pod::Handle` 的 schema。
pub const HANDLE_SPEC: &TypeSpec = &TypeSpec::Opaque("ze_pod.Handle");

/// 一个形参：**名字** + 类型。
///
/// 形参名进 schema 是硬要求（计划 §4.3）：否则 LuaLS 只能生成 `p1..pN`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParamSpec {
    pub name: &'static str,
    pub ty: &'static TypeSpec,
}

/// 一条函数的 I/O 描述（与 [`FnSig`] 按序对应）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FnIo {
    pub params: &'static [ParamSpec],
    pub ret: &'static TypeSpec,
}

/// **绑定用的类型擦除入口**：由 `#[capability(bindings)]` 为每个字段生成。
///
/// `call` 在装配期按完整函数名解析出 typed 裸 fn 指针，再做
/// `PodValue ↔ 原生值` 转换 —— **只在跨语言边界**，不进热路径。
#[derive(Clone, Copy)]
pub struct ErasedFn {
    /// 与 [`crate::Capability::signatures`] 中同名同序的那一条。
    pub signature: FnSig,
    /// 形参名 + 结构类型。
    pub io: FnIo,
    /// 类型擦除调用：参数与返回值都是 [`PodValue`]。
    pub call: fn(&dyn crate::Pod, &[PodValue]) -> Result<PodValue, PodError>,
}

impl std::fmt::Debug for ErasedFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ErasedFn")
            .field("signature", &self.signature)
            .field("io", &self.io)
            .finish_non_exhaustive()
    }
}
