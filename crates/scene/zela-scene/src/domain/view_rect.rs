// view_rect.rs — 渲染目标矩形（Resource）。
//
// 归位（S0）：`ViewRect` 原先在 `zela-render`（更早之前在 `zela-egui`）。
// 场景侧 System（`AxesRefreshSystem` 等）需要读它来决定"相机绑定到哪个视口"，
// 而 `zela-scene` 不能反向依赖 `zela-render` ⇒ 定义下沉到场景 domain。
//
// 写入仍由宿主完成（GUI 在每帧布局后写；headless 在启动时写整幅画面）。
// 它是纯数据，不含任何 ECS / 渲染后端 / egui 类型。

use ze_notion::{Rect2D, Vec2};

/// 本帧 2D 渲染目标的视口矩形（像素，屏幕坐标）。
///
/// 与相机的关系：相机（`Camera2DHandle` → `Framing2D`）只给**取景**，
/// 视口矩形由本资源在录制时绑定 —— 这正是 P1-2 "相机与 rect 解绑"的落点。
#[derive(Debug, Clone)]
pub struct ViewRect {
    /// 视口矩形（像素，屏幕坐标）。
    pub rect: Rect2D,
}

impl Default for ViewRect {
    fn default() -> Self {
        Self {
            rect: Rect2D::new(Vec2::ZERO, Vec2::new(800.0, 600.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_view_rect_is_800x600() {
        assert_eq!(ViewRect::default().rect.size(), Vec2::new(800.0, 600.0));
    }
}
