// binding.rs — 显式绑定表
//
// 取代旧的组件组合查询（三者挂同一实体）。新增动画 = 加一行 Binding，不改 System 源码。
//
// P0：`Binding.target` 由**枚举**改为 `Arc<dyn Target>`（类型擦除）——
// 新增驱动目标不再需要改枚举、也不需要改 System 源码。五个目标类型见
// [`crate::animation::target`]（四个既有 + 2D 相机取景）。

use std::sync::Arc;

use ze_time::{ClockId, Curve};

use crate::animation::target::Target;

/// seek 语义：当前全为 O1（纯函数、无记忆），弹簧等有记忆演化器需要 Replay 模式。
///
/// **本轮有意保留的方向标记**（§8.1）：`Replay` 从"预留未接线"升级为
/// "**Scene 不保证纯**"的正式记账——触发条件：接弹簧 / 数值积分 / 外部事件。
/// 它**不是**死代码，不得顺手删除。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekKind {
    O1,
    Replay,
}

/// 单条绑定：`(时钟, 曲线, 目标)`。
///
/// `Binding: Clone` 是硬约束（`BindingApplySystem` 每帧 clone 整表），
/// 这也是 `target` 用 `Arc<dyn Target>` 而非 `Box<dyn Target>` 的原因
/// —— `Arc` 的 clone 只是引用计数（见 `target.rs` 头部）。
#[derive(Debug, Clone)]
pub struct Binding {
    pub clock: ClockId,
    pub curve: Curve,
    /// 驱动目标（类型擦除：`BindingTable` 要装异构的所有 target）。
    pub target: Arc<dyn Target>,
    pub seek_kind: SeekKind,
}

#[derive(Debug, Default, Clone)]
pub struct BindingTable {
    pub bindings: Vec<Binding>,
}

impl BindingTable {
    pub fn new() -> Self {
        Self {
            bindings: Vec::new(),
        }
    }
    pub fn add(&mut self, binding: Binding) {
        self.bindings.push(binding);
    }
    /// 用**类型擦除的目标**添加一条绑定，`seek_kind` 取 [`SeekKind::O1`]
    /// （当前所有绑定都是纯函数 O1）。Timeline 的编译产物在此落地。
    pub fn bind_arc(&mut self, clock: ClockId, curve: Curve, target: Arc<dyn Target>) {
        self.add(Binding {
            clock,
            curve,
            target,
            seek_kind: SeekKind::O1,
        });
    }
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}
