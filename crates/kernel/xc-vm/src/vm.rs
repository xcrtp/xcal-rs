//! VM trait：不透明状态机 + 函数集合 + 协商 + 快慢双路径。

use std::any::Any;
use std::fmt;

use crate::capability::{Capability, Resolved};
use crate::error::VmError;
use crate::handle::VmId;
use crate::qualified::QualifiedName;

/// per-VM 局部的函数编号（函数集合属于某个状态机）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FunctionId(u64);

impl FunctionId {
    /// 直接构造（自定义 VM 可在慢路径里使用）。
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

/// 一个 VM：**不透明状态机** + 它的函数集合。
///
/// # 借用方案（§0.2.1 第 3 条）
///
/// 快慢两路径都用 `&self`。有状态 VM 用内部可变性（`Cell` / `RefCell` / 锁）；
/// 状态归 VM 自己管，外部看不到"它在变"。
///
/// # 快路径 vs 慢路径
///
/// - **快路径**：[`Vm::negotiate`] 一次性解析出 typed 句柄，之后每次调用 =
///   一次 fn 指针间接寻址；无字符串比较、无 downcast；
/// - **慢路径**：[`Vm::call`] 按 [`FunctionId`] + 类型擦除实参保底；
///   纯解释型 VM 无法提供 typed 可调用时走它。
///
/// 两者必须**结果一致**（用例锁定）。
pub trait Vm: 'static {
    /// per-VM 身份（用于不透明句柄的跨 VM 检测）。
    fn id(&self) -> VmId;

    /// 具体状态机的向下转型入口（有状态 VM 在函数里读自己的状态）。
    fn as_any(&self) -> &dyn Any;

    /// 函数集合的完整函数名清单。
    ///
    /// 这是"VM 自述"的最小形态（§3 待定项 4：直接返回 `&[QualifiedName]` 最简单）。
    fn functions(&self) -> &[QualifiedName];

    /// 按完整函数名解析出 typed 可调用对象。
    ///
    /// 返回 `None` ⇒ 该 VM 没有（或只有 erased 实现、无法提供快路径）。
    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)>;

    /// 慢路径（保底）：按函数编号 + 类型擦除实参调用。
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, VmError>;

    /// 按完整函数名取函数编号（默认按 [`Vm::functions`] 的顺序）。
    fn function_id(&self, name: &QualifiedName) -> Option<FunctionId> {
        self.functions()
            .iter()
            .position(|n| n == name)
            .map(|i| FunctionId::new(i as u64))
    }

    /// 以 capability struct 为泛型实参协商；**全有或全无**。
    ///
    /// `C` 是 **struct**（如 `MathVm<'a>`），不是 trait —— 见 [`Capability`]。
    fn negotiate<'a, C>(&'a self) -> Option<C>
    where
        C: Capability<'a>,
        Self: Sized + 'a,
    {
        let sigs = C::signatures();
        let mut resolved: Vec<Resolved<'a>> = Vec::with_capacity(sigs.len());
        for sig in sigs {
            let name = sig.qualified();
            // 任一签名缺失 ⇒ 整个协商失败（不返回"有的能调、有的崩"的句柄）。
            let callable = self.resolve(&name)?;
            resolved.push(Resolved {
                signature: *sig,
                callable,
            });
        }
        C::from_negotiated(self, &resolved)
    }
}
