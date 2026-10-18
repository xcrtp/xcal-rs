// view_rect.rs — 渲染目标矩形的**转发路径**。
//
// S0：定义已下沉到 `xcal_scene::domain::ViewRect`（场景侧 System 要读它，
// 而 `xcal-scene` 不能依赖 `xcal-render`）。
// 本模块只保留 `xcal_render::ViewRect` 这一既有公开路径。

pub use xcal_scene::domain::ViewRect;
