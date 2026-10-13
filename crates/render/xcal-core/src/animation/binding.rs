// binding.rs — 显式绑定表
//
// 取代旧的组件组合查询（三者挂同一实体）。新增动画 = 加一行 Binding，不改 System 源码。

use hecs::Entity;
use xc_time::{ClockId, Curve};

/// 绑定目标：描述哪类组件的哪个字段需要被驱动。
///
/// 每个 variant 携带一个 hecs::Entity，由 BindingApplySystem 在 ECS World 中查找。
#[derive(Debug, Clone)]
pub enum BindingTarget {
    /// 写 `Circle.radius`（2D 圆半径振荡）。
    CircleRadius(Entity),
    /// 写 `PhaseAnim.phase`（供 FunctionX2Y 水平采样滑动）。
    Phase(Entity),
    /// 写 `Camera3D.eye`（3D 轨道相机）。
    CameraEye(Entity),
    /// 写 `MorphShape.t`（形状形变参数；值取 `Curve::sample` 的 `Scalar`）。
    MorphT(Entity),
}

/// seek 语义：当前全为 O1（纯函数、无记忆），弹簧等有记忆演化器需要 Replay 模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekKind {
    O1,
    Replay,
}

/// 单条绑定。
#[derive(Debug, Clone)]
pub struct Binding {
    pub clock: ClockId,
    pub curve: Curve,
    pub target: BindingTarget,
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
    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}
