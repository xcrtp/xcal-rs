// xcal-egui: egui UI 层
// 依赖方向：xcal-scene ← xcal-render ← xcal-render-backends ← xcal-egui
//
// 包含：
//   - host.rs::EguiSceneHost —— egui 与场景之间的唯一适配层
//     （输入翻译为 SceneEvent、面板 chrome、写 ViewRect、内容提交）
//   - 全局 Resource 类型：UiConfig, TimeState（P1：`AppCamera` 已删除，
//     P4：`ViewRect` 已下沉到 `xcal-scene::domain`）
//   - 时间控制 / 参数面板的 **chrome**（事件消费者已归位 xcal-scene）
//
// 已迁出：
//   - `CameraInputSystem` → `xcal-scene::camera_input`（P1：相机状态归场景对象）；
//   - `SceneRecordSystem` / `MorphRecordSystem` / `ContentTextureRecordSystem`
//     → `xcal-render::record_systems`（P4：渲染动作住渲染层）；
//   - `AxesRefreshSystem` / `FunctionGraphRefreshSystem` → `xcal-scene::geometry`；
//   - `TimeControlSystem` / `ParamPanelSystem` → `xcal-scene::input`
//     （S0：场景侧的输入消费者，不含 egui 类型）。
//
// B 阶段红线：除 host.rs 外，本 crate 模块不得出现 egui 类型
// （场景逻辑可 headless 测试；迁移 UI 只需另写一个 host）。
//
// 图层合成回调（SceneCompositorCallback）位于 xcal-render-backends。
// S4.1：Painter2D 的 egui 回放实现与旧 CPU 2D 管线已删除。

pub mod frame;
pub mod health;
pub mod host;
pub mod progress;
pub mod resources;

pub use frame::{SceneFrame, install_visuals, window_options};
pub use health::HealthOverlay;
pub use host::EguiSceneHost;
pub use progress::FrameProgress;
pub use resources::{TimeState, UiConfig};
