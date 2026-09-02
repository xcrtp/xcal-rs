// viewport.rs
//
// 渲染视口：目标表面上的一块矩形区域（物理像素）。纯数据，无 GPU 依赖。

/// 渲染视口：目标表面上的一块矩形区域（物理像素）。
///
/// # Example
/// ```
/// use xcal_render::Viewport;
///
/// let vp = Viewport::full(1920, 1080);
/// assert_eq!((vp.x, vp.y, vp.w, vp.h), (0, 0, 1920, 1080));
/// assert!((vp.aspect() - 16.0 / 9.0).abs() < 1e-6);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Viewport {
    /// 左上角 x（物理像素）。
    pub x: u32,
    /// 左上角 y（物理像素）。
    pub y: u32,
    /// 宽度（物理像素）。
    pub w: u32,
    /// 高度（物理像素）。
    pub h: u32,
}

impl Viewport {
    pub fn new(x: u32, y: u32, w: u32, h: u32) -> Self {
        Self { x, y, w, h }
    }

    /// 覆盖整个目标表面（左上角原点）。
    pub fn full(w: u32, h: u32) -> Self {
        Self { x: 0, y: 0, w, h }
    }

    /// 宽高比 w / h（h 为 0 时回退为 1，避免除零）。
    pub fn aspect(&self) -> f32 {
        self.w as f32 / self.h.max(1) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_viewport_full_covers_surface() {
        let vp = Viewport::full(1920, 1080);
        assert_eq!(vp.x, 0);
        assert_eq!(vp.y, 0);
        assert_eq!(vp.w, 1920);
        assert_eq!(vp.h, 1080);
    }

    #[test]
    fn test_viewport_aspect() {
        let vp = Viewport::full(1600, 900);
        assert!((vp.aspect() - 16.0 / 9.0).abs() < 1e-6);
    }

    #[test]
    fn test_viewport_aspect_zero_height_no_panic() {
        let vp = Viewport::new(0, 0, 100, 0);
        assert_eq!(vp.aspect(), 100.0);
    }
}
