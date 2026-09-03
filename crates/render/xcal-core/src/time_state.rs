// time_state.rs — 全局时间状态（Resource）
//
// 从 xcal-egui::resources 迁移而来，因为它是核心时间状态，不属于 UI 层。

/// 全局时间累积（秒）。由 `DirectorAdvanceSystem` 推进（Director 是唯一时间源）；
/// 其他 System 只读它来驱动动画。
#[derive(Debug, Default)]
pub struct TimeState {
    pub elapsed: f32,
}

impl TimeState {
    pub fn new() -> Self {
        Self { elapsed: 0.0 }
    }
}
