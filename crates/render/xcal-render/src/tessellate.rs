// tessellate.rs
//
// S3-0：lyon 三角化适配层（纯 CPU、headless，无 wgpu / egui 依赖）。
//
// 目标：`Shape2D + Transform2D + Style2D` → 索引化顶点缓冲，顶点结构**严格**符合
// 自有顶点契约（[`VertexLayout::standard`]，stride = 20，三槽 position/attrib0/attrib1）。
//
// # 顶点槽语义（`AaModel::Analytic` 口径）
//
// ```text
// position [f32;2] @0   ← lyon `StrokeVertex::position()`（**已含宽度偏移的实际轮廓点**）
// normal   [f32;2] @8   ← lyon `StrokeVertex::normal()`（含 miter 缩放，未归一化）
// side     f32     @16  ← lyon `Side` → ±1.0
// ```
//
// 三者恒满足 `position_on_path = position - normal * (line_width * 0.5) * side`
// （lyon 的 `normal` 长度定义为"沿它位移 1 个单位即把描边撑粗 2.0"）。
// 反算出的 `position_on_path` 落在原始路径上——S3-0 的往返测试即锁定此恒等式。
//
// **为什么存 `position()`（轮廓点）而不是 `position_on_path()`**：
// 计划 §4.3 的注释写的是 `position_on_path()`，但 §4.7 的往返测试公式
// （`center = position - normal * (w*0.5) * side` 落在原路径上）与 §5.3
// （"`AaModel::None` + MSAA 时 shader **不解释** normal/side"）都要求
// `position` 是可直接光栅化的最终轮廓点。两者不能同时成立，这里以 **§4.7 测试
// + §5.3 渲染正确性** 为准：存 `position()`。将来切 Analytic AA 时 shader 用
// `position - normal * (w*0.5) * side` 即可还原路径点，顶点格式无需改动。
//
// # 坐标系
//
// 几何在 **`transform` 作用之后的空间**（输出空间）；`style.stroke_width` 也按
// 同一空间解释（因此本阶段描边宽度由 CPU 侧 lyon 落进几何，见计划 §7 已知限制）。
//
// # Group
//
// 计划 §4.6 建议 `tessellate` 返回 `Vec<Tessellation2D>`，Group 展开为多项；
// 本实现遵循该建议（`Empty` 因此返回**空 Vec**，而非 `Option::None`）。

use lyon_path::Path as LyonPath;
use lyon_path::math::point;
use lyon_tessellation::{
    BuffersBuilder, FillOptions, FillRule, FillTessellator, FillVertex, FillVertexConstructor,
    StrokeOptions, StrokeTessellator, StrokeVertex, StrokeVertexConstructor, VertexBuffers,
};

use xc_geometry::{Transform2D, Triangle2D, Vec2};

use crate::painter_2d::Style2D;
use crate::shape::{CustomShapeData, Shape2D};
use crate::vertex::{AaModel, CacheStrategy, VertexLayout};

/// 描边/填充顶点：与顶点契约的三槽 1:1。
///
/// # Safety / 布局
///
/// `#[repr(C)]` + 三槽 `[f32;2] / [f32;2] / f32` → size 20、align 4，
/// 与 [`VertexLayout::standard`] 的 `position@0 / attrib0@8 / attrib1@16 / stride 20`
/// 完全一致（编译期断言见下方 `const _`）。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex2D {
    /// 轮廓点（含宽度偏移）→ 契约槽 `position`。
    pub position: [f32; 2],
    /// 边法线（含 miter 缩放，未归一化）→ 契约槽 `attrib0`。
    pub normal: [f32; 2],
    /// 边侧符号 ±1.0（填充恒 0.0）→ 契约槽 `attrib1`。
    pub side: f32,
}

impl Vertex2D {
    /// 填充顶点：`normal` / `side` 无 AA 语义，恒为 0（**文档化行为**）。
    pub const FILL_SLOT_ZERO: ([f32; 2], f32) = ([0.0, 0.0], 0.0);
}

// 编译期断言：顶点布局与契约 stride 一致（计划 §4.3 硬要求）。
const _: () = assert!(std::mem::size_of::<Vertex2D>() == 20);

/// 三角化结果类型：填充 or 描边（决定 shader 是否可解释 normal/side）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TessKind {
    Fill,
    Stroke,
}

/// 三角化结果：索引化几何 + 布局声明。
#[derive(Debug, Clone)]
pub struct Tessellation2D {
    pub vertices: Vec<Vertex2D>,
    pub indices: Vec<u32>,
    /// 顶点布局声明（恒为 [`VertexLayout::standard`]`(AaModel::Analytic, CacheStrategy::Static)`）。
    pub layout: VertexLayout,
    pub kind: TessKind,
}

impl Tessellation2D {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// 顶点契约布局（本阶段固定）：`Analytic` 三槽 + `Static` 缓存策略。
///
/// 用 `Analytic` 而非 `None`：布局**三槽相同**，将来从 MSAA 切解析 AA 只需换 shader。
pub fn tessellation_layout() -> VertexLayout {
    VertexLayout::standard(AaModel::Analytic, CacheStrategy::Static)
}

/// 展平容差：让误差不超过约 0.25 屏幕像素（`scale` = 每世界单位像素数）。
///
/// 固定容差在缩小时过粗、放大时过细，故按 `scale` 反比推导并夹在 `[1e-4, 1.0]`。
pub fn tolerance_for_scale(scale: f32) -> f32 {
    (0.25 / scale.max(1e-6)).clamp(1e-4, 1.0)
}

// ─── 内部：轮廓（子路径）提取 ──────────────────────────────────────────────

/// 一条子路径：输出空间下的点列（闭合时**不含**重复的尾点）。
struct Contour {
    points: Vec<Vec2>,
    closed: bool,
}

fn closed_eps() -> f32 {
    1e-6
}

/// 从「可能含重复尾点」的点列构造轮廓（闭合判定：首尾点重合）。
///
/// `force_closed`：调用方已知闭合（Polygon / Rect / Circle 等，**不含**重复尾点）；
/// 否则按首尾点重合判定（`Path2D::flatten_subpaths` 的 `Close` 会补出重复尾点）。
fn contour_of(mut points: Vec<Vec2>, force_closed: bool) -> Option<Contour> {
    if points.len() < 2 {
        return None;
    }
    let duplicated = points.len() >= 3
        && (points[0] - points[points.len() - 1]).length() <= closed_eps();
    let closed = force_closed || duplicated;
    if duplicated {
        points.pop();
    }
    if points.len() < 2 {
        return None;
    }
    Some(Contour { points, closed })
}

/// 圆/椭圆按弦误差 ≤ `tolerance` 的折线近似（闭合）。
fn ellipse_points(center: Vec2, rx: f32, ry: f32, tolerance: f32) -> Vec<Vec2> {
    let r = rx.abs().max(ry.abs()).max(1e-6);
    let tol = tolerance.max(1e-6).min(r * 0.9);
    // 弦误差（sagitta）= r * (1 - cos(step/2)) ≤ tol
    let cos_half = (1.0 - tol / r).clamp(-1.0, 1.0);
    let step = 2.0 * cos_half.acos();
    let n = ((std::f32::consts::TAU / step.max(1e-3)).ceil() as usize).clamp(8, 4096);
    (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * (i as f32) / (n as f32);
            center + Vec2::new(rx * a.cos(), ry * a.sin())
        })
        .collect()
}

/// `Shape2D` → 轮廓列表（已应用 `transform`；`Path` 额外按 `tolerance` 展平）。
fn contours_of(shape: &Shape2D, transform: &Transform2D, tolerance: f32) -> Vec<Contour> {
    let map = |pts: &[Vec2]| -> Vec<Vec2> { pts.iter().map(|p| transform.apply_point(*p)).collect() };

    match shape {
        Shape2D::Empty => Vec::new(),
        Shape2D::Line { start, end } => {
            let pts = map(&[*start, *end]);
            contour_of(pts, false).into_iter().collect()
        }
        Shape2D::Polyline { points } => {
            let pts = map(points);
            contour_of(pts, false).into_iter().collect()
        }
        Shape2D::Polygon { points } => {
            let pts = map(points);
            contour_of(pts, true).into_iter().collect()
        }
        Shape2D::Rect(rect) => {
            let pts = map(&[
                rect.min,
                Vec2::new(rect.max.x, rect.min.y),
                rect.max,
                Vec2::new(rect.min.x, rect.max.y),
            ]);
            contour_of(pts, true).into_iter().collect()
        }
        Shape2D::Circle { center, radius } => {
            let pts = map(&ellipse_points(*center, *radius, *radius, tolerance));
            contour_of(pts, true).into_iter().collect()
        }
        Shape2D::Ellipse { center, rx, ry } => {
            let pts = map(&ellipse_points(*center, *rx, *ry, tolerance));
            contour_of(pts, true).into_iter().collect()
        }
        Shape2D::Path(path) => path
            .flatten_subpaths(tolerance)
            .into_iter()
            .filter_map(|sub| contour_of(map(&sub), false))
            .collect(),
        // Group / Custom 在 [`tessellate`] 顶层分派，不会走到这里。
        Shape2D::Group { .. } | Shape2D::Custom(_) => Vec::new(),
    }
}

fn build_lyon_path(contours: &[Contour]) -> LyonPath {
    let mut builder = LyonPath::builder();
    for c in contours {
        if c.points.len() < 2 {
            continue;
        }
        builder.begin(point(c.points[0].x, c.points[0].y));
        for p in &c.points[1..] {
            builder.line_to(point(p.x, p.y));
        }
        if c.closed {
            // `close()` 即 `end(true)`（lyon 语义：结束子路径并闭合）。
            builder.close();
        } else {
            builder.end(false);
        }
    }
    builder.build()
}

// ─── 顶点构造器 ───────────────────────────────────────────────────────────

/// 填充顶点：`normal = [0,0]`、`side = 0`（无解析 AA 语义，文档化行为）。
struct FillCtor;

impl FillVertexConstructor<Vertex2D> for FillCtor {
    fn new_vertex(&mut self, vertex: FillVertex) -> Vertex2D {
        let p = vertex.position();
        Vertex2D {
            position: [p.x, p.y],
            normal: Vertex2D::FILL_SLOT_ZERO.0,
            side: Vertex2D::FILL_SLOT_ZERO.1,
        }
    }
}

/// 描边顶点：`position` 取 lyon 的实际轮廓点，`normal`/`side` 供解析 AA 用。
struct StrokeCtor;

impl StrokeVertexConstructor<Vertex2D> for StrokeCtor {
    fn new_vertex(&mut self, vertex: StrokeVertex) -> Vertex2D {
        let p = vertex.position();
        let n = vertex.normal();
        let side = if vertex.side().is_positive() { 1.0 } else { -1.0 };
        Vertex2D {
            position: [p.x, p.y],
            normal: [n.x, n.y],
            side,
        }
    }
}

// ─── 顶层 API ─────────────────────────────────────────────────────────────

/// `Shape2D` → 三角化结果列表。
///
/// - `transform`：图元本地点 → 输出空间的变换（在三角化**之前**应用于路径点，
///   使 lyon 在输出空间直接算出正确的法线/miter）；
/// - `style`：`fill` / `stroke` / `stroke_width` 决定产出（各自至多一项）；
/// - `tolerance`：展平容差（输出空间单位），由 [`tolerance_for_scale`] 推导。
///
/// `Group` 递归展开为多项；`Empty` 返回空列表。
pub fn tessellate(
    shape: &Shape2D,
    transform: &Transform2D,
    style: &Style2D,
    tolerance: f32,
) -> Vec<Tessellation2D> {
    match shape {
        Shape2D::Empty => Vec::new(),
        Shape2D::Group { children } => children
            .iter()
            .flat_map(|child| tessellate(child, transform, style, tolerance))
            .collect(),
        Shape2D::Custom(data) => tessellate_custom(data, transform, style, tolerance),
        _ => {
            let contours = contours_of(shape, transform, tolerance);
            if contours.is_empty() {
                return Vec::new();
            }
            let mut out = Vec::new();
            if style.fill.is_some() {
                if let Some(t) = tessellate_fill(&contours) {
                    out.push(t);
                }
            }
            if style.stroke.is_some() && style.stroke_width > 0.0 {
                if let Some(t) = tessellate_stroke(&contours, style.stroke_width) {
                    out.push(t);
                }
            }
            out
        }
    }
}

/// 降级钩子：宿主自定义图元（`to_triangles` 优先，其次 `to_path`）。
fn tessellate_custom(
    data: &CustomShapeData,
    transform: &Transform2D,
    style: &Style2D,
    tolerance: f32,
) -> Vec<Tessellation2D> {
    if let Some(to_triangles) = data.to_triangles {
        // 约定：`to_triangles` 产出**已应用 transform** 的三角形。
        let tris = to_triangles(data, transform);
        let mut vertices = Vec::with_capacity(tris.len() * 3);
        let mut indices = Vec::with_capacity(tris.len() * 3);
        for tri in tris {
            for p in [tri.a, tri.b, tri.c] {
                indices.push(vertices.len() as u32);
                vertices.push(Vertex2D {
                    position: [p.x, p.y],
                    normal: Vertex2D::FILL_SLOT_ZERO.0,
                    side: Vertex2D::FILL_SLOT_ZERO.1,
                });
            }
        }
        return vec![Tessellation2D {
            vertices,
            indices,
            layout: tessellation_layout(),
            kind: TessKind::Fill,
        }];
    }
    if let Some(to_path) = data.to_path {
        // 约定：`to_path` 产出**已应用 transform** 的路径。
        let path = to_path(data, transform);
        return tessellate(&Shape2D::Path(path), &Transform2D::identity(), style, tolerance);
    }
    Vec::new()
}

fn tessellate_fill(contours: &[Contour]) -> Option<Tessellation2D> {
    let path = build_lyon_path(contours);
    let mut buffers: VertexBuffers<Vertex2D, u32> = VertexBuffers::new();
    let mut tessellator = FillTessellator::new();
    tessellator
        .tessellate_path(
            &path,
            &FillOptions::default().with_fill_rule(FillRule::NonZero),
            &mut BuffersBuilder::new(&mut buffers, FillCtor),
        )
        .ok()?;
    if buffers.indices.is_empty() {
        return None;
    }
    Some(Tessellation2D {
        vertices: buffers.vertices,
        indices: buffers.indices,
        layout: tessellation_layout(),
        kind: TessKind::Fill,
    })
}

fn tessellate_stroke(contours: &[Contour], width: f32) -> Option<Tessellation2D> {
    let path = build_lyon_path(contours);
    let mut buffers: VertexBuffers<Vertex2D, u32> = VertexBuffers::new();
    let mut tessellator = StrokeTessellator::new();
    let options = StrokeOptions::default().with_line_width(width);
    tessellator
        .tessellate_path(
            &path,
            &options,
            &mut BuffersBuilder::new(&mut buffers, StrokeCtor),
        )
        .ok()?;
    if buffers.indices.is_empty() {
        return None;
    }
    Some(Tessellation2D {
        vertices: buffers.vertices,
        indices: buffers.indices,
        layout: tessellation_layout(),
        kind: TessKind::Stroke,
    })
}

// ─── 测试辅助（也供集成测试/下游复用） ─────────────────────────────────────

/// 由顶点三元组反算路径点：`position - normal * (line_width * 0.5)`。
///
/// **不含 `* side`**：lyon 的 `normal()` 已带 side 符号
/// （源码恒等式 `position() = position_on_path() + normal() * half_width`，
/// 见 `StrokeVertex::position`），再乘 `side` 会二次施加符号。计划 §4.7 的
/// `... * side` 是笔误，§8 的 `position + normal * (w*0.5)` 才是正确口径。
pub fn position_on_path_of(v: &Vertex2D, line_width: f32) -> Vec2 {
    let k = line_width * 0.5;
    Vec2::new(v.position[0] - v.normal[0] * k, v.position[1] - v.normal[1] * k)
}

/// 顶点三元组的三角形总面积（光栅化面积统计用）。
pub fn total_area(t: &Tessellation2D) -> f32 {
    t.indices
        .chunks_exact(3)
        .map(|tri| {
            let a = t.vertices[tri[0] as usize].position;
            let b = t.vertices[tri[1] as usize].position;
            let c = t.vertices[tri[2] as usize].position;
            Triangle2D::new(
                Vec2::new(a[0], a[1]),
                Vec2::new(b[0], b[1]),
                Vec2::new(c[0], c[1]),
            )
            .area()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_geometry::{Color32, MeasureSpace, Path2D, Rect2D};

    fn style_fill() -> Style2D {
        Style2D {
            fill: Some(Color32::WHITE),
            stroke: None,
            stroke_width: 0.0,
            space: MeasureSpace::Screen,
        }
    }

    fn style_stroke(width: f32) -> Style2D {
        Style2D {
            fill: None,
            stroke: Some(Color32::BLACK),
            stroke_width: width,
            space: MeasureSpace::Screen,
        }
    }

    fn identity() -> Transform2D {
        Transform2D::identity()
    }

    fn dist_to_polyline(p: Vec2, poly: &[Vec2]) -> f32 {
        let mut best = f32::INFINITY;
        for w in poly.windows(2) {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let len2 = ab.length_squared();
            let d = if len2 < 1e-12 {
                (p - a).length()
            } else {
                let t = ((p - a).dot(ab) / len2).clamp(0.0, 1.0);
                (p - (a + ab * t)).length()
            };
            best = best.min(d);
        }
        best
    }

    // ── 契约匹配测试（最重要） ────────────────────────────────────────────

    #[test]
    fn vertex2d_size_is_20() {
        assert_eq!(std::mem::size_of::<Vertex2D>(), 20);
        assert_eq!(std::mem::align_of::<Vertex2D>(), 4);
    }

    #[test]
    fn stroke_vertices_match_contract_layout() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(10.0, 6.0)));
        let out = tessellate(&shape, &identity(), &style_stroke(2.0), 0.05);
        assert_eq!(out.len(), 1);
        let t = &out[0];
        assert_eq!(t.kind, TessKind::Stroke);
        assert_eq!(t.layout, VertexLayout::standard(AaModel::Analytic, CacheStrategy::Static));
        assert_eq!(t.layout.stride, 20);
        t.layout.validate().unwrap();
    }

    #[test]
    fn stroke_side_is_plus_or_minus_one() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(10.0, 6.0)));
        let t = &tessellate(&shape, &identity(), &style_stroke(2.0), 0.05)[0];
        for v in &t.vertices {
            assert!(
                (v.side - 1.0).abs() < 1e-6 || (v.side + 1.0).abs() < 1e-6,
                "side 必须为 ±1.0，实际 {}",
                v.side
            );
        }
    }

    #[test]
    fn stroke_normals_are_non_degenerate_and_finite() {
        let shape = Shape2D::Path(Path2D::from_polyline(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(10.0, 0.0),
                Vec2::new(10.0, 10.0),
            ],
            false,
        ));
        let t = &tessellate(&shape, &identity(), &style_stroke(3.0), 0.05)[0];
        assert!(!t.vertices.is_empty());
        for v in &t.vertices {
            let len = (v.normal[0] * v.normal[0] + v.normal[1] * v.normal[1]).sqrt();
            assert!(len > 1e-4, "normal 退化: {v:?}");
            assert!(v.normal[0].is_finite() && v.normal[1].is_finite());
            assert!(v.position[0].is_finite() && v.position[1].is_finite());
        }
    }

    // ── 往返测试（一次验证 position / normal / side） ──────────────────────

    #[test]
    fn stroke_roundtrip_lands_on_original_path() {
        let width = 4.0;
        let poly = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            Vec2::new(20.0, 12.0),
            Vec2::new(0.0, 12.0),
        ];
        let shape = Shape2D::Path(Path2D::from_polyline(&poly, true));
        let t = &tessellate(&shape, &identity(), &style_stroke(width), 0.05)[0];

        let mut max_err: f32 = 0.0;
        for v in &t.vertices {
            let center = position_on_path_of(v, width);
            max_err = max_err.max(dist_to_polyline(center, &closing(&poly)));
        }
        assert!(max_err < 1e-3, "反算中心点偏离原始路径 {max_err}");
    }

    /// 闭合折线的边集合（补上收尾边）。
    fn closing(poly: &[Vec2]) -> Vec<Vec2> {
        let mut v = poly.to_vec();
        v.push(poly[0]);
        v
    }

    #[test]
    fn stroke_roundtrip_with_transform() {
        let width = 2.0;
        let poly = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(4.0, 0.0),
            Vec2::new(4.0, 4.0),
        ];
        // 缩放 3 倍 + 平移 (5, -2)：宽度按输出空间解释，故路径点同样被变换。
        let transform = Transform2D::scale(Vec2::splat(3.0))
            .compose(Transform2D::translate(Vec2::new(5.0, -2.0)));
        let shape = Shape2D::Path(Path2D::from_polyline(&poly, false));
        let t = &tessellate(&shape, &transform, &style_stroke(width), 0.05)[0];

        let expected: Vec<Vec2> = poly.iter().map(|p| transform.apply_point(*p)).collect();
        for v in &t.vertices {
            let center = position_on_path_of(v, width);
            assert!(
                dist_to_polyline(center, &expected) < 1e-3,
                "变换后反算中心点偏离路径"
            );
        }
    }

    // ── 几何性质 ──────────────────────────────────────────────────────────

    #[test]
    fn rect_stroke_area_matches_ring() {
        let (w, h, lw) = (10.0f32, 10.0f32, 2.0f32);
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(w, h)));
        let t = &tessellate(&shape, &identity(), &style_stroke(lw), 0.05)[0];
        let expected = (w + lw) * (h + lw) - (w - lw) * (h - lw);
        let got = total_area(t);
        assert!(
            (got - expected).abs() <= expected * 0.05,
            "描边三角化面积 {got} 应≈外框-内框 {expected}"
        );
    }

    #[test]
    fn fill_area_matches_rect_area() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(10.0, 4.0)));
        let t = &tessellate(&shape, &identity(), &style_fill(), 0.05)[0];
        let got = total_area(t);
        assert!((got - 40.0).abs() < 1.0, "填充面积 {got} ≈ 40");
    }

    // ── 索引有效性 ────────────────────────────────────────────────────────

    #[test]
    fn indices_are_valid_and_non_degenerate() {
        let shapes = [
            Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(8.0, 5.0))),
            Shape2D::Circle {
                center: Vec2::new(1.0, 2.0),
                radius: 3.0,
            },
            Shape2D::Path(Path2D::from_polyline(
                &[
                    Vec2::new(0.0, 0.0),
                    Vec2::new(6.0, 0.0),
                    Vec2::new(6.0, 3.0),
                    Vec2::new(0.0, 3.0),
                ],
                true,
            )),
            Shape2D::Line {
                start: Vec2::ZERO,
                end: Vec2::new(5.0, 5.0),
            },
        ];
        for shape in &shapes {
            for style in [style_fill(), style_stroke(1.5)] {
                for t in tessellate(shape, &identity(), &style, 0.05) {
                    assert_eq!(t.indices.len() % 3, 0, "索引数必须是 3 的倍数");
                    for &i in &t.indices {
                        assert!((i as usize) < t.vertices.len(), "索引越界");
                    }
                    for tri in t.indices.chunks_exact(3) {
                        let a = t.vertices[tri[0] as usize].position;
                        let b = t.vertices[tri[1] as usize].position;
                        let c = t.vertices[tri[2] as usize].position;
                        let area = Triangle2D::new(
                            Vec2::new(a[0], a[1]),
                            Vec2::new(b[0], b[1]),
                            Vec2::new(c[0], c[1]),
                        )
                        .area();
                        assert!(area > 1e-9, "退化三角形 area={area}");
                    }
                }
            }
        }
    }

    // ── 填充槽位（文档化行为） ────────────────────────────────────────────

    #[test]
    fn fill_slots_are_zero() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(10.0, 10.0)));
        let t = &tessellate(&shape, &identity(), &style_fill(), 0.05)[0];
        assert_eq!(t.kind, TessKind::Fill);
        assert!(!t.vertices.is_empty());
        for v in &t.vertices {
            assert_eq!(v.normal, [0.0, 0.0]);
            assert_eq!(v.side, 0.0);
        }
    }

    /// 同时有 fill 与 stroke 时产出两项（Fill 在前）。
    #[test]
    fn fill_and_stroke_produce_two_items() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(10.0, 10.0)));
        let style = Style2D {
            fill: Some(Color32::WHITE),
            stroke: Some(Color32::BLACK),
            stroke_width: 2.0,
            space: MeasureSpace::Screen,
        };
        let out = tessellate(&shape, &identity(), &style, 0.05);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].kind, TessKind::Fill);
        assert_eq!(out[1].kind, TessKind::Stroke);
    }

    // ── Group / Empty ─────────────────────────────────────────────────────

    #[test]
    fn group_expands_into_multiple_items() {
        let group = Shape2D::Group {
            children: vec![
                Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(2.0, 2.0))),
                Shape2D::Circle {
                    center: Vec2::new(5.0, 5.0),
                    radius: 1.0,
                },
            ],
        };
        let out = tessellate(&group, &identity(), &style_fill(), 0.05);
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn group_skips_empty_children() {
        let group = Shape2D::Group {
            children: vec![Shape2D::Empty, Shape2D::Empty],
        };
        assert!(tessellate(&group, &identity(), &style_fill(), 0.05).is_empty());
    }

    #[test]
    fn empty_returns_no_items() {
        assert!(tessellate(&Shape2D::Empty, &identity(), &style_stroke(1.0), 0.05).is_empty());
    }

    /// 零宽度描边不产出几何（避免退化条带）。
    #[test]
    fn zero_width_stroke_produces_nothing() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(4.0, 4.0)));
        assert!(tessellate(&shape, &identity(), &style_stroke(0.0), 0.05).is_empty());
    }

    // ── Circle / Ellipse / Polygon ────────────────────────────────────────

    /// 圆折线近似的顶点都落在半径附近（与 `tolerance` 同量级的偏差）。
    #[test]
    fn circle_approximation_stays_within_tolerance() {
        let (c, r) = (Vec2::new(2.0, -1.0), 5.0f32);
        let shape = Shape2D::Circle { center: c, radius: r };
        let tol = 0.05;
        let t = &tessellate(&shape, &identity(), &style_stroke(1.0), tol)[0];
        for v in &t.vertices {
            let center = position_on_path_of(v, 1.0);
            let d = (center - c).length();
            assert!((d - r).abs() <= tol + 1e-3, "半径偏差 {} (d={d})", (d - r).abs());
        }
    }

    #[test]
    fn ellipse_and_polygon_tessellate() {
        let ellipse = Shape2D::Ellipse {
            center: Vec2::ZERO,
            rx: 4.0,
            ry: 2.0,
        };
        let fill = &tessellate(&ellipse, &identity(), &style_fill(), 0.05)[0];
        // 椭圆面积 ≈ π·rx·ry
        assert!((total_area(fill) - std::f32::consts::PI * 8.0).abs() < 0.5);

        let polygon = Shape2D::Polygon {
            points: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(4.0, 0.0),
                Vec2::new(4.0, 4.0),
                Vec2::new(0.0, 4.0),
            ],
        };
        let pf = &tessellate(&polygon, &identity(), &style_fill(), 0.05)[0];
        assert!((total_area(pf) - 16.0).abs() < 0.1);
    }

    // ── 容差自适应 ────────────────────────────────────────────────────────

    #[test]
    fn tolerance_decreases_with_scale_and_clamps() {
        assert!(tolerance_for_scale(1.0) > tolerance_for_scale(10.0));
        assert!(tolerance_for_scale(10.0) > tolerance_for_scale(100.0));
        // 上限 1.0（极小 scale）
        assert_eq!(tolerance_for_scale(0.0), 1.0);
        assert_eq!(tolerance_for_scale(1e-9), 1.0);
        // 下限 1e-4（极大 scale）
        assert_eq!(tolerance_for_scale(1e9), 1e-4);
        // 0.25 / scale 的中段
        assert!((tolerance_for_scale(1.0) - 0.25).abs() < 1e-6);
    }

    // ── Custom 降级钩子 ───────────────────────────────────────────────────

    #[test]
    fn custom_to_triangles_is_adopted() {
        let data = CustomShapeData::new(
            std::any::TypeId::of::<u32>(),
            Rect2D::new(Vec2::ZERO, Vec2::new(2.0, 2.0)),
        )
        .with_to_triangles(|_, _| {
            vec![Triangle2D::new(
                Vec2::ZERO,
                Vec2::new(2.0, 0.0),
                Vec2::new(0.0, 2.0),
            )]
        });
        let t = &tessellate(&Shape2D::Custom(data), &identity(), &style_fill(), 0.05)[0];
        assert_eq!(t.kind, TessKind::Fill);
        assert_eq!(t.indices.len(), 3);
        assert!((total_area(t) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn custom_to_path_falls_back_to_path_tessellation() {
        let data = CustomShapeData::new(
            std::any::TypeId::of::<u32>(),
            Rect2D::new(Vec2::ZERO, Vec2::new(4.0, 4.0)),
        )
        .with_to_path(|_, _| Path2D::from_polyline(&[Vec2::ZERO, Vec2::new(4.0, 0.0)], false));
        let out = tessellate(&Shape2D::Custom(data), &identity(), &style_stroke(1.0), 0.05);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].kind, TessKind::Stroke);
    }

    #[test]
    fn custom_without_hooks_produces_nothing() {
        let data = CustomShapeData::new(
            std::any::TypeId::of::<u32>(),
            Rect2D::new(Vec2::ZERO, Vec2::new(1.0, 1.0)),
        );
        assert!(tessellate(&Shape2D::Custom(data), &identity(), &style_fill(), 0.05).is_empty());
    }
}
