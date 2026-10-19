// rect.rs
//
// 2D 轴对齐矩形：min/max 角点 + 包含/相交算法（裁剪与命中测试用）。

use crate::vec::Vec2;

/// 2D 轴对齐矩形（AABB），由 `min`（左下）与 `max`（右上）角点定义。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect2D {
    pub min: Vec2,
    pub max: Vec2,
}

impl Rect2D {
    pub fn new(min: Vec2, max: Vec2) -> Self {
        Self { min, max }
    }

    /// 以中心点与半尺寸构造。
    pub fn from_center(center: Vec2, half_size: Vec2) -> Self {
        Self {
            min: center - half_size,
            max: center + half_size,
        }
    }

    /// 以左上角 + 宽高构造（屏幕坐标习惯：x 向右、y 向下）。
    pub fn from_min_size(min: Vec2, size: Vec2) -> Self {
        Self {
            min,
            max: min + size,
        }
    }

    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }

    /// 尺寸（max - min）；若构造时角点反向，结果可能为负。
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }

    /// 点是否在矩形内（含边界）。
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }

    /// 与另一矩形的交集；不相交时返回 `None`。
    pub fn intersect(&self, other: Rect2D) -> Option<Rect2D> {
        let min = self.min.max(other.min);
        let max = self.max.min(other.max);
        if min.x <= max.x && min.y <= max.y {
            Some(Rect2D { min, max })
        } else {
            None
        }
    }

    /// 是否与另一矩形相交（含边接触）。
    pub fn intersects(&self, other: Rect2D) -> bool {
        self.intersect(other).is_some()
    }

    /// 与另一矩形的并集（同时包含两者的最小 AABB）。
    pub fn union(&self, other: &Rect2D) -> Rect2D {
        Rect2D {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect2D {
        Rect2D::new(Vec2::new(x0, y0), Vec2::new(x1, y1))
    }

    #[test]
    fn test_center_and_size() {
        let rect = r(0.0, 0.0, 10.0, 20.0);
        assert_eq!(rect.center(), Vec2::new(5.0, 10.0));
        assert_eq!(rect.size(), Vec2::new(10.0, 20.0));
    }

    #[test]
    fn test_from_center() {
        let rect = Rect2D::from_center(Vec2::new(0.0, 0.0), Vec2::new(5.0, 10.0));
        assert_eq!(rect.min, Vec2::new(-5.0, -10.0));
        assert_eq!(rect.max, Vec2::new(5.0, 10.0));
    }

    #[test]
    fn test_contains() {
        let rect = r(0.0, 0.0, 10.0, 10.0);
        assert!(rect.contains(Vec2::new(5.0, 5.0)));
        assert!(rect.contains(Vec2::new(0.0, 0.0)));
        assert!(!rect.contains(Vec2::new(-0.1, 5.0)));
        assert!(!rect.contains(Vec2::new(10.1, 5.0)));
    }

    #[test]
    fn test_intersect_overlap() {
        let a = r(0.0, 0.0, 10.0, 10.0);
        let b = r(5.0, 5.0, 15.0, 15.0);
        assert_eq!(a.intersect(b), Some(r(5.0, 5.0, 10.0, 10.0)));
        assert!(a.intersects(b));
    }

    #[test]
    fn test_intersect_disjoint() {
        let a = r(0.0, 0.0, 10.0, 10.0);
        let b = r(20.0, 20.0, 30.0, 30.0);
        assert_eq!(a.intersect(b), None);
        assert!(!a.intersects(b));
    }

    #[test]
    fn test_intersect_edge_touch() {
        let a = r(0.0, 0.0, 10.0, 10.0);
        let b = r(10.0, 0.0, 20.0, 10.0);
        // 边接触视为相交（退化矩形）。
        assert!(a.intersects(b));
    }
}
