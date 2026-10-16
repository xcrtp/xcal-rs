// xcal-egui: egui UI 层
// 依赖方向：xcal-scene ← xcal-render ← xcal-render-backends ← xcal-egui
//
// 包含：
//   - host.rs::EguiSceneHost —— egui 与场景之间的唯一适配层
//     （输入翻译为 SceneEvent、面板 chrome、ViewRect、内容提交）
//   - 全局 Resource 类型：AppCamera, TimeState, UiConfig, ViewRect
//   - 场景侧 System 实现：CameraInput（消费事件）、AxesRefresh / FunctionGraphRefresh
//     （M4：几何在场景对象层，本层只接线）、SceneRecord
//   - 时间控制事件消费：TimeControl（消费 Button/Slider 事件写 DirectorState）
//
// B 阶段红线：除 host.rs 外，本 crate 模块不得出现 egui 类型
// （场景逻辑可 headless 测试；迁移 UI 只需另写一个 host）。
//
// 图层合成回调（SceneCompositorCallback）位于 xcal-render-backends。
// S4.1：Painter2D 的 egui 回放实现与旧 CPU 2D 管线已删除。

pub mod host;
pub mod resources;
pub mod systems;
pub mod time_control;

pub use host::EguiSceneHost;
pub use resources::{AppCamera, TimeState, UiConfig, ViewRect};
pub use systems::{
    AxesRefreshSystem, CameraInputSystem, FunctionGraphRefreshSystem, SceneRecordSystem,
};
pub use time_control::TimeControlSystem;
