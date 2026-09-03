// xcal-egui: egui UI 层
// 依赖方向：xcal-core ← xcal-render ← xcal-render-backends ← xcal-egui
//
// 包含：
//   - EguiFrameContext (UI 降级为 Resource)
//   - 全局 Resource 类型：AppCamera, TimeState, UiConfig, ViewRect
//   - egui System 实现：TimeUpdate, CameraInput, ControlPanel, ViewRectUpdate,
//                       GridBuild, SceneRecord, RenderSubmit
//
// Painter2D 的 egui 实现（EguiPainter）位于 xcal-render-backends。

pub mod resources;
pub mod systems;
pub mod time_control;

pub use resources::{AppCamera, EguiFrameContext, TimeState, UiConfig, ViewRect};
pub use systems::{
    AnimationApplySystem, CameraInputSystem, ControlPanelSystem, GridBuildSystem,
    RenderSubmitSystem, SceneRecordSystem, TimeUpdateSystem, ViewRectUpdateSystem,
};
pub use time_control::TimeControlSystem;
pub use xcal_render_backends::EguiPainter;
