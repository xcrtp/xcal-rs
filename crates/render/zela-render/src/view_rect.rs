// view_rect.rs — 渲染目标矩形的**转发路径**。
//
// S0：定义已下沉到 `zela_scene::domain::ViewRect`（场景侧 System 要读它，
// 而 `zela-scene` 不能依赖 `zela-render`）。
// 本模块只保留 `zela_render::ViewRect` 这一既有公开路径。

pub use zela_scene::domain::ViewRect;
