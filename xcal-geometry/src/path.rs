// path.rs
//
// 2D 路径数据：PathCommand（移动/直线/二次/三次贝塞尔/弧/闭合）+ Path2D。
// 纯数据结构 + flatten（曲线采样为折线）与 bounds 计算，无后端逻辑。

use crate::rect::Rect2D;
use crate::vec::Vec2;

/// 路径绘制命令。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCommand {
    /// 抬笔移动到点（开始新子路径）。
    MoveTo(Vec2),
    /// 直线到点。
    LineTo(Vec2),
    /// 二次贝塞尔曲线。
    QuadTo {
        ctrl: Vec2,
        end: Vec2,
    },
    /// 三次贝塞尔曲线。
    CubicTo {
        ctrl1: Vec2,
        ctrl2: Vec2,
        end: Vec2,
    },
    /// 椭圆弧：中心 + 半径 + 旋转角 + 起止角（弧度）。
    Arc {
        center: Vec2,
        radii: Vec2,
        rotation: f32,
        start_angle: f32,
        end_angle: f32,
    },
    /// 闭合当前子路径（回到子路径起点）。
    Close,
}

/// 2D 路径：命令序列 + 懒计算包围盒缓存。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path2D {
    pub commands: Vec<PathCommand>,
    /// 懒计算缓存（由 [`Path2D::bounds`] 填充）。
    pub bounds: Option<Rect2D>,
}

impl Path2D {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            commands: Vec::with_capacity(capacity),
            bounds: None,
        }
    }

    pub fn move_to(&mut self, p: Vec2) -> &mut Self {
        self.commands.push(PathCommand::MoveTo(p));
        self.bounds = None;
        self
    }

    pub fn line_to(&mut self, p: Vec2) -> &mut Self {
        self.commands.push(PathCommand::LineTo(p));
        self.bounds = None;
        self
    }

    pub fn quad_to(&mut self, ctrl: Vec2, end: Vec2) -> &mut Self {
        self.commands.push(PathCommand::QuadTo { ctrl, end });
        self.bounds = None;
        self
    }

    pub fn cubic_to(&mut self, ctrl1: Vec2, ctrl2: Vec2, end: Vec2) -> &mut Self {
        self.commands
            .push(PathCommand::CubicTo { ctrl1, ctrl2, end });
        self.bounds = None;
        self
    }

    pub fn arc_to(
        &mut self,
        center: Vec2,
        radii: Vec2,
        rotation: f32,
        start_angle: f32,
        end_angle: f32,
    ) -> &mut Self {
        self.commands.push(PathCommand::Arc {
            center,
            radii,
            rotation,
            start_angle,
            end_angle,
        });
        self.bounds = None;
        self
    }

    pub fn close(&mut self) -> &mut Self {
        self.commands.push(PathCommand::Close);
        self
    }

    /// 从折线点序列构造路径；`closed` 时追加闭合命令。
    pub fn from_polyline(points: &[Vec2], closed: bool) -> Self {
        let mut path = Self::with_capacity(points.len() + 1);
        if let Some(first) = points.first() {
            path.move_to(*first);
            for p in &points[1..] {
                path.line_to(*p);
            }
            if closed {
                path.close();
            }
        }
        path
    }

    /// 把曲线采样为折线点序列。
    ///
    /// - `MoveTo` 开始新段（其点加入结果）；
    /// - `LineTo` 直接追加端点；
    /// - `QuadTo`/`CubicTo` 用 de Casteljau 自适应细分（平直度 ≤ `tolerance`）；
    /// - `Arc` 按弦误差 ≤ `tolerance` 分弧段；
    /// - `Close` 追加当前子路径起点。
    pub fn flatten(&self, tolerance: f32) -> Vec<Vec2> {
        let mut out = Vec::new();
        let mut sub_start: Option<Vec2> = None;
        let mut current: Option<Vec2> = None;

        for cmd in &self.commands {
            match *cmd {
                PathCommand::MoveTo(p) => {
                    out.push(p);
                    sub_start = Some(p);
                    current = Some(p);
                }
                PathCommand::LineTo(p) => {
                    out.push(p);
                    current = Some(p);
                }
                PathCommand::QuadTo { ctrl, end } => {
                    let start = current.unwrap_or(end);
                    flatten_quad(start, ctrl, end, tolerance.max(1e-4), &mut out);
                    current = Some(end);
                }
                PathCommand::CubicTo { ctrl1, ctrl2, end } => {
                    let start = current.unwrap_or(end);
                    flatten_cubic(start, ctrl1, ctrl2, end, tolerance.max(1e-4), &mut out);
                    current = Some(end);
                }
                PathCommand::Arc {
                    center,
                    radii,
                    rotation,
                    start_angle,
                    end_angle,
                } => {
                    flatten_arc(
                        center,
                        radii,
                        rotation,
                        start_angle,
                        end_angle,
                        tolerance.max(1e-4),
                        &mut out,
                    );
                    // 弧的终点作为当前点。
                    current = Some(arc_point(center, radii, rotation, end_angle));
                    if sub_start.is_none() {
                        sub_start = Some(arc_point(center, radii, rotation, start_angle));
                    }
                }
                PathCommand::Close => {
                    if let Some(start) = sub_start {
                        out.push(start);
                        current = Some(start);
                    }
                }
            }
        }
        out
    }

    /// 计算包围盒（不修改 self，无缓存）。
    ///
    /// 贝塞尔曲线完全位于控制点凸包内，故用控制点 + 端点求 AABB 即可保守包围；
    /// 弧用其参数角实际扫过的范围计算（轴对齐椭圆近似取 radii 分量）。
    /// 空路径返回 `None`。
    pub fn compute_bounds(&self) -> Option<Rect2D> {
        let mut min = Vec2::splat(f32::INFINITY);
        let mut max = Vec2::splat(f32::NEG_INFINITY);
        let mut include = |p: Vec2| {
            min = min.min(p);
            max = max.max(p);
        };

        for cmd in &self.commands {
            match *cmd {
                PathCommand::MoveTo(p) | PathCommand::LineTo(p) => include(p),
                PathCommand::QuadTo { ctrl, end } => {
                    include(ctrl);
                    include(end);
                }
                PathCommand::CubicTo { ctrl1, ctrl2, end } => {
                    include(ctrl1);
                    include(ctrl2);
                    include(end);
                }
                PathCommand::Arc {
                    center,
                    radii,
                    ..
                } => {
                    // 保守包围：中心 ± 半径分量。
                    include(center - radii);
                    include(center + radii);
                }
                PathCommand::Close => {}
            }
        }

        if min.x.is_finite() {
            Some(Rect2D::new(min, max))
        } else {
            None
        }
    }

    /// 计算包围盒并缓存（懒计算；编辑路径后缓存自动失效）。
    ///
    /// 空路径返回 [`Rect2D::default`]。
    pub fn bounds(&mut self) -> Rect2D {
        if let Some(b) = self.bounds {
            return b;
        }
        let bounds = self.compute_bounds().unwrap_or_default();
        self.bounds = Some(bounds);
        bounds
    }
}

// ─── 采样算法 ────────────────────────────────────────────────────────────────

fn arc_point(center: Vec2, radii: Vec2, rotation: f32, angle: f32) -> Vec2 {
    let (sin, cos) = rotation.sin_cos();
    let local = Vec2::new(radii.x * angle.cos(), radii.y * angle.sin());
    Vec2::new(
        center.x + cos * local.x - sin * local.y,
        center.y + sin * local.x + cos * local.y,
    )
}

fn flatten_arc(
    center: Vec2,
    radii: Vec2,
    rotation: f32,
    start: f32,
    end: f32,
    tolerance: f32,
    out: &mut Vec<Vec2>,
) {
    let delta = end - start;
    if delta.abs() < f32::EPSILON {
        return;
    }
    let max_radius = radii.x.max(radii.y).max(1e-4);
    // 弧弦误差（sagitta）≈ r * (1 - cos(step/2)) ≤ tolerance，
    // 解出每步角度上限：step ≤ 2 * acos(1 - tol/r)。
    let cos_half = (1.0 - tolerance / max_radius).clamp(0.0, 1.0);
    let step = 2.0 * cos_half.acos();
    let step = step.clamp(1e-3, std::f32::consts::PI / 8.0);
    let n = ((delta.abs() / step).ceil() as usize).max(1);
    for i in 1..=n {
        let angle = start + delta * (i as f32 / n as f32);
        out.push(arc_point(center, radii, rotation, angle));
    }
}

/// 点到线段距离。
fn point_segment_dist(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq < f32::EPSILON {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

fn flatten_quad(p0: Vec2, p1: Vec2, p2: Vec2, tol: f32, out: &mut Vec<Vec2>) {
    // 控制点离弦的最大距离作为平直度度量。
    let flat = point_segment_dist(p1, p0, p2);
    if flat <= tol {
        out.push(p2);
        return;
    }
    // de Casteljau 一次细分。
    let p01 = (p0 + p1) * 0.5;
    let p12 = (p1 + p2) * 0.5;
    let p012 = (p01 + p12) * 0.5;
    flatten_quad(p0, p01, p012, tol, out);
    flatten_quad(p012, p12, p2, tol, out);
}

fn flatten_cubic(p0: Vec2, p1: Vec2, p2: Vec2, p3: Vec2, tol: f32, out: &mut Vec<Vec2>) {
    let flat = point_segment_dist(p1, p0, p3).max(point_segment_dist(p2, p0, p3));
    if flat <= tol {
        out.push(p3);
        return;
    }
    let p01 = (p0 + p1) * 0.5;
    let p12 = (p1 + p2) * 0.5;
    let p23 = (p2 + p3) * 0.5;
    let p012 = (p01 + p12) * 0.5;
    let p123 = (p12 + p23) * 0.5;
    let p0123 = (p012 + p123) * 0.5;
    flatten_cubic(p0, p01, p012, p0123, tol, out);
    flatten_cubic(p0123, p123, p23, p3, tol, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_polyline_and_close() {
        let path = Path2D::from_polyline(
            &[Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), Vec2::new(1.0, 1.0)],
            true,
        );
        let pts = path.flatten(0.1);
        assert_eq!(*pts.first().unwrap(), Vec2::new(0.0, 0.0));
        assert_eq!(*pts.last().unwrap(), Vec2::new(0.0, 0.0)); // Close 回到起点
    }

    #[test]
    fn test_flatten_line_only() {
        let mut path = Path2D::new();
        path.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(10.0, 0.0));
        let pts = path.flatten(0.5);
        assert_eq!(pts.len(), 2);
    }

    #[test]
    fn test_flatten_quad_produces_intermediate_points() {
        let mut path = Path2D::new();
        path.move_to(Vec2::new(0.0, 0.0))
            .quad_to(Vec2::new(5.0, 10.0), Vec2::new(10.0, 0.0));
        let coarse = path.flatten(1.0);
        let fine = path.flatten(0.01);
        assert!(fine.len() > coarse.len());
        assert!(fine.len() > 2);
        // 终点一致。
        assert_eq!(*fine.last().unwrap(), Vec2::new(10.0, 0.0));
    }

    #[test]
    fn test_flatten_cubic_endpoint() {
        let mut path = Path2D::new();
        path.move_to(Vec2::new(0.0, 0.0)).cubic_to(
            Vec2::new(1.0, 5.0),
            Vec2::new(9.0, 5.0),
            Vec2::new(10.0, 0.0),
        );
        let pts = path.flatten(0.05);
        assert_eq!(*pts.last().unwrap(), Vec2::new(10.0, 0.0));
        assert!(pts.len() > 2);
    }

    #[test]
    fn test_flatten_arc_samples_quadrants() {
        let mut path = Path2D::new();
        path.move_to(Vec2::new(1.0, 0.0)).arc_to(
            Vec2::ZERO,
            Vec2::new(1.0, 1.0),
            0.0,
            0.0,
            std::f32::consts::FRAC_PI_2,
        );
        let pts = path.flatten(0.05);
        assert!(pts.len() >= 3);
        // 终点应为 (0, 1)。
        let last = *pts.last().unwrap();
        assert!((last.x - 0.0).abs() < 1e-4);
        assert!((last.y - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_bounds_cached() {
        let mut path = Path2D::new();
        path.move_to(Vec2::new(-1.0, -2.0))
            .line_to(Vec2::new(3.0, 4.0));
        let b = path.bounds();
        assert_eq!(b.min, Vec2::new(-1.0, -2.0));
        assert_eq!(b.max, Vec2::new(3.0, 4.0));
        assert!(path.bounds.is_some());
    }

    #[test]
    fn test_bounds_invalidates_on_edit() {
        let mut path = Path2D::new();
        path.move_to(Vec2::new(0.0, 0.0));
        let _ = path.bounds();
        assert!(path.bounds.is_some());
        path.line_to(Vec2::new(5.0, 5.0));
        assert!(path.bounds.is_none());
        let b = path.bounds();
        assert_eq!(b.max, Vec2::new(5.0, 5.0));
    }
}
