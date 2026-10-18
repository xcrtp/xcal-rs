//! Pod trait：不透明状态机 + 函数集合 + 协商 + 快慢双路径。

use std::any::Any;
use std::fmt;

use crate::capability::{Capability, Resolved};
use crate::error::PodError;
use crate::handle::PodId;
use crate::qualified::QualifiedName;

/// per-Pod 局部的函数编号（函数集合属于某个状态机）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FunctionId(u64);

impl FunctionId {
    /// 直接构造（自定义 Pod 可在慢路径里使用）。
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// 原始值。
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for FunctionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// 一个 Pod：**不透明状态机** + 它的函数集合。
///
/// # 借用方案（§0.2.1 第 3 条）
///
/// 快慢两路径都用 `&self`。有状态 Pod 用内部可变性（`Cell` / `RefCell` / 锁）；
/// 状态归 Pod 自己管，外部看不到"它在变"。
///
/// # 快路径 vs 慢路径
///
/// - **快路径**：[`Pod::negotiate`] 一次性解析出 typed 句柄，之后每次调用 =
///   一次 fn 指针间接寻址；无字符串比较、无 downcast；
/// - **慢路径**：[`Pod::call`] 按 [`FunctionId`] + 类型擦除实参保底；
///   纯解释型 Pod 无法提供 typed 可调用时走它。
///
/// 两者必须**结果一致**（用例锁定）。
pub trait Pod: 'static {
    /// per-Pod 身份（用于不透明句柄的跨 Pod 检测）。
    fn id(&self) -> PodId;

    /// 具体状态机的向下转型入口（有状态 Pod 在函数里读自己的状态）。
    fn as_any(&self) -> &dyn Any;

    /// 函数集合的完整函数名清单。
    ///
    /// 这是"Pod 自述"的最小形态（§3 待定项 4：直接返回 `&[QualifiedName]` 最简单）。
    fn functions(&self) -> &[QualifiedName];

    /// 按完整函数名解析出 typed 可调用对象。
    ///
    /// 返回 `None` ⇒ 该 Pod 没有（或只有 erased 实现、无法提供快路径）。
    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)>;

    /// 慢路径（保底）：按函数编号 + 类型擦除实参调用。
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError>;

    /// 按完整函数名取函数编号（默认按 [`Pod::functions`] 的顺序）。
    fn function_id(&self, name: &QualifiedName) -> Option<FunctionId> {
        self.functions()
            .iter()
            .position(|n| n == name)
            .map(|i| FunctionId::new(i as u64))
    }

    /// 以 capability struct 为泛型实参协商；**全有或全无**。
    ///
    /// `C` 是 **struct**（如 `MathPod<'a>`），不是 trait —— 见 [`Capability`]。
    ///
    /// 对 `dyn Pod`（如注入后的 `Arc<dyn Pod>`）请用自由函数
    /// [`negotiate_dyn`]：泛型方法不进入 vtable，trait object 上调不到。
    fn negotiate<'a, C>(&'a self) -> Option<C>
    where
        C: Capability<'a>,
        Self: Sized + 'a,
    {
        negotiate_dyn::<C>(self)
    }
}

/// 对 **trait object**（`&dyn Pod`）可用的协商入口。
///
/// # 为什么是自由函数而不是 trait 方法
///
/// `Pod::negotiate` 是泛型方法 + `Self: Sized` ⇒ 只存在于具体类型，trait object
/// 上没有这个符号。泛型方法**无法** object-safe（即便去掉 `Self: Sized`，
/// vtable 也放不下单态化后的所有 `C`）。
///
/// 宿主把能力以 `Arc<dyn Pod>` 注入后，Pod 侧拿到的正是 trait object ——
/// 这条路径必须用自由函数。
///
/// 行为与 [`Pod::negotiate`] **完全一致**：按完整函数名逐个解析，全有或全无；
/// 调用期仍是一次 fn 指针间接寻址（协商只发生一次）。
pub fn negotiate_dyn<'a, C>(pod: &'a dyn Pod) -> Option<C>
where
    C: Capability<'a>,
{
    let sigs = C::signatures();
    let mut resolved: Vec<Resolved<'a>> = Vec::with_capacity(sigs.len());
    for sig in sigs {
        let name = sig.qualified();
        // 任一签名缺失 ⇒ 整个协商失败（不返回"有的能调、有的崩"的句柄）。
        let callable = pod.resolve(&name)?;
        resolved.push(Resolved {
            signature: *sig,
            callable,
        });
    }
    C::from_negotiated(pod, &resolved)
}
