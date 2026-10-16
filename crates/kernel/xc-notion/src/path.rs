// path.rs
//
// 2D 路径数据：PathCommand（移动/直线/二次/三次贝塞尔/弧/闭合）+ Path2D。
// 纯数据结构 + flatten（曲线采样为折线）与 bounds 计算，无后端逻辑。
//
// 3D 路径：Path3D（M1 T3 新建）—— 承载 `ParameterCurveT2XYZ` 采样后的
// `Vec<Vec3>`，此前全仓零命中，是真缺口。

use crate::rect::Rect2D;
use crate::transform::{Transform2D, Transform3D};
use crate::vec::{Vec2, Vec3};

/// 路径绘制命令。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathCommand {
    /// 抬笔移动到点（开始新子路径）。
    MoveTo(Vec2),
    /// 直线到点。
    LineTo(Vec2),
    /// 二次贝塞尔曲线。
    QuadTo { ctrl: Vec2, end: Vec2 },
    /// 三次贝塞尔曲线。
    CubicTo { ctrl1: Vec2, ctrl2: Vec2, end: Vec2 },
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
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path2D {
    pub commands: Vec<PathCommand>,
    /// 懒计算缓存（由 [`Path2D::bounds`] 填充）。
    pub bounds: Option<Rect2D>,
}

/// 形变采样的默认顶点数。恒定输出点数 = 稳定开销（与输入路径复杂度无关）。
pub const DEFAULT_MORPH_SAMPLES: usize = 128;
/// 形变重采样时曲线展平的容差。
pub const MORPH_TOLERANCE: f32 = 0.05;

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

    /// 把曲线采样为**按子路径分组**的折线点序列。
    ///
    /// 与 [`flatten`] 的区别：`flatten` 把所有子路径首尾相接拼成一个扁平
    /// `Vec<Vec2>`（丢掉了子路径边界），多轮廓字形（`o`/`B`/`8` 有内圈或孔洞）
    /// 若用扁平结果逐段连线，会在轮廓之间画出横穿字形内部的**多余桥接线**。
    /// 本方法按 `MoveTo` 分组返回，保留每个子轮廓的独立性。
    ///
    /// 每个子路径组内，`Close` 会把当前子路径起点追加到组尾
    /// （即组内首尾点相同，消费端据此判定闭合）。
    pub fn flatten_subpaths(&self, tolerance: f32) -> Vec<Vec<Vec2>> {
        let tol = tolerance.max(1e-4);
        let mut out: Vec<Vec<Vec2>> = Vec::new();
        let mut current_group: Option<Vec<Vec2>> = None;
        let mut sub_start: Option<Vec2> = None;
        let mut current: Option<Vec2> = None;

        for cmd in &self.commands {
            match *cmd {
                PathCommand::MoveTo(p) => {
                    // 上一个组收尾，开始新组。
                    if let Some(g) = current_group.take() {
                        if !g.is_empty() {
                            out.push(g);
                        }
                    }
                    current_group = Some(vec![p]);
                    sub_start = Some(p);
                    current = Some(p);
                }
                PathCommand::LineTo(p) => {
                    current_group.get_or_insert_with(Vec::new).push(p);
                    current = Some(p);
                }
                PathCommand::QuadTo { ctrl, end } => {
                    let start = current.unwrap_or(end);
                    flatten_quad(start, ctrl, end, tol, current_group.get_or_insert_with(Vec::new));
                    current = Some(end);
                }
                PathCommand::CubicTo { ctrl1, ctrl2, end } => {
                    let start = current.unwrap_or(end);
                    flatten_cubic(
                        start,
                        ctrl1,
                        ctrl2,
                        end,
                        tol,
                        current_group.get_or_insert_with(Vec::new),
                    );
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
                        tol,
                        current_group.get_or_insert_with(Vec::new),
                    );
                    // 弧的终点作为当前点。
                    current = Some(arc_point(center, radii, rotation, end_angle));
                    if sub_start.is_none() {
                        sub_start = Some(arc_point(center, radii, rotation, start_angle));
                    }
                }
                PathCommand::Close => {
                    if let (Some(start), Some(g)) = (sub_start, current_group.as_mut()) {
                        g.push(start);
                        current = Some(start);
                    }
                }
            }
        }
        if let Some(g) = current_group.take() {
            if !g.is_empty() {
                out.push(g);
            }
        }
        out
    }

    /// 累积弧长表（按 `flatten` 后的折线计算）。
    ///
    /// 返回长度与折线点数相同，首元素恒为 `0.0`；`closed = true` 时若首尾点
    /// 不重合，会在末尾补上起点（闭合段计入总长）。
    pub fn arc_lengths(&self, tolerance: f32, closed: bool) -> Vec<f32> {
        let pts = self.flatten_closed_points(tolerance, closed);
        cumulative_lengths(&pts)
    }

    /// 按弧长均匀重采样为 `n` 个点——形变对齐的基础。
    ///
    /// `flatten` 输出的点沿曲线分布不均（曲率大处更密）；按索引配对会让
    /// 直线段被拉伸、曲线段被压缩，形变过程视觉上"抽动"。本方法把点变成
    /// 沿周长/长度**均匀分布**，配对才有意义。
    ///
    /// - `closed = true`：沿闭合环取 `n` 个点（首点即起点，不含重复的尾点）；
    /// - `closed = false`：沿开放折线取 `n` 个点，含两端点（`n == 1` 时取起点）；
    /// - 空路径返回空；单点路径或零长路径返回 `n` 个相同点（不 panic）。
    pub fn resample_arc_length(&self, n: usize, tolerance: f32, closed: bool) -> Vec<Vec2> {
        if n == 0 {
            return Vec::new();
        }
        let pts = self.flatten_closed_points(tolerance, closed);
        if pts.is_empty() {
            return Vec::new();
        }
        if pts.len() == 1 {
            return vec![pts[0]; n];
        }
        let cum = cumulative_lengths(&pts);
        let total = *cum.last().unwrap();
        if total <= 1e-9 {
            // 退化（所有点重合）：返回同一点，形状有意义但无长度。
            return vec![pts[0]; n];
        }

        let mut out = Vec::with_capacity(n);
        let mut j = 0usize; // 段索引游标：target 单调不减 → 单遍推进
        for i in 0..n {
            let target = if closed {
                total * (i as f32) / (n as f32)
            } else if n == 1 {
                0.0
            } else {
                total * (i as f32) / ((n as f32) - 1.0)
            };
            while j + 2 < pts.len() && cum[j + 1] < target {
                j += 1;
            }
            let (a, b) = (pts[j], pts[j + 1]);
            let seg = cum[j + 1] - cum[j];
            let local = if seg > 1e-9 {
                ((target - cum[j]) / seg).clamp(0.0, 1.0)
            } else {
                0.0
            };
            out.push(a + (b - a) * local);
        }
        out
    }

    /// 形变后的顶点列表：按弧长对齐后逐点插值。
    ///
    /// 长度恒为 `samples`（闭合环，**不含**重复的尾点）——这是"稳定开销"的
    /// 核心保证：与输入路径复杂度无关。任一侧为空时退化为另一侧（仍补齐到
    /// `samples` 点）；两侧皆空返回空。
    pub fn morph_points(from: &Path2D, to: &Path2D, t: f32, samples: usize) -> Vec<Vec2> {
        let n = samples.max(1);
        let a = from.resample_arc_length(n, MORPH_TOLERANCE, true);
        let b = to.resample_arc_length(n, MORPH_TOLERANCE, true);
        match (a.is_empty(), b.is_empty()) {
            (true, true) => Vec::new(),
            (true, false) => b,
            (false, true) => a,
            (false, false) => a
                .iter()
                .zip(b.iter())
                .map(|(p, q)| *p + (*q - *p) * t)
                .collect(),
        }
    }

    /// 两条路径按弧长对齐后逐点插值，输出**闭合折线**（含 `samples` 个不同顶点）。
    ///
    /// 对齐按弧长（非索引），因此不同复杂度 / 不同点数的路径也能平滑过渡。
    pub fn morph(from: &Path2D, to: &Path2D, t: f32, samples: usize) -> Path2D {
        let pts = Path2D::morph_points(from, to, t, samples);
        if pts.is_empty() {
            return Path2D::new();
        }
        Path2D::from_polyline(&pts, true)
    }

    /// 返回把所有点应用 `transform` 后的新路径。
    ///
    /// - `MoveTo` / `LineTo` / `QuadTo` / `CubicTo` / `Close` 的端点与控制点逐点变换
    ///   （仿射变换下贝塞尔保持形状）；
    /// - `Arc` 在一般仿射变换下不再是标准弧，按 32 段采样成折线（已知近似）。
    pub fn transformed(&self, transform: &Transform2D) -> Path2D {
        let mut out = Path2D::with_capacity(self.commands.len());
        for cmd in &self.commands {
            match *cmd {
                PathCommand::MoveTo(p) => {
                    out.move_to(transform.apply_point(p));
                }
                PathCommand::LineTo(p) => {
                    out.line_to(transform.apply_point(p));
                }
                PathCommand::QuadTo { ctrl, end } => {
                    out.quad_to(transform.apply_point(ctrl), transform.apply_point(end));
                }
                PathCommand::CubicTo { ctrl1, ctrl2, end } => {
                    out.cubic_to(
                        transform.apply_point(ctrl1),
                        transform.apply_point(ctrl2),
                        transform.apply_point(end),
                    );
                }
                PathCommand::Arc {
                    center,
                    radii,
                    rotation,
                    start_angle,
                    end_angle,
                } => {
                    const STEPS: usize = 32;
                    for k in 0..=STEPS {
                        let a = start_angle
                            + (end_angle - start_angle) * (k as f32 / STEPS as f32);
                        let p = transform.apply_point(arc_point(center, radii, rotation, a));
                        if out.commands.is_empty() {
                            out.move_to(p);
                        } else {
                            out.line_to(p);
                        }
                    }
                }
                PathCommand::Close => {
                    out.close();
                }
            }
        }
        out
    }

    /// `flatten` 后（可选）补上闭合段起点。
    fn flatten_closed_points(&self, tolerance: f32, closed: bool) -> Vec<Vec2> {
        let mut pts = self.flatten(tolerance);
        if closed && pts.len() >= 2 {
            let first = pts[0];
            let last = *pts.last().unwrap();
            if (first - last).length() > 1e-6 {
                pts.push(first);
            }
        }
        pts
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
                PathCommand::Arc { center, radii, .. } => {
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

// ─── 3D 路径（M1 T3）────────────────────────────────────────────────────────

/// 3D 路径：折线顶点序列 + 闭合标记。
///
/// ⚠️ **本类型为简化版（折线），与 [`Path2D`]（命令式）不同构**：
/// `Path2D = Vec<PathCommand>`（精确边界，可无限细分）；`Path3D = Vec<Vec3> + closed`
/// （本质是采样结果 —— 其唯一来源就是
/// [`ParameterCurveT2XYZ`](crate::curve::ParameterCurveT2XYZ) 采样后的折线）。
/// 若将来 3D 需要真正的曲线（贝塞尔曲面边界 / 3D 描边 / 任意细分），`Path3D`
/// 须改为命令式 —— **那是破坏性变更**，需单独立项。
///
/// **形态选择（M1 交付说明）**：采用 `Vec<Vec3>` + `closed` 的**简化版**，而非与
/// [`Path2D`] 对称的枚举命令式。理由：3D 侧目前唯一来源是
/// [`ParameterCurveT2XYZ`](crate::curve::ParameterCurveT2XYZ) 采样后吐出的离散折线，
/// 全仓没有任何 3D 曲线命令（贝塞尔 / 弧）的消费方；先建立**类型出口**，
/// 待出现命令式需求（3D 曲线编辑）再升级，避免本阶段引入无人消费的
/// `PathCommand3D` 与 3D 细分算法。弧长重采样 / 形变 / 仿射变换这些理念层代数
/// 在两种形态下都成立，故简化版不损失本阶段承诺的能力。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Path3D {
    pub points: Vec<Vec3>,
    /// 是否隐式闭合（末点 → 首点一段计入长度 / 重采样）。
    pub closed: bool,
}

impl Path3D {
    pub fn new() -> Self {
        Self::default()
    }

    /// 从折线点序列构造；`closed` 时闭合段计入长度。
    pub fn from_polyline(points: &[Vec3], closed: bool) -> Self {
        Self {
            points: points.to_vec(),
            closed,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// 顶点数（`closed = true` 时不含重复的尾点）。
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// 总长度；`closed` 且首尾不重合时补上闭合段。
    pub fn length(&self) -> f32 {
        let cum = cumulative_lengths3(&self.sample_points());
        cum.last().copied().unwrap_or(0.0)
    }

    /// 轴对齐包围盒 `(min, max)`；空路径返回 `None`。
    pub fn bounds(&self) -> Option<(Vec3, Vec3)> {
        let mut it = self.sample_points().into_iter();
        let first = it.next()?;
        let (mut min, mut max) = (first, first);
        for p in it {
            min = min.min(p);
            max = max.max(p);
        }
        Some((min, max))
    }

    /// 按弧长均匀重采样为 `n` 个点。
    ///
    /// - `closed = true`：沿闭合环取 `n` 个点（首点即起点，不含重复尾点）；
    /// - `closed = false`：含两端点（`n == 1` 取起点）；
    /// - 空返回空；单点 / 零长返回 `n` 个相同点（不 panic）。
    pub fn resample_arc_length(&self, n: usize) -> Vec<Vec3> {
        if n == 0 {
            return Vec::new();
        }
        let pts = self.sample_points();
        if pts.is_empty() {
            return Vec::new();
        }
        if pts.len() == 1 {
            return vec![pts[0]; n];
        }
        let cum = cumulative_lengths3(&pts);
        let total = *cum.last().unwrap();
        if total <= 1e-9 {
            return vec![pts[0]; n];
        }

        let mut out = Vec::with_capacity(n);
        let mut j = 0usize;
        for i in 0..n {
            let target = if self.closed {
                total * (i as f32) / (n as f32)
            } else if n == 1 {
                0.0
            } else {
                total * (i as f32) / ((n as f32) - 1.0)
            };
            while j + 2 < pts.len() && cum[j + 1] < target {
                j += 1;
            }
            let (a, b) = (pts[j], pts[j + 1]);
            let seg = cum[j + 1] - cum[j];
            let local = if seg > 1e-9 {
                ((target - cum[j]) / seg).clamp(0.0, 1.0)
            } else {
                0.0
            };
            out.push(a + (b - a) * local);
        }
        out
    }

    /// 两条 3D 路径按弧长对齐逐点插值，输出闭合折线（`samples` 个不同顶点）。
    ///
    /// 口径与 [`Path2D::morph`] 一致：长度恒为 `samples`，与输入复杂度无关。
    pub fn morph(from: &Path3D, to: &Path3D, t: f32, samples: usize) -> Path3D {
        let n = samples.max(1);
        let a = from.resample_closed(n);
        let b = to.resample_closed(n);
        let pts = match (a.is_empty(), b.is_empty()) {
            (true, true) => return Path3D::new(),
            (true, false) => b,
            (false, true) => a,
            (false, false) => a
                .iter()
                .zip(b.iter())
                .map(|(p, q)| *p + (*q - *p) * t)
                .collect(),
        };
        Path3D {
            points: pts,
            closed: true,
        }
    }

    /// 返回把所有顶点应用 `transform` 后的新路径（仿射变换下仍是折线）。
    pub fn transformed(&self, transform: &Transform3D) -> Path3D {
        Path3D {
            points: self
                .points
                .iter()
                .map(|p| transform.apply_point(*p))
                .collect(),
            closed: self.closed,
        }
    }

    /// 闭合环重采样（`closed` 语义由方法强制，供 `morph` 使用）。
    fn resample_closed(&self, n: usize) -> Vec<Vec3> {
        Path3D {
            points: self.points.clone(),
            closed: true,
        }
        .resample_arc_length(n)
    }

    /// 用于长度 / 包围盒的顶点序列（`closed` 时补上闭合段起点）。
    fn sample_points(&self) -> Vec<Vec3> {
        let mut pts = self.points.clone();
        if self.closed && pts.len() >= 2 {
            let first = pts[0];
            let last = *pts.last().unwrap();
            if (first - last).length() > 1e-6 {
                pts.push(first);
            }
        }
        pts
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

/// 折线的累积弧长表：与点数等长，首元素 `0.0`，末元素为总长。
fn cumulative_lengths(pts: &[Vec2]) -> Vec<f32> {
    if pts.is_empty() {
        return Vec::new();
    }
    let mut cum = Vec::with_capacity(pts.len());
    let mut acc = 0.0f32;
    cum.push(0.0);
    for w in pts.windows(2) {
        acc += (w[1] - w[0]).length();
        cum.push(acc);
    }
    cum
}

/// 3D 折线的累积弧长表（与 [`cumulative_lengths`] 同口径）。
fn cumulative_lengths3(pts: &[Vec3]) -> Vec<f32> {
    if pts.is_empty() {
        return Vec::new();
    }
    let mut cum = Vec::with_capacity(pts.len());
    let mut acc = 0.0f32;
    cum.push(0.0);
    for w in pts.windows(2) {
        acc += (w[1] - w[0]).length();
        cum.push(acc);
    }
    cum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_polyline_and_close() {
        let path = Path2D::from_polyline(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 0.0),
                Vec2::new(1.0, 1.0),
            ],
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
    fn test_flatten_subpaths_splits_two_contours() {
        // 外圈矩形 + 内圈矩形（模拟 `o` 的外轮廓与内孔）。
        let mut path = Path2D::new();
        path.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(10.0, 0.0))
            .line_to(Vec2::new(10.0, 10.0))
            .line_to(Vec2::new(0.0, 10.0))
            .close()
            .move_to(Vec2::new(2.0, 2.0))
            .line_to(Vec2::new(8.0, 2.0))
            .line_to(Vec2::new(8.0, 8.0))
            .line_to(Vec2::new(2.0, 8.0))
            .close();

        let groups = path.flatten_subpaths(0.1);
        assert_eq!(groups.len(), 2, "应分成两个子路径组");
        assert_eq!(groups[0].len(), 5, "外圈：4 顶点 + Close 回起点");
        assert_eq!(groups[1].len(), 5, "内圈：4 顶点 + Close 回起点");
        // 每组首尾相同 → 视为闭合。
        assert_eq!(groups[0].first(), groups[0].last());
        assert_eq!(groups[1].first(), groups[1].last());
        // 两组起点不同（未被拼成一条）。
        assert_ne!(groups[0].first(), groups[1].first());
    }

    #[test]
    fn test_flatten_subpaths_keeps_open_subpaths_separate() {
        // 两条互不相连的开放折线：不得在两者之间产生桥接段。
        let mut path = Path2D::new();
        path.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(1.0, 0.0))
            .move_to(Vec2::new(5.0, 5.0))
            .line_to(Vec2::new(6.0, 5.0));

        let groups = path.flatten_subpaths(0.1);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), 2);
        assert_eq!(groups[1].len(), 2);
        // 开放（未 Close）：首尾不同。
        assert_ne!(groups[0].first(), groups[0].last());
        assert_ne!(groups[1].first(), groups[1].last());
    }

    // ── 弧长重采样 / 形变 ────────────────────────────────────────────

    /// 逼近单位圆的 512 边形（弧长测试基准）。
    fn unit_circle_poly() -> Vec<Vec2> {
        let n = 512;
        (0..n)
            .map(|i| {
                let a = std::f32::consts::TAU * (i as f32) / (n as f32);
                Vec2::new(a.cos(), a.sin())
            })
            .collect()
    }

    /// 闭合折线的"不同顶点数"（flatten 末尾会重复起点）。
    fn distinct_len(p: &Path2D) -> usize {
        let f = p.flatten(0.0);
        if f.len() >= 2 && f.first() == f.last() {
            f.len() - 1
        } else {
            f.len()
        }
    }

    /// 开放直线：重采样后点等距，含两端点。
    #[test]
    fn resample_open_line_is_evenly_spaced() {
        let path = Path2D::from_polyline(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)], false);
        let pts = path.resample_arc_length(11, 0.01, false);
        assert_eq!(pts.len(), 11);
        for (i, p) in pts.iter().enumerate() {
            assert!((p.x - i as f32).abs() < 1e-4, "i={i}: {p:?}");
            assert!(p.y.abs() < 1e-6);
        }
    }

    /// 圆周：闭合重采样后相邻点距离近似相等（误差 < 1e-3）。
    #[test]
    fn resample_closed_circle_is_evenly_spaced() {
        let circle = Path2D::from_polyline(&unit_circle_poly(), true);

        let m = 64usize;
        let pts = circle.resample_arc_length(m, 0.001, true);
        assert_eq!(pts.len(), m);
        let expected = std::f32::consts::TAU / (m as f32); // 单位圆周长均分
        for i in 0..m {
            let d = (pts[(i + 1) % m] - pts[i]).length();
            assert!(
                (d - expected).abs() < 1e-3,
                "i={i}: d={d}, expected={expected}"
            );
        }
    }

    /// 不同 n 都逼近同一形状（都在单位圆上）。
    #[test]
    fn resample_count_invariant_shape() {
        let circle = Path2D::from_polyline(&unit_circle_poly(), true);
        for &m in &[32usize, 128] {
            let pts = circle.resample_arc_length(m, 0.001, true);
            assert_eq!(pts.len(), m);
            for p in &pts {
                assert!((p.length() - 1.0).abs() < 1e-3, "r={}", p.length());
            }
        }
    }

    /// 空 / 单点 / 零长度路径不 panic。
    #[test]
    fn resample_edge_cases_do_not_panic() {
        assert!(Path2D::new().resample_arc_length(8, 0.1, true).is_empty());
        assert!(Path2D::new().arc_lengths(0.1, true).is_empty());

        let single = Path2D::from_polyline(&[Vec2::new(3.0, 4.0)], false);
        let pts = single.resample_arc_length(8, 0.1, false);
        assert_eq!(pts.len(), 8);
        assert!(pts.iter().all(|p| (*p - Vec2::new(3.0, 4.0)).length() < 1e-6));

        // n = 0 → 空
        assert!(single.resample_arc_length(0, 0.1, false).is_empty());
    }

    /// 圆 → 方：t=0.5 时不同顶点数 == samples，且包围盒介于两者之间。
    #[test]
    fn morph_circle_to_square_midpoint() {
        let circle = Path2D::from_polyline(&unit_circle_poly(), true);
        // 方比圆大，便于断言"中点介于两者之间"。
        let square = Path2D::from_polyline(
            &[
                Vec2::new(-1.5, -1.5),
                Vec2::new(1.5, -1.5),
                Vec2::new(1.5, 1.5),
                Vec2::new(-1.5, 1.5),
            ],
            true,
        );

        let samples = 128;
        let m0 = Path2D::morph(&circle, &square, 0.0, samples);
        let m1 = Path2D::morph(&circle, &square, 1.0, samples);
        let mm = Path2D::morph(&circle, &square, 0.5, samples);

        assert_eq!(distinct_len(&m0), samples);
        assert_eq!(distinct_len(&m1), samples);
        assert_eq!(distinct_len(&mm), samples);

        let bb = |p: &Path2D| p.compute_bounds().unwrap();
        let (bs, bm) = (bb(&square), bb(&mm));
        // 端点精确：t=0 → 圆（±1），t=1 → 方（±1.5）。
        assert!((bb(&m0).min.x + 1.0).abs() < 1e-3 && (bb(&m0).max.x - 1.0).abs() < 1e-3);
        assert!((bb(&m1).min.x + 1.5).abs() < 1e-3 && (bb(&m1).max.x - 1.5).abs() < 1e-3);
        // 中点不越界：逐点是两端的凸组合 → 包围盒落在二者并集内（无过冲）。
        assert!(bm.min.x >= bs.min.x - 1e-4 && bm.max.x <= bs.max.x + 1e-4);
        assert!(bm.min.y >= bs.min.y - 1e-4 && bm.max.y <= bs.max.y + 1e-4);
    }

    /// 变换后端点正确（缩放 + 平移）。
    #[test]
    fn transformed_applies_scale_and_translate() {
        let p = Path2D::from_polyline(&[Vec2::new(0.0, 0.0), Vec2::new(2.0, 1.0)], false);
        let t = Transform2D::scale(Vec2::splat(2.0)).compose(Transform2D::translate(Vec2::new(1.0, -1.0)));
        let out = p.transformed(&t);
        let b = out.compute_bounds().unwrap();
        // (0,0)→(1,-1)，(2,1)→(5,1)
        assert!((b.min.x - 1.0).abs() < 1e-5 && (b.min.y + 1.0).abs() < 1e-5);
        assert!((b.max.x - 5.0).abs() < 1e-5 && (b.max.y - 1.0).abs() < 1e-5);
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

    // ── Path3D（M1 T3）────────────────────────────────────────────────

    /// 3-4-5 直角折线（z=0 平面内）。
    fn right_triangle_3d() -> Path3D {
        Path3D::from_polyline(
            &[
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(4.0, 0.0, 0.0),
                Vec3::new(0.0, 3.0, 0.0),
            ],
            true,
        )
    }

    #[test]
    fn path3d_open_length_and_bounds() {
        let p = Path3D::from_polyline(
            &[
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(3.0, 0.0, 0.0),
                Vec3::new(3.0, 4.0, 0.0),
            ],
            false,
        );
        assert_eq!(p.len(), 3);
        assert_eq!(p.length(), 7.0);
        assert_eq!(p.bounds(), Some((Vec3::ZERO, Vec3::new(3.0, 4.0, 0.0))));
    }

    /// 闭合：末点 → 首点一段计入长度（4 + 5 + 3 = 12，而非 9）。
    #[test]
    fn path3d_closed_adds_segment() {
        let p = right_triangle_3d();
        assert_eq!(p.length(), 12.0);
        // 首尾已显式重合时不重复计段。
        let mut q = right_triangle_3d();
        q.points.push(Vec3::ZERO);
        assert_eq!(q.length(), 12.0);
    }

    #[test]
    fn path3d_empty_and_single_point_do_not_panic() {
        let empty = Path3D::new();
        assert!(empty.is_empty());
        assert_eq!(empty.length(), 0.0);
        assert_eq!(empty.bounds(), None);
        assert!(empty.resample_arc_length(8).is_empty());

        let single = Path3D::from_polyline(&[Vec3::new(1.0, 2.0, 3.0)], true);
        let pts = single.resample_arc_length(5);
        assert_eq!(pts.len(), 5);
        assert!(pts.iter().all(|p| *p == Vec3::new(1.0, 2.0, 3.0)));
    }

    /// 开放直线重采样：n 点等距，含两端点。
    #[test]
    fn path3d_resample_open_line_is_evenly_spaced() {
        let p = Path3D::from_polyline(&[Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)], false);
        let pts = p.resample_arc_length(11);
        assert_eq!(pts.len(), 11);
        for (i, q) in pts.iter().enumerate() {
            assert!((q.x - i as f32).abs() < 1e-4, "i={i}: {q:?}");
        }
    }

    /// 闭合环重采样：n 点绕环均分，不含重复尾点。
    #[test]
    fn path3d_resample_closed_loops_uniformly() {
        let p = right_triangle_3d();
        let pts = p.resample_arc_length(12);
        assert_eq!(pts.len(), 12);
        // 周长 12 / 12 点 → 相邻点（含末点→首点）距离均为 1。
        for i in 0..12 {
            let d = (pts[(i + 1) % 12] - pts[i]).length();
            assert!((d - 1.0).abs() < 1e-4, "i={i}: d={d}");
        }
    }

    /// `morph` 长度恒为 `samples`；`to = 2 * from` 时 t=0.5 恰为 1.5 × from。
    #[test]
    fn path3d_morph_is_arc_length_aligned() {
        let from = right_triangle_3d();
        let to = Path3D::from_polyline(
            &from.points.iter().map(|p| *p * 2.0).collect::<Vec<_>>(),
            true,
        );

        let samples = 16;
        let mid = Path3D::morph(&from, &to, 0.5, samples);
        assert_eq!(mid.len(), samples);
        assert!(mid.closed);

        let a = Path3D {
            points: from.points.clone(),
            closed: true,
        }
        .resample_arc_length(samples);
        for (i, p) in mid.points.iter().enumerate() {
            let expected = a[i] * 1.5;
            assert!(
                (*p - expected).length() < 1e-4,
                "i={i}: {p:?} vs {expected:?}"
            );
        }

        // 两端：t=0 → from 的闭合环；两侧皆空 → 空。
        let zero = Path3D::morph(&from, &to, 0.0, samples);
        for (i, p) in zero.points.iter().enumerate() {
            assert!((*p - a[i]).length() < 1e-4, "i={i}: {p:?} vs {:?}", a[i]);
        }
        assert!(Path3D::morph(&Path3D::new(), &Path3D::new(), 0.5, 8).is_empty());
    }

    /// 仿射变换：先 2 倍缩放再平移 (1, 0, 0)；闭合标记保持。
    #[test]
    fn path3d_transformed_scales_and_translates() {
        let p = right_triangle_3d();
        let t = Transform3D::scale(Vec3::splat(2.0))
            .compose(Transform3D::translate(Vec3::new(1.0, 0.0, 0.0)));
        let out = p.transformed(&t);
        assert_eq!(out.points[0], Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(out.points[1], Vec3::new(9.0, 0.0, 0.0));
        assert_eq!(out.points[2], Vec3::new(1.0, 6.0, 0.0));
        assert!(out.closed);
        assert!((out.length() - 2.0 * p.length()).abs() < 1e-4);
        assert_eq!(
            out.bounds(),
            Some((Vec3::new(1.0, 0.0, 0.0), Vec3::new(9.0, 6.0, 0.0)))
        );
    }
}
