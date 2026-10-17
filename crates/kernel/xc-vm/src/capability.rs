//! capability：运行时协商出的（typed）接口。
//!
//! `Capability` 是 trait，但**泛型实参是 struct**（如 `MathVm<'a>`），不是 trait ——
//! Rust 的 trait 不是类型，不能作 `negotiate::<C>` 的实参。
//!
//! 宏（`xc-vm-macro` 的 `#[capability]`）为生成的 struct 实现本 trait：
//! - [`Capability::signatures`]：声明时逐条写下的函数签名（运行时凭据）；
//! - [`Capability::meta`]：可选自述信息（version / description）；
//! - [`Capability::from_negotiated`]：协商成功 ⇒ 构造自身（持有 `&dyn Vm` + 解析结果）。

use std::any::Any;

use crate::qualified::QualifiedName;
use crate::type_name::VmTypeName;
use crate::vm::Vm;

/// capability 的自述元数据（全部可选，缺省为"无"）。
///
/// **`version` 不参与协商匹配**：它只是自述信息。若参与，VM 版本略高就会导致
/// 协商失败；将来需要"至少 vN"的约束时再引入显式规则（本轮不做）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapabilityMeta {
    /// 自述版本（不参与匹配）。
    pub version: Option<&'static str>,
    /// 自述描述。
    pub description: Option<&'static str>,
}

impl CapabilityMeta {
    /// 什么都没有（缺省 meta）。
    pub const NONE: Self = Self {
        version: None,
        description: None,
    };

    /// 逐字段构造（`const`，便于宏生成）。
    pub const fn new(version: Option<&'static str>, description: Option<&'static str>) -> Self {
        Self {
            version,
            description,
        }
    }

    /// 是否没有任何自述信息。
    pub const fn is_empty(&self) -> bool {
        self.version.is_none() && self.description.is_none()
    }
}

/// 一条函数签名（编译期可构造，用于 `signatures()` 的 `const` 数组）。
///
/// `params` / `ret` 是稳定类型名；完整函数名由 [`FnSig::qualified`] 惰性拼出。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FnSig {
    /// namespace（可为空串；`"xcal.math"` / `"ns::path"`）。
    pub namespace: &'static str,
    /// 函数名（不含 namespace）。
    pub name: &'static str,
    /// 参数类型名（按声明顺序）。
    pub params: &'static [VmTypeName],
    /// 返回类型名。
    pub ret: VmTypeName,
}

impl FnSig {
    /// 逐字段构造（`const`，便于宏生成 `const` 数组）。
    pub const fn new(
        namespace: &'static str,
        name: &'static str,
        params: &'static [VmTypeName],
        ret: VmTypeName,
    ) -> Self {
        Self {
            namespace,
            name,
            params,
            ret,
        }
    }

    /// 转成完整函数名（`"ns::name(A, B) -> R"`）。
    ///
    /// 分配一次字符串：只发生在**协商期**，不在调用热路径。
    pub fn qualified(&self) -> QualifiedName {
        QualifiedName::new(
            self.namespace,
            self.name,
            self.params.iter().map(|s| (*s).to_string()).collect(),
            self.ret,
        )
    }
}

/// 协商产物的一项：签名 + VM 提供的可调用对象。
///
/// `callable` 的类型擦除载体里存的是**裸 fn 指针**（如
/// `fn(&dyn Vm, f32) -> f32`）；宏生成的 `from_negotiated` 用
/// `downcast_ref::<fn(...)>()` 取回并写入句柄字段 —— 不是字符串查找、不是 downcast 调用。
pub struct Resolved<'a> {
    /// 这一项对应的签名（与声明逐条对应）。
    pub signature: FnSig,
    /// VM 侧的可调用对象（类型擦除）。
    pub callable: &'a (dyn Any + Send + Sync),
}

/// capability 的凭据接口：由 `#[capability]` 宏为生成的 struct 实现。
///
/// `'a` 是 VM 的借用生命周期：句柄持有 `&'a dyn Vm`，与 VM 同寿。
pub trait Capability<'a>: Sized {
    /// 声明时写下的函数签名，**逐条对应**（用例锁定，防错位）。
    fn signatures() -> &'static [FnSig];

    /// 可选自述信息；缺省为 [`CapabilityMeta::NONE`]。
    ///
    /// **不参与协商**：`version` 不同不会导致 `negotiate` 失败。
    fn meta() -> CapabilityMeta {
        CapabilityMeta::NONE
    }

    /// 协商成功 ⇒ 构造自身；任一缺失 ⇒ `None`（全有或全无）。
    ///
    /// `resolved` 与 [`Self::signatures`] **按序对应**。
    fn from_negotiated(vm: &'a dyn Vm, resolved: &[Resolved<'_>]) -> Option<Self>;
}
