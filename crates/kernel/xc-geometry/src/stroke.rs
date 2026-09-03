// stroke.rs
//
// 2D 描边样式：颜色 + 宽度；纯数据，与后端无关。

use crate::color::Color32;

/// 2D 描边样式（颜色 + 宽度）。
///
/// 宽度以当前图元坐标单位为准（对于 xc-content 的 IR 即 pt）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stroke2D {
    pub color: Color32,
    pub width: f32,
}

impl Stroke2D {
    pub const fn new(color: Color32, width: f32) -> Self {
        Self { color, width }
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
}
