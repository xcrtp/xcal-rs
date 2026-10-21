//! capability：运行时协商出的（typed）接口。
//!
//! `Capability` 是 trait，但**泛型实参是 struct**（如 `MathPod<'a>`），不是 trait ——
//! Rust 的 trait 不是类型，不能作 `negotiate::<C>` 的实参。
//!
//! 宏（`ze-pod-macro` 的 `#[capability]`）为生成的 struct 实现本 trait：
//! - [`Capability::signatures`]：声明时逐条写下的函数签名（运行时凭据）；
//! - [`Capability::meta`]：可选自述信息（version / description）；
//! - [`Capability::from_negotiated`]：协商成功 ⇒ 构造自身（持有 `&dyn Pod` + 解析结果）。

use std::any::Any;

use crate::qualified::QualifiedName;
use crate::type_name::PodTypeName;
use crate::typespec::ErasedFn;
use crate::pod::Pod;

/// capability 的自述元数据（全部可选，缺省为"无"）。
///
/// **`version` 不参与协商匹配**：它只是自述信息。若参与，Pod 版本略高就会导致
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
    /// namespace（可为空串；`"zela.math"` / `"ns::path"`）。
    pub namespace: &'static str,
    /// 函数名（不含 namespace）。
    pub name: &'static str,
    /// 参数类型名（按声明顺序）。
    pub params: &'static [PodTypeName],
    /// 返回类型名。
    pub ret: PodTypeName,
}

impl FnSig {
    /// 逐字段构造（`const`，便于宏生成 `const` 数组）。
    pub const fn new(
        namespace: &'static str,
        name: &'static str,
        params: &'static [PodTypeName],
        ret: PodTypeName,
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

/// **能力指纹**（B2-9 / D14）：`hash(signatures())`，与 `meta().version` 配对校验。
///
/// # 为什么自带 FNV-1a 而不是 `DefaultHasher`
/// 指纹会被**钉成常量**（版本配对的基线），必须跨 Rust 版本稳定；
/// `DefaultHasher` 的算法不保证跨版本一致。FNV-1a 是固定算法，永远稳定。
///
/// # 用法
/// 能力实现侧固定一个小表：`(version, fingerprint)`。API 变了 ⇒ 指纹变 ⇒
/// 必须**同时**递增 `version` 并更新表；否则该能力的护栏测试变红（不是警告）。
pub fn capability_fingerprint(signatures: &[FnSig]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    let mut eat = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
        // 字段分隔符（0xFF 不是合法 UTF-8 字节，避免拼接歧义）。
        hash ^= 0xFF;
        hash = hash.wrapping_mul(FNV_PRIME);
    };
    for sig in signatures {
        eat(sig.namespace.as_bytes());
        eat(sig.name.as_bytes());
        for param in sig.params {
            eat(param.as_bytes());
        }
        eat(sig.ret.as_bytes());
    }
    hash
}

/// 一条函数的**文档**（编译期常量）。
///
/// 与 [`FnSig`] **按序对应**：`signatures()[i]` 与 `docs()[i]` 描述同一个函数。
/// `description` 来自字段上的 `///`（proc-macro 可读的 `#[doc]` 属性），
/// 因此收集它是**零运行时开销**（只是多几个 `'static` 常量）。
///
/// # 为什么不直接存 `QualifiedName`
///
/// `QualifiedName` 内部是 `String`，无法在 `const` 数组里构造。故这里保留
/// 拆分字段，由 [`FnDoc::qualified`] 按需拼出（导出文档时一次调用，非热路径）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FnDoc {
    /// namespace（与 [`FnSig::namespace`] 相同）。
    pub namespace: &'static str,
    /// 函数名（与 [`FnSig::name`] 相同）。
    pub name: &'static str,
    /// 自由文本说明（来自 `///`；缺失时为空串）。
    pub description: &'static str,
    /// 参数类型名（按声明顺序）。
    pub params: &'static [PodTypeName],
    /// 返回类型名。
    pub ret: PodTypeName,
}

impl FnDoc {
    /// 逐字段构造（`const`，便于宏生成）。
    pub const fn new(
        namespace: &'static str,
        name: &'static str,
        description: &'static str,
        params: &'static [PodTypeName],
        ret: PodTypeName,
    ) -> Self {
        Self {
            namespace,
            name,
            description,
            params,
            ret,
        }
    }

    /// 转成完整函数名（形态与 [`FnSig::qualified`] 一致）。
    pub fn qualified(&self) -> QualifiedName {
        QualifiedName::new(
            self.namespace,
            self.name,
            self.params.iter().map(|s| (*s).to_string()).collect(),
            self.ret,
        )
    }
}

/// 协商产物的一项：签名 + Pod 提供的可调用对象。
///
/// `callable` 的类型擦除载体里存的是**裸 fn 指针**（如
/// `fn(&dyn Pod, f32) -> f32`）；宏生成的 `from_negotiated` 用
/// `downcast_ref::<fn(...)>()` 取回并写入句柄字段 —— 不是字符串查找、不是 downcast 调用。
pub struct Resolved<'a> {
    /// 这一项对应的签名（与声明逐条对应）。
    pub signature: FnSig,
    /// Pod 侧的可调用对象（类型擦除）。
    pub callable: &'a (dyn Any + Send + Sync),
}

/// capability 的凭据接口：由 `#[capability]` 宏为生成的 struct 实现。
///
/// `'a` 是 Pod 的借用生命周期：句柄持有 `&'a dyn Pod`，与 Pod 同寿。
pub trait Capability<'a>: Sized {
    /// 声明时写下的函数签名，**逐条对应**（用例锁定，防错位）。
    fn signatures() -> &'static [FnSig];

    /// 可选自述信息；缺省为 [`CapabilityMeta::NONE`]。
    ///
    /// **不参与协商**：`version` 不同不会导致 `negotiate` 失败。
    fn meta() -> CapabilityMeta {
        CapabilityMeta::NONE
    }

    /// 函数文档（与 [`Self::signatures`] **按序对应**）。
    ///
    /// `#[capability]` 宏默认从字段上的 `///` 生成；手写实现可省略（默认空）。
    /// 文档只用于导出（JSON / LuaLS / Markdown），**不参与协商**、无运行时开销。
    fn docs() -> &'static [FnDoc] {
        &[]
    }

    /// **绑定入口**：每条函数的类型擦除调用（参数 / 返回值是 [`crate::PodValue`]）。
    ///
    /// 由 `#[capability(bindings)]` 生成；未开启时为空。
    /// 与 [`Self::signatures`] **按序对应**，供语言绑定层自动生成胶水。
    fn calls() -> &'static [ErasedFn] {
        &[]
    }

    /// 协商成功 ⇒ 构造自身；任一缺失 ⇒ `None`（全有或全无）。
    ///
    /// `resolved` 与 [`Self::signatures`] **按序对应**。
    fn from_negotiated(pod: &'a dyn Pod, resolved: &[Resolved<'_>]) -> Option<Self>;
}
