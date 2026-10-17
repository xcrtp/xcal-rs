// primitives.rs
//
// 基础图元数据：3D 点/线段（渲染新图元 Line3D/Point3D 的几何载体）、
// 2D 三角形（三角化输出单位，含 2D 有符号面积），以及 M1 T2 补齐的
// 2D 图元值类型（圆 / 椭圆 / 线段 / 折线 / 多边形）。
//
// ⚠️ 离散化产物的判据是**动作归属**而非类型归属：`Polyline2D` / `Polygon2D`
// 的 `points` 是**定义本身**（折线的顶点即其精确表示，与 `Path2D` 的控制点同类）；
// 而"对函数采样 100 点得到的折线"是**离散化产物**——类型仍是 `Polyline2D`。
// 类型在理念层，**采样动作在场景对象层 / 装配层**：故本文件只提供
// 包围盒 / 面积 / 长度 / 包含 / 仿射变换这类精确代数，**不提供 `sample()`**。
//
// `Circle2D` / `Ellipse2D` 不提供 `transformed`：一般仿射变换下圆 / 轴对齐椭圆的
// 像是**带旋转的椭圆**，而 `Ellipse2D` 无旋转字段，无法精确表示（需要旋转时应在
// 场景对象层采样为 `Polygon2D`）。仅提供平移与纯代数。

use crate::color::Color32;
use crate::rect::Rect2D;
use crate::transform::Transform2D;
use crate::vec::{Vec2, Vec3};

/// 3D 点图元：位置 + 颜色。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

// ─── 2D 图元值类型（M1 T2）──────────────────────────────────────────────────
//
// 补齐动机：2D 图元此前一个都没有，导致同一个圆两处定义、字段完全相同
// （`xcal-scene::scene::components::Circle { center, radius }` 与
// `xcal-render` 的 `Shape2D::Circle { center, radius }`）。本组类型为 M3 的
// newtype 改造提供封装目标。
//
// 每个类型只配**理念层该有的代数**：不含样式、不含渲染知识。

/// 2D 圆：中心 + 半径。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Circle2D {
    pub center: Vec2,
    pub radius: f32,
}

impl Circle2D {
    pub fn new(center: Vec2, radius: f32) -> Self {
        Self { center, radius }
    }

    /// 轴对齐包围盒。
    pub fn bounds(&self) -> Rect2D {
        let r = Vec2::splat(self.radius);
        Rect2D::new(self.center - r, self.center + r)
    }

    pub fn area(&self) -> f32 {
        std::f32::consts::PI * self.radius * self.radius
    }

    pub fn circumference(&self) -> f32 {
        std::f32::consts::TAU * self.radius
    }

    /// 点是否落在圆内（含边界）。
    pub fn contains(&self, p: Vec2) -> bool {
        (p - self.center).length_squared() <= self.radius * self.radius
    }

    /// 平移（圆是平移不变族，结果仍是圆）。
    pub fn translated(&self, delta: Vec2) -> Self {
        Self::new(self.center + delta, self.radius)
    }

    /// 等角采样为折线点列（**首尾点不重复**；闭合语义由调用方的 `closed` 表达，
    /// 与 [`Path2D::from_polyline`](crate::Path2D::from_polyline) 同口径）。
    ///
    /// - `n`：段数 ⇒ 点数恒为 `n`；`n = 0` 返回空（不做特判 panic）；
    /// - `start_angle`：首点方向（弧度，`0.0` = +X）；后续点按 `TAU / n` **逆时针**推进。
    ///
    /// 为什么带 `start_angle`：形变 / 描边场景需要"起笔方向对齐"——例如圆→方形按弧长配对时，
    /// 起笔方向不一致会让形变过程整体旋转（见 S1 形变的原型约定）。
    ///
    /// 返回 `Vec<Vec2>` 而非 `Polyline2D`：与 `Path2D::from_polyline(&points, closed)`
    /// 的入参口径一致，调用方自行决定闭合与包装（本方法只管"圆的离散化"这一件事）。
    pub fn sample(&self, n: u32, start_angle: f32) -> Vec<Vec2> {
        if n == 0 {
            return Vec::new();
        }
        let step = std::f32::consts::TAU / n as f32;
        (0..n)
            .map(|i| {
                let a = start_angle + step * i as f32;
                self.center + Vec2::new(a.cos(), a.sin()) * self.radius
            })
            .collect()
    }
}

/// 2D 轴对齐椭圆：中心 + 两个半轴。
///
/// ⚠️ 一般仿射（含非等比缩放 / 旋转）下圆与轴对齐椭圆的像为**带旋转的椭圆**，
/// 本类型无旋转字段，故不提供 `transformed`。
/// **降级路径**：场景层父子变换若含非等比缩放，由渲染侧将圆 / 椭圆采样为
/// [`Polygon2D`] 后变换（精度由采样点数决定，不影响理念层精确性）。
/// 若将来确需精确表示（含旋转字段），须连带改 IR 变体与三角化 —— 属范围扩张，
/// 需单独立项。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Ellipse2D {
    pub center: Vec2,
    pub rx: f32,
    pub ry: f32,
}

impl Ellipse2D {
    pub fn new(center: Vec2, rx: f32, ry: f32) -> Self {
        Self { center, rx, ry }
    }

    /// 轴对齐包围盒。
    pub fn bounds(&self) -> Rect2D {
        Rect2D::new(
            self.center - Vec2::new(self.rx, self.ry),
            self.center + Vec2::new(self.rx, self.ry),
        )
    }

    pub fn area(&self) -> f32 {
        std::f32::consts::PI * self.rx * self.ry
    }

    /// 点是否落在椭圆内（含边界）；半轴为 0 时退化为线段 / 点，不产生 NaN
    /// （`(0/0)` 视作与中心重合才命中）。
    pub fn contains(&self, p: Vec2) -> bool {
        let d = p - self.center;
        let nx = if self.rx != 0.0 { d.x / self.rx } else { 0.0 };
        let ny = if self.ry != 0.0 { d.y / self.ry } else { 0.0 };
        let flat = (self.rx == 0.0 && d.x != 0.0) || (self.ry == 0.0 && d.y != 0.0);
        !flat && nx * nx + ny * ny <= 1.0
    }

    pub fn translated(&self, delta: Vec2) -> Self {
        Self::new(self.center + delta, self.rx, self.ry)
    }
}

/// 2D 线段：两个端点。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Line2D {
    pub a: Vec2,
    pub b: Vec2,
}

impl Line2D {
    pub fn new(a: Vec2, b: Vec2) -> Self {
        Self { a, b }
    }

    pub fn length(&self) -> f32 {
        (self.b - self.a).length()
    }

    pub fn midpoint(&self) -> Vec2 {
        (self.a + self.b) * 0.5
    }

    /// 方向向量 `b - a`（未单位化）。
    pub fn direction(&self) -> Vec2 {
        self.b - self.a
    }

    /// 线性参数取点：`t = 0 → a`，`t = 1 → b`（区间外线性外插，不 clamp）。
    pub fn point_at(&self, t: f32) -> Vec2 {
        self.a + (self.b - self.a) * t
    }

    /// 轴对齐包围盒。
    pub fn bounds(&self) -> Rect2D {
        Rect2D::new(self.a.min(self.b), self.a.max(self.b))
    }

    /// 线段上距 `p` 最近的点（退化线段即端点 `a`）。
    pub fn closest_point(&self, p: Vec2) -> Vec2 {
        let ab = self.b - self.a;
        let len_sq = ab.length_squared();
        if len_sq < f32::EPSILON {
            return self.a;
        }
        let t = ((p - self.a).dot(ab) / len_sq).clamp(0.0, 1.0);
        self.point_at(t)
    }

    /// 点到线段的距离。
    pub fn distance_to_point(&self, p: Vec2) -> f32 {
        (p - self.closest_point(p)).length()
    }

    /// 仿射变换（直线 → 直线，精确保持类型）。
    pub fn transformed(&self, transform: &Transform2D) -> Self {
        Self::new(transform.apply_point(self.a), transform.apply_point(self.b))
    }
}

/// 箭头**杆宽**相对**头宽**的比例（T2）。
///
/// 用户决策的字段表只有 `from` / `to` / `head_length` / `head_width`（见计划 §3.1），
/// 没有独立的杆宽字段 ⇒ 杆宽由此比例导出 —— 避免"为假想需求设计"的额外参数。
pub const ARROW_SHAFT_WIDTH_RATIO: f32 = 0.5;

/// 2D 直线箭头：起点 + 终点 + 头部尺寸。
///
/// **度量空间 = 世界单位**（与 [`Circle2D::radius`] 同口径）：`head_length` / `head_width`
/// 随相机 `scale` 一同缩放；"屏幕恒定粗细的箭头"需另接 [`MeasureSpace`](crate::MeasureSpace)
/// （本轮不做）。
///
/// 头部样式本轮**只做实心三角**（参数里不放样式枚举，将来要扩展时再加字段）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Arrow2D {
    /// 箭尾（起点）。
    pub from: Vec2,
    /// 箭头（终点）。
    pub to: Vec2,
    /// 头部长度（从 `to` 沿轴向回退的距离，世界单位）。
    pub head_length: f32,
    /// 头底宽度（世界单位）。
    pub head_width: f32,
}

impl Arrow2D {
    pub fn new(from: Vec2, to: Vec2, head_length: f32, head_width: f32) -> Self {
        Self {
            from,
            to,
            head_length,
            head_width,
        }
    }

    /// 全长（`from` → `to`）。
    pub fn length(&self) -> f32 {
        (self.to - self.from).length()
    }

    /// 单位方向向量（退化零长 ⇒ [`Vec2::ZERO`]，不 panic）。
    pub fn direction(&self) -> Vec2 {
        (self.to - self.from).normalize_or_zero()
    }

    /// 几何分解：`(杆, 头)`，二者均为**闭合多边形**（逆时针）。
    ///
    /// - 杆 = 从 `from` 到头底的细长矩形，宽 = `head_width ×` [`ARROW_SHAFT_WIDTH_RATIO`]；
    /// - 头 = 以头底为底、`to` 为顶点的三角；`head_length` 被钳到全长以内（不越过起点）。
    ///
    /// 退化（零长 / 头宽或头长为 0）⇒ 返回空多边形（不 panic，由三角化静默丢弃）。
    pub fn parts(&self) -> (Polygon2D, Polygon2D) {
        let d = self.to - self.from;
        let len = d.length();
        if len <= f32::EPSILON {
            return (Polygon2D::new(Vec::new()), Polygon2D::new(Vec::new()));
        }
        let dir = d / len;
        // 左法线（用于给杆 / 头"张开宽度"）。
        let n = Vec2::new(-dir.y, dir.x);
        let half_head = self.head_width.max(0.0) * 0.5;
        let half_shaft = half_head * ARROW_SHAFT_WIDTH_RATIO;
        // 头长不得超过全长（否则头会越过起点）。
        let head_len = self.head_length.clamp(0.0, len);
        let head_base = self.to - dir * head_len;

        let shaft = Polygon2D::new(vec![
            self.from - n * half_shaft,
            head_base - n * half_shaft,
            head_base + n * half_shaft,
            self.from + n * half_shaft,
        ]);
        let head = Polygon2D::new(vec![
            head_base - n * half_head,
            self.to,
            head_base + n * half_head,
        ]);
        (shaft, head)
    }
}

/// 2D 折线：开放或闭合的顶点序列（顶点即精确表示，非采样产物）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Polyline2D {
    pub points: Vec<Vec2>,
}

impl Polyline2D {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_points(points: Vec<Vec2>) -> Self {
        Self { points }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// 顶点数。
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// 折线总长（相邻顶点距离之和）。
    pub fn length(&self) -> f32 {
        self.points.windows(2).map(|w| (w[1] - w[0]).length()).sum()
    }

    /// 包围盒；空折线返回 `None`。
    pub fn bounds(&self) -> Option<Rect2D> {
        points_bounds(&self.points)
    }

    /// 仿射变换（折线 → 折线，精确保持类型）。
    pub fn transformed(&self, transform: &Transform2D) -> Self {
        Self {
            points: self
                .points
                .iter()
                .map(|p| transform.apply_point(*p))
                .collect(),
        }
    }
}

/// 2D 多边形：顶点序列隐式闭合（`points` 不含重复的尾点）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Polygon2D {
    pub points: Vec<Vec2>,
}

impl Polygon2D {
    pub fn new(points: Vec<Vec2>) -> Self {
        Self { points }
    }

    /// 顶点数。
    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// 有符号面积的 2 倍（鞋带公式；正值 = 逆时针 CCW，与 [`Triangle2D::signed_area2`] 同口径）。
    pub fn signed_area2(&self) -> f32 {
        if self.points.len() < 3 {
            return 0.0;
        }
        let mut acc = 0.0;
        for i in 0..self.points.len() {
            let p = self.points[i];
            let q = self.points[(i + 1) % self.points.len()];
            acc += p.perp_dot(q);
        }
        acc
    }

    /// 无符号面积的 2 倍。
    pub fn area2(&self) -> f32 {
        self.signed_area2().abs()
    }

    /// 多边形面积。
    pub fn area(&self) -> f32 {
        self.area2() * 0.5
    }

    /// 顶点是否按逆时针顺序排列。
    pub fn is_ccw(&self) -> bool {
        self.signed_area2() > 0.0
    }

    /// 周长（隐式闭合，含末点 → 首点一段）。
    pub fn perimeter(&self) -> f32 {
        if self.points.len() < 2 {
            return 0.0;
        }
        let mut acc = 0.0;
        for i in 0..self.points.len() {
            let p = self.points[i];
            let q = self.points[(i + 1) % self.points.len()];
            acc += (q - p).length();
        }
        acc
    }

    /// 包围盒；空多边形返回 `None`。
    pub fn bounds(&self) -> Option<Rect2D> {
        points_bounds(&self.points)
    }

    /// 仿射变换（多边形 → 多边形，面积按变换行列式缩放）。
    pub fn transformed(&self, transform: &Transform2D) -> Self {
        Self {
            points: self
                .points
                .iter()
                .map(|p| transform.apply_point(*p))
                .collect(),
        }
    }
}

/// 顶点集的轴对齐包围盒；空集返回 `None`。
fn points_bounds(points: &[Vec2]) -> Option<Rect2D> {
    let mut it = points.iter().copied();
    let first = it.next()?;
    let (mut min, mut max) = (first, first);
    for p in it {
        min = min.min(p);
        max = max.max(p);
    }
    Some(Rect2D::new(min, max))
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

    // ── 2D 图元值类型（M1 T2）──────────────────────────────────────────

    #[test]
    fn test_circle2d_bounds_area_contains() {
        let c = Circle2D::new(Vec2::new(3.0, -1.0), 2.0);
        let b = c.bounds();
        assert_eq!(b.min, Vec2::new(1.0, -3.0));
        assert_eq!(b.max, Vec2::new(5.0, 1.0));
        assert!((c.area() - std::f32::consts::PI * 4.0).abs() < 1e-5);
        assert!((c.circumference() - std::f32::consts::TAU * 2.0).abs() < 1e-5);
        // 内部命中；边界命中；外部不命中。
        assert!(c.contains(Vec2::new(3.0, -1.0)));
        assert!(c.contains(Vec2::new(5.0, -1.0)));
        assert!(!c.contains(Vec2::new(5.01, -1.0)));
    }

    #[test]
    fn test_circle2d_translated_is_still_circle() {
        let c = Circle2D::new(Vec2::new(0.0, 0.0), 1.0);
        assert_eq!(
            c.translated(Vec2::new(2.0, 3.0)),
            Circle2D::new(Vec2::new(2.0, 3.0), 1.0)
        );
    }

    /// `Circle2D::sample`：点数 = 段数、半径守恒、首尾**不重复**（闭合由 `closed` 表达）、
    /// 起点由 `start_angle` 决定、步长恒为 `TAU / n`（逆时针）。
    ///
    /// 该口径与 `xcal-app` 里原手写循环（起笔 `-π/4`、64 段、半径 `1.2`）逐点一致。
    #[test]
    fn test_circle2d_sample_equiangular_without_duplicate_endpoint() {
        let center = Vec2::new(-6.0, -2.5);
        let radius = 1.2;
        let start_angle = -std::f32::consts::FRAC_PI_4;
        let c = Circle2D::new(center, radius);
        let n = 64u32;
        let pts = c.sample(n, start_angle);

        assert_eq!(pts.len(), n as usize, "点数恒等于段数");
        // 半径守恒（含首点与末点）。
        for p in &pts {
            assert!(((*p - center).length() - radius).abs() < 1e-4);
        }
        // 首点 = center + r·(cos start, sin start)。
        let first = center + Vec2::new(start_angle.cos(), start_angle.sin()) * radius;
        assert!((pts[0] - first).length() < 1e-5);
        // 步长 = TAU / n。
        let step = std::f32::consts::TAU / n as f32;
        let second =
            center + Vec2::new((start_angle + step).cos(), (start_angle + step).sin()) * radius;
        assert!((pts[1] - second).length() < 1e-5);
        // 首尾不重复：末点与首点的弦长 = 2r·sin(π/n)（≠ 0 ⇒ 点数恰为 n，
        // 闭合语义由调用方的 `closed` 表达，不由重复端点表达）。
        let chord = 2.0 * radius * (std::f32::consts::PI / n as f32).sin();
        assert!(
            ((pts[0] - pts[(n - 1) as usize]).length() - chord).abs() < 1e-4,
            "末点与首点的弦长应为 2r·sin(π/n)"
        );
    }

    /// 退化段数不 panic：`n = 0` 返回空，`n = 1 / 2` 返回 1 / 2 个点。
    #[test]
    fn test_circle2d_sample_degenerate_segment_counts() {
        let c = Circle2D::new(Vec2::ZERO, 3.0);
        assert!(c.sample(0, 0.0).is_empty());
        assert_eq!(c.sample(1, 0.0).len(), 1);
        assert_eq!(c.sample(2, 0.0).len(), 2);
        assert_eq!(c.sample(1, 0.0)[0], Vec2::new(3.0, 0.0));
    }

    #[test]
    fn test_ellipse2d_contains_boundary_and_axes() {
        let e = Ellipse2D::new(Vec2::new(0.0, 0.0), 4.0, 2.0);
        // 四顶点都在边界上。
        assert!(e.contains(Vec2::new(4.0, 0.0)));
        assert!(e.contains(Vec2::new(-4.0, 0.0)));
        assert!(e.contains(Vec2::new(0.0, 2.0)));
        assert!(e.contains(Vec2::new(0.0, -2.0)));
        // 参数化边界点（3-4-5 类比：(4*0.6, 2*0.8)）。
        assert!(e.contains(Vec2::new(2.4, 1.6)));
        // 明显在外。
        assert!(!e.contains(Vec2::new(3.0, 1.9)));
        assert_eq!(e.bounds().max, Vec2::new(4.0, 2.0));
        assert!((e.area() - std::f32::consts::PI * 8.0).abs() < 1e-5);
    }

    #[test]
    fn test_ellipse2d_degenerate_axis_does_not_nan() {
        // ry = 0 → 退化为 x 轴上的线段。
        let e = Ellipse2D::new(Vec2::ZERO, 2.0, 0.0);
        assert!(e.contains(Vec2::new(1.0, 0.0)));
        assert!(e.contains(Vec2::new(2.0, 0.0)));
        assert!(!e.contains(Vec2::new(1.0, 0.5)));
        assert!(!e.contains(Vec2::new(2.5, 0.0)));
    }

    #[test]
    fn test_line2d_length_midpoint_and_closest_point() {
        let l = Line2D::new(Vec2::new(0.0, 0.0), Vec2::new(3.0, 4.0));
        assert_eq!(l.length(), 5.0);
        assert_eq!(l.midpoint(), Vec2::new(1.5, 2.0));
        assert_eq!(l.direction(), Vec2::new(3.0, 4.0));
        assert_eq!(l.point_at(0.0), Vec2::new(0.0, 0.0));
        assert_eq!(l.point_at(1.0), Vec2::new(3.0, 4.0));
        assert_eq!(l.bounds().min, Vec2::new(0.0, 0.0));
        assert_eq!(l.bounds().max, Vec2::new(3.0, 4.0));

        // 落在中垂线上的点 → 投影到中点，距离 = 到中点的距离。
        let p = Vec2::new(1.5, 2.0) + Vec2::new(-4.0, 3.0); // 垂直方向偏移 5
        assert!((l.closest_point(p) - Vec2::new(1.5, 2.0)).length() < 1e-5);
        assert!((l.distance_to_point(p) - 5.0).abs() < 1e-5);

        // 端点外侧的点 → 投影钳到端点。
        assert_eq!(l.closest_point(Vec2::new(-1.0, 0.0)), Vec2::new(0.0, 0.0));
        assert_eq!(l.closest_point(Vec2::new(6.0, 8.0)), Vec2::new(3.0, 4.0));
    }

    #[test]
    fn test_line2d_degenerate_is_a_point() {
        let l = Line2D::new(Vec2::new(2.0, 2.0), Vec2::new(2.0, 2.0));
        assert_eq!(l.length(), 0.0);
        assert_eq!(l.closest_point(Vec2::new(9.0, 9.0)), Vec2::new(2.0, 2.0));
        assert!((l.distance_to_point(Vec2::new(2.0, 5.0)) - 3.0).abs() < 1e-5);
    }

    #[test]
    fn test_line2d_transformed_scale_and_translate() {
        let l = Line2D::new(Vec2::new(0.0, 0.0), Vec2::new(2.0, 1.0));
        // 先缩放 2 倍，再平移 (1, -1)。
        let t = Transform2D::scale(Vec2::splat(2.0))
            .compose(Transform2D::translate(Vec2::new(1.0, -1.0)));
        let out = l.transformed(&t);
        assert_eq!(out.a, Vec2::new(1.0, -1.0));
        assert_eq!(out.b, Vec2::new(5.0, 1.0));
        // 长度按 2 倍缩放。
        assert!((out.length() - 2.0 * l.length()).abs() < 1e-5);
    }

    #[test]
    fn test_polyline2d_length_and_bounds() {
        let pl = Polyline2D::from_points(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(3.0, 0.0),
            Vec2::new(3.0, 4.0),
        ]);
        assert_eq!(pl.len(), 3);
        assert!(!pl.is_empty());
        assert_eq!(pl.length(), 7.0);
        let b = pl.bounds().unwrap();
        assert_eq!(b.min, Vec2::new(0.0, 0.0));
        assert_eq!(b.max, Vec2::new(3.0, 4.0));
        // 空折线：无包围盒，长度为 0。
        assert!(Polyline2D::new().bounds().is_none());
        assert_eq!(Polyline2D::new().length(), 0.0);
    }

    #[test]
    fn test_polyline2d_transformed_preserves_length_ratio() {
        let pl = Polyline2D::from_points(vec![Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0)]);
        let t = Transform2D::translate(Vec2::new(10.0, 10.0));
        let out = pl.transformed(&t);
        assert_eq!(out.points[1], Vec2::new(11.0, 10.0));
        assert!((out.length() - pl.length()).abs() < 1e-6);
    }

    #[test]
    fn test_polygon2d_signed_area_sign_and_value() {
        // CCW 单位正方形 → signed_area2 = +2，area = 1。
        let ccw = Polygon2D::new(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ]);
        assert!(ccw.is_ccw());
        assert_eq!(ccw.signed_area2(), 2.0);
        assert_eq!(ccw.area(), 1.0);
        assert_eq!(ccw.perimeter(), 4.0);

        // 反向绕序 → 符号取反，面积不变。
        let cw = Polygon2D::new(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(0.0, 1.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 0.0),
        ]);
        assert!(!cw.is_ccw());
        assert_eq!(cw.signed_area2(), -2.0);
        assert_eq!(cw.area(), 1.0);
    }

    #[test]
    fn test_polygon2d_degenerate_and_bounds() {
        // 顶点不足 3：面积为 0，不 panic。
        assert_eq!(Polygon2D::new(vec![Vec2::ZERO]).signed_area2(), 0.0);
        assert_eq!(Polygon2D::new(vec![]).area(), 0.0);
        assert!(Polygon2D::new(vec![]).bounds().is_none());

        let tri = Polygon2D::new(vec![
            Vec2::new(-1.0, -2.0),
            Vec2::new(3.0, -2.0),
            Vec2::new(0.0, 5.0),
        ]);
        let b = tri.bounds().unwrap();
        assert_eq!(b.min, Vec2::new(-1.0, -2.0));
        assert_eq!(b.max, Vec2::new(3.0, 5.0));
    }

    #[test]
    fn test_polygon2d_transformed_scales_area_by_determinant() {
        let sq = Polygon2D::new(vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ]);
        // 非等比缩放 (2, 3)：面积 ×6（行列式）。
        let t = Transform2D::scale(Vec2::new(2.0, 3.0));
        let out = sq.transformed(&t);
        assert!((out.area() - 6.0).abs() < 1e-5);
        assert!(out.is_ccw(), "非负行列式不翻转绕序");

        // 镜像（行列式 < 0）→ 绕序翻转。
        let flip = Transform2D::scale(Vec2::new(-1.0, 1.0));
        assert!(!sq.transformed(&flip).is_ccw());
    }

    // ── 箭头（T2）──────────────────────────────────────────────────────

    /// 杆是矩形（4 顶点）、头是三角（3 顶点），且两极都落在 `from` / `to`。
    #[test]
    fn arrow2d_parts_are_quad_and_triangle() {
        let a = Arrow2D::new(Vec2::ZERO, Vec2::new(4.0, 0.0), 1.0, 1.0);
        let (shaft, head) = a.parts();
        assert_eq!(shaft.len(), 4, "杆 = 细长矩形");
        assert_eq!(head.len(), 3, "头 = 三角");
        // 杆起点 = from；头顶点 = to。
        assert!((shaft.points[0].x - 0.0).abs() < 1e-6);
        assert!((head.points.iter().find(|p| p.x == 4.0).unwrap().x - 4.0).abs() < 1e-6);
        // 头底在 `to - dir*head_length` = x = 3。
        let base_x = head.points[0].x;
        assert!((base_x - 3.0).abs() < 1e-6, "头底 x = {base_x}");
        // 头宽 > 杆宽（判据：头部确实张开）。
        let head_w = (head.points[0].y - head.points[2].y).abs();
        let shaft_w = (shaft.points[0].y - shaft.points[2].y).abs();
        assert!(head_w > shaft_w, "头宽 {head_w} 应大于杆宽 {shaft_w}");
        assert!(shaft.is_ccw() && head.is_ccw(), "两极应为逆时针");
    }

    /// 退化：零长 ⇒ 空多边形（不 panic）。
    #[test]
    fn arrow2d_degenerate_is_empty() {
        let a = Arrow2D::new(Vec2::new(1.0, 1.0), Vec2::new(1.0, 1.0), 1.0, 1.0);
        let (shaft, head) = a.parts();
        assert!(shaft.is_empty() && head.is_empty());
        assert_eq!(a.length(), 0.0);
        assert_eq!(a.direction(), Vec2::ZERO);
    }

    /// 头长超过全长 ⇒ 钳到全长（头底回到起点，不越过）。
    #[test]
    fn arrow2d_head_length_is_clamped_to_total_length() {
        let a = Arrow2D::new(Vec2::ZERO, Vec2::new(1.0, 0.0), 99.0, 1.0);
        let (_, head) = a.parts();
        assert!(
            (head.points[0].x - 0.0).abs() < 1e-6,
            "头底应被钳到起点，实际 x = {}",
            head.points[0].x
        );
    }
}
