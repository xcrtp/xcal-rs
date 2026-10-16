// primitives.rs — 场景图元对象（newtype 封装）。
//
// **M3-5 newtype 改造**：字段只在理念层定义一次，本层与 IR 层不再各写一份：
//
// ```text
// 理念层 xc-notion      Circle2D { center, radius }      ← 唯一的字段定义
// 场景对象层 xc-object  Circle(pub Circle2D)             ← 本文件（+ Deref）
// 渲染 IR  Shape2D::Circle { center, radius }            ← 由 ToShape2D 从 Circle2D 构造
// ```
//
// 于是"同一个圆两处定义、字段完全相同"的两处真相消失：`Shape2D::Circle` 的扁平
// 字段只是 IR 的**载荷形态**（后端按它匹配，故形状冻结），数值来源只有一处。
//
// `Deref` 到内层 ⇒ `circle.radius` / `line.a` / `rect.center()` 等原有写法不变。

use std::ops::{Deref, DerefMut};

use xc_notion::{Circle2D, Line2D, Path2D, Rect2D, Transform2D, Vec2};
use xc_render_ir::{Shape2D, ToShape2D};

use crate::affine::{map_path_affine, uniform_scale};

// ─── Circle ───────────────────────────────────────────────────────────────

/// 圆图元对象。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Circle(pub Circle2D);

impl Circle {
    /// 构造：`center` 接受 `(f32, f32)` / `Vec2`（`Into<Vec2>` 泛化）。
    pub fn new(center: impl Into<Vec2>, radius: f32) -> Self {
        Self(Circle2D::new(center.into(), radius))
    }

    /// 内层理念层值。
    pub fn as_primitive(&self) -> &Circle2D {
        &self.0
    }
}

impl Deref for Circle {
    type Target = Circle2D;
    fn deref(&self) -> &Circle2D {
        &self.0
    }
}

impl DerefMut for Circle {
    fn deref_mut(&mut self) -> &mut Circle2D {
        &mut self.0
    }
}

impl From<Circle2D> for Circle {
    fn from(v: Circle2D) -> Self {
        Self(v)
    }
}

impl ToShape2D for Circle {
    fn to_shape2d(&self, t: &Transform2D) -> Shape2D {
        // 半径不是"点映射"能得到的量：按统一缩放因子缩放。
        Shape2D::Circle {
            center: t.apply_point(self.0.center),
            radius: self.0.radius * uniform_scale(t),
        }
    }
}

// ─── Path ─────────────────────────────────────────────────────────────────

/// 2D 路径图元对象（直接持有理念层 [`Path2D`]，自带闭合语义）。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path(pub Path2D);

impl Path {
    pub fn new(path: Path2D) -> Self {
        Self(path)
    }

    pub fn as_primitive(&self) -> &Path2D {
        &self.0
    }
}

impl Deref for Path {
    type Target = Path2D;
    fn deref(&self) -> &Path2D {
        &self.0
    }
}

impl DerefMut for Path {
    fn deref_mut(&mut self) -> &mut Path2D {
        &mut self.0
    }
}

impl From<Path2D> for Path {
    fn from(v: Path2D) -> Self {
        Self(v)
    }
}

impl ToShape2D for Path {
    fn to_shape2d(&self, t: &Transform2D) -> Shape2D {
        Shape2D::Path(map_path_affine(&self.0, t))
    }
}

// ─── Line ─────────────────────────────────────────────────────────────────

/// 线段图元对象（两端点独立映射）。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Line(pub Line2D);

impl Line {
    pub fn new(a: impl Into<Vec2>, b: impl Into<Vec2>) -> Self {
        Self(Line2D::new(a.into(), b.into()))
    }

    pub fn as_primitive(&self) -> &Line2D {
        &self.0
    }

    pub fn a(&self) -> Vec2 {
        self.0.a
    }

    pub fn b(&self) -> Vec2 {
        self.0.b
    }
}

impl Deref for Line {
    type Target = Line2D;
    fn deref(&self) -> &Line2D {
        &self.0
    }
}

impl DerefMut for Line {
    fn deref_mut(&mut self) -> &mut Line2D {
        &mut self.0
    }
}

impl From<Line2D> for Line {
    fn from(v: Line2D) -> Self {
        Self(v)
    }
}

impl ToShape2D for Line {
    fn to_shape2d(&self, t: &Transform2D) -> Shape2D {
        Shape2D::Line {
            start: t.apply_point(self.0.a),
            end: t.apply_point(self.0.b),
        }
    }
}

// ─── Rect ─────────────────────────────────────────────────────────────────

/// 轴对齐矩形图元对象。
///
/// 保留 `min()` / `max()` 访问器方法（内层 [`Rect2D`] 是 `min` / `max` **字段**）——
/// 既有调用点 `rect.min()` 因此无需改动。
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect(pub Rect2D);

impl Rect {
    pub fn new(min: impl Into<Vec2>, max: impl Into<Vec2>) -> Self {
        Self(Rect2D::new(min.into(), max.into()))
    }

    pub fn as_primitive(&self) -> &Rect2D {
        &self.0
    }

    pub fn min(&self) -> Vec2 {
        self.0.min
    }

    pub fn max(&self) -> Vec2 {
        self.0.max
    }
}

impl Deref for Rect {
    type Target = Rect2D;
    fn deref(&self) -> &Rect2D {
        &self.0
    }
}

impl DerefMut for Rect {
    fn deref_mut(&mut self) -> &mut Rect2D {
        &mut self.0
    }
}

impl From<Rect2D> for Rect {
    fn from(v: Rect2D) -> Self {
        Self(v)
    }
}

impl ToShape2D for Rect {
    fn to_shape2d(&self, t: &Transform2D) -> Shape2D {
        // 轴对齐矩形在相机仿射（无旋转）下仍是轴对齐矩形：映射两角点后取 min/max。
        let a = t.apply_point(self.0.min);
        let b = t.apply_point(self.0.max);
        Shape2D::Rect(Rect2D::new(a.min(b), a.max(b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_notion::{Mat3, Vec3};

    /// 相机降级矩阵：`diag(s, -s)` + 平移。
    fn camera_like(s: f32, tx: f32, ty: f32) -> Transform2D {
        Transform2D {
            matrix: Mat3::from_cols(
                Vec3::new(s, 0.0, 0.0),
                Vec3::new(0.0, -s, 0.0),
                Vec3::new(tx, ty, 1.0),
            ),
        }
    }

    /// newtype + Deref：内层字段直接可读（原写法不变）。
    #[test]
    fn deref_exposes_inner_fields() {
        let c = Circle::new((1.0, 2.0), 3.0);
        assert_eq!((c.center.x, c.center.y, c.radius), (1.0, 2.0, 3.0));

        let l = Line::new((0.0, 0.0), (3.0, 4.0));
        assert_eq!(l.a(), Vec2::ZERO);
        assert_eq!(l.length(), 5.0, "Line2D::length 经 Deref 可用");

        let r = Rect::new((0.0, 0.0), (10.0, 5.0));
        assert_eq!(r.min(), Vec2::ZERO);
        assert_eq!(r.max(), Vec2::new(10.0, 5.0));
        assert_eq!(r.size(), Vec2::new(10.0, 5.0), "Rect2D::size 经 Deref 可用");
    }

    /// 两处真相消失：`Shape2D::Circle` 的数值来源是理念层 `Circle2D`。
    #[test]
    fn circle_degrades_from_primitive() {
        let c = Circle::new((1.0, 1.0), 2.0);
        match c.to_shape2d(&camera_like(3.0, 100.0, 50.0)) {
            Shape2D::Circle { center, radius } => {
                // (1,1) → (100 + 3, 50 - 3) = (103, 47)；半径 2 × 3 = 6。
                assert_eq!(center, Vec2::new(103.0, 47.0));
                assert_eq!(radius, 6.0);
            }
            other => panic!("expected Circle, got {other:?}"),
        }
    }

    #[test]
    fn line_and_rect_degrade() {
        let l = Line::new((0.0, 0.0), (1.0, 0.0));
        match l.to_shape2d(&camera_like(2.0, 10.0, 20.0)) {
            Shape2D::Line { start, end } => {
                assert_eq!(start, Vec2::new(10.0, 20.0));
                assert_eq!(end, Vec2::new(12.0, 20.0));
            }
            other => panic!("expected Line, got {other:?}"),
        }

        let r = Rect::new((0.0, 0.0), (2.0, 1.0));
        match r.to_shape2d(&camera_like(2.0, 10.0, 20.0)) {
            Shape2D::Rect(rr) => {
                // 两角映射后 (10,20) / (14,18) → 归一化为 min/max。
                assert_eq!(rr.min, Vec2::new(10.0, 18.0));
                assert_eq!(rr.max, Vec2::new(14.0, 20.0));
            }
            other => panic!("expected Rect, got {other:?}"),
        }
    }

    /// `Path` 经 Deref 直接可用 `Path2D` 的方法；降级保留 `Close`。
    #[test]
    fn path_degrades_and_keeps_close() {
        let p = Path::new(Path2D::from_polyline(
            &[Vec2::ZERO, Vec2::new(1.0, 1.0)],
            true,
        ));
        assert_eq!(p.commands.len(), 3, "Path2D::commands 经 Deref 可见");
        match p.to_shape2d(&camera_like(1.0, 0.0, 0.0)) {
            Shape2D::Path(out) => {
                assert_eq!(out.commands.last(), Some(&xc_notion::PathCommand::Close));
            }
            other => panic!("expected Path, got {other:?}"),
        }
    }
}
