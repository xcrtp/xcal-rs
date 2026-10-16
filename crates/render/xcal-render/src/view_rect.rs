// view_rect.rs — 渲染目标矩形（Resource）。
//
// P4 归位：`ViewRect` 原先定义在 `xcal-egui`（UI 层），而三个录制 System 读它来决定
// "2D 相机的视口矩形" —— 那是**渲染目标**的事实，不是 UI 的事实（UI 只是恰好第一个
// 知道布局的人：它每帧把布局结果写进来）。
//
// 归位后：本类型属于渲染层（录制 System 读它，不读 UI 的类型）；写入仍由宿主
// （`EguiSceneHost::update_view_rect`）在每帧布局完成后完成。

use xc_notion::{Rect2D, Vec2};

/// 本帧 2D 渲染目标的视口矩形（像素，屏幕坐标）。
///
/// 与相机的关系：相机（`Camera2DHandle` → `Framing2D`）只给**取景**，
/// 视口矩形由本资源在录制时绑定 —— 这正是 P1-2 "相机与 rect 解绑"的落点。
#[derive(Debug, Clone)]
pub struct ViewRect {
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
