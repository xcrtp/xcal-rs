// primitives.rs
//
// 基础图元数据：3D 点/线段（渲染新图元 Line3D/Point3D 的几何载体）与
// 2D 三角形（三角化输出单位，含 2D 有符号面积）。

use crate::color::Color32;
use crate::vec::{Vec2, Vec3};

/// 3D 点图元：位置 + 颜色。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point3D {
    pub position: Vec3,
    pub color: Color32,
}

impl Point3D {
    pub fn new(position: Vec3, color: Color32) -> Self {
        Self { position, color }
    }
}

/// 3D 线段图元：两个端点 + 颜色。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Line3D {
    pub start: Vec3,
    pub end: Vec3,
    pub color: Color32,
}

impl Line3D {
    pub fn new(start: Vec3, end: Vec3, color: Color32) -> Self {
        Self { start, end, color }
    }

    /// 线段中点。
    pub fn midpoint(&self) -> Vec3 {
        (self.start + self.end) * 0.5
    }

    /// 线段长度。
    pub fn length(&self) -> f32 {
        (self.end - self.start).length()
    }
}

/// 2D 三角形（三角化输出的基本单位）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Triangle2D {
    pub a: Vec2,
    pub b: Vec2,
    pub c: Vec2,
}

impl Triangle2D {
    pub fn new(a: Vec2, b: Vec2, c: Vec2) -> Self {
        Self { a, b, c }
    }

    /// 有符号面积的 2 倍（正值 = 逆时针 CCW，负值 = 顺时针 CW）。
    ///
    /// 计算：(b - a) × (c - a) 的 z 分量（perp_dot）。
    pub fn signed_area2(&self) -> f32 {
        (self.b - self.a).perp_dot(self.c - self.a)
    }

    /// 无符号面积的 2 倍（光栅化/面积统计用）。
    pub fn area2(&self) -> f32 {
        self.signed_area2().abs()
    }

    /// 三角形面积。
    pub fn area(&self) -> f32 {
        self.area2() * 0.5
    }

    /// 顶点是否按逆时针顺序排列。
    pub fn is_ccw(&self) -> bool {
        self.signed_area2() > 0.0
    }

    /// 返回翻转绕序后的三角形。
    pub fn flipped(&self) -> Self {
        Self {
            a: self.a,
            b: self.c,
            c: self.b,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_point3d_new() {
        let p = Point3D::new(Vec3::new(1.0, 2.0, 3.0), Color32::RED);
        assert_eq!(p.position, Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(p.color, Color32::RED);
    }

    #[test]
    fn test_line3d_midpoint_and_length() {
        let l = Line3D::new(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Color32::WHITE,
        );
        assert_eq!(l.midpoint(), Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(l.length(), 2.0);
    }

    #[test]
    fn test_triangle_signed_area_ccw() {
        // CCW 三角形：(0,0) (1,0) (0,1) → 面积 0.5，signed_area2 = +1。
        let tri = Triangle2D::new(
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 1.0),
        );
        assert!(tri.is_ccw());
        assert_eq!(tri.signed_area2(), 1.0);
        assert_eq!(tri.area(), 0.5);

        // 翻转绕序后符号取反。
        let cw = tri.flipped();
        assert!(!cw.is_ccw());
        assert_eq!(cw.signed_area2(), -1.0);
        assert_eq!(cw.area2(), 1.0);
    }

    #[test]
    fn test_degenerate_triangle_has_zero_area() {
        let tri = Triangle2D::new(
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(2.0, 2.0),
        );
        assert_eq!(tri.area(), 0.0);
    }
}
