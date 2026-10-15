// domain — UI 与场景解耦的纯数据契约
//
// 两个方向，缺一不可：
// - 向下（场景 → UI）：[`view::SceneView`] 声明"有什么图层"，UI 决定"怎么画"。
// - 向上（UI → 场景）：[`event::SceneEvent`] 把原生输入翻译成场景语义事件。
//
// 红线：本模块（以及一切场景逻辑）不得出现任何 `egui::` 类型。
// egui 适配的唯一入口在 xcal-egui 的 host（EguiSceneHost）。

pub mod camera;
pub mod event;
pub mod view;

pub use camera::{CameraCommand, CameraMode};
pub use event::{ids, InputEvents, SceneEvent, WidgetId};
pub use view::{
    ContentHandle, DEFAULT_BACKGROUND, LayerBlend, LayerDescriptor, LayerId, LayerKind, SceneView,
};
