// stroke.rs
//
// 2D 描边样式：颜色 + 宽度 + 宽度所属空间；纯数据，与后端无关。

use crate::color::Color32;
use crate::space::MeasureSpace;

/// 2D 描边样式（颜色 + 宽度 + 度量空间）。
///
/// 宽度以当前图元坐标单位为准（对于 xc-content 的 IR 即 pt）；
/// `space` 声明该宽度最终属于**世界**还是**屏幕**空间（默认 `Screen`）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke2D {
    pub color: Color32,
    pub width: f32,
    /// 宽度所属空间；默认 [`MeasureSpace::Screen`]（不随缩放变化）。
    pub space: MeasureSpace,
}

impl Stroke2D {
    /// 两参构造：`space` 取默认 [`MeasureSpace::Screen`]（既有调用点行为不变）。
    pub const fn new(color: Color32, width: f32) -> Self {
        Self {
            color,
            width,
            space: MeasureSpace::Screen,
        }
    }

    /// 显式指定度量空间的构造。
    pub const fn new_in(color: Color32, width: f32, space: MeasureSpace) -> Self {
        Self {
            color,
            width,
            space,
        }
    }

    /// 描边是否会产生可见像素（宽 > 0 且 alpha > 0）。
    pub fn is_visible(&self) -> bool {
        self.width > 0.0 && self.color.a > 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_check() {
        assert!(Stroke2D::new(Color32::BLACK, 1.0).is_visible());
        assert!(!Stroke2D::new(Color32::BLACK, 0.0).is_visible());
        assert!(!Stroke2D::new(Color32::TRANSPARENT, 1.0).is_visible());
        assert!(!Stroke2D::new(Color32::from_rgba(0, 0, 0, 0), 2.0).is_visible());
    }

    /// 默认构造的空间是 `Screen`（保证现有行为不变）。
    #[test]
    fn default_construction_is_screen() {
        let s = Stroke2D::new(Color32::BLACK, 1.0);
        assert_eq!(s.space, MeasureSpace::Screen);
        let w = Stroke2D::new_in(Color32::BLACK, 1.0, MeasureSpace::World);
        assert_eq!(w.space, MeasureSpace::World);
    }
}
