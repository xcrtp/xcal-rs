// tessellate.rs
//
// S3-0：lyon 三角化适配层（纯 CPU、headless，无 wgpu / egui 依赖）。
//
// 目标：`Shape2D + Transform2D + Style2D` → 索引化顶点缓冲，顶点结构**严格**符合
// 自有顶点契约（[`VertexLayout::standard`]，R0 后 stride = 24，四槽
// position/attrib0/attrib1/attrib2）。
//
// # 顶点槽语义（`AaModel::Analytic` 口径）
//
// ```text
// position [f32;2] @0   ← lyon `StrokeVertex::position_on_path()`（**路径上的点，不含宽度**）
// normal   [f32;2] @8   ← lyon `StrokeVertex::normal()`（含 miter 缩放，未归一化）
// side     f32     @16  ← lyon `Side` → ±1.0
// advance  f32     @20  ← lyon `StrokeVertex::advancement()` **归一化** → s ∈ [0,1]
// ```
//
// `advance`（R0 新增，填上 `aa::AttribSemantic::Advance` 这条预留语义）是
// **书写动画（reveal）**的载体：几何帧间不变，逐帧变的只有区间 `[a,b]`（走 uniform），
// 故稳态零上传。归一化口径见 [`tessellate_reveal`] 与 [`normalize_advance`]。
//
// # 描边几何参数化（R1）
//
// 描边宽度**不进几何**：光栅化位置由 `shape_2d.wgsl` 现场补偿
//
// ```text
// 轮廓点 = position + normal * (stroke_width * 0.5)
// ```
//
// （lyon 的 `normal` 长度定义为"沿它位移 1.0 即把描边撑粗 2.0"，故乘半宽即还原
// lyon 原本烘进几何的轮廓点；**不得归一化 `normal`**，它已含 miter 缩放。）
//
// 由此几何与 `stroke_width` **无关**（`stroke_geometry_is_independent_of_width` 锁死），
// 进而与 `camera.scale` 无关 —— 这是"顶点布局声明 PerFrame 是诚实的"的前提。
// 填充顶点 `normal = [0,0]` → 偏移恒为 0（`fill_vertices_are_unaffected_by_offset` 锁死）。
//
// 宽度仍由 CPU 侧**唯一换算点**（`draw.rs::resolve_style`）解析为屏幕像素，
// 经 uniform 交给 shader —— shader 只做几何偏移，不做空间换算（`MeasureSpace` 语义不变）。
//
// # 坐标系
//
// 几何在 **`transform` 作用之后的空间**（输出空间）；uniform 里的 `stroke_width`
// 也按**同一空间**解释。
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

use xc_notion::{Path2D, Transform2D, Triangle2D, Vec2};

use crate::shape::{CustomShapeData, Shape2D, Style2D};
use crate::vertex::{AaModel, CacheStrategy, VertexLayout};

/// 书写动画（reveal）的默认细分步长：**输出空间单位**（= 屏幕像素）。
///
/// lyon **不细分直线段**（`tolerance` 只用于展平曲线），而 reveal 的切口精度
/// = 顶点间距：若不预细分，`sin` 曲线的 128 段折线在放大后每段几十像素，
/// 切口会出现**几十像素的可见阶梯**。2 px 与 `tolerance_for_scale` 的口径同量级。
pub const REVEAL_STEP_PX: f32 = 2.0;

/// 描边/填充顶点：与顶点契约的四槽 1:1。
///
/// # Safety / 布局
///
/// `#[repr(C)]` + 四槽 `[f32;2] / [f32;2] / f32 / f32` → size 24、align 4，
/// 与 [`VertexLayout::standard`] 的
/// `position@0 / attrib0@8 / attrib1@16 / attrib2@20 / stride 24`
/// 完全一致（编译期断言见下方 `const _`）。
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex2D {
    /// 路径上的点（**不含宽度偏移**）→ 契约槽 `position`。
    ///
    /// 光栅化位置由 shader 按 `position + normal * (stroke_width * 0.5)` 还原
    /// （[`contour_position_of`] 是同口径的 CPU 版）。
    pub position: [f32; 2],
    /// 边法线（含 miter 缩放，未归一化）→ 契约槽 `attrib0`。
    pub normal: [f32; 2],
    /// 边侧符号 ±1.0（填充恒 0.0）→ 契约槽 `attrib1`。
    pub side: f32,
    /// **归一化弧长** `s ∈ [0, 1]`（填充恒 0.0）→ 契约槽 `attrib2`。
    ///
    /// 由 lyon 的 `advancement()`（沿路径的累计推进量）除以本条几何的最大推进量得到
    /// —— **多子路径累加**（见 `normalize_advance` 的说明），故书写顺序是
    /// "子轮廓依次写出"。reveal 区间 `[a, b]` 与该值比较决定可见性。
    pub advance: f32,
}

impl Vertex2D {
    /// 填充顶点：`normal` / `side` / `advance` 无 AA 语义，恒为 0（**文档化行为**）。
    pub const FILL_SLOT_ZERO: ([f32; 2], f32, f32) = ([0.0, 0.0], 0.0, 0.0);
}

// 编译期断言：顶点布局与契约 stride 一致（计划 §4.3 硬要求）。
const _: () = assert!(std::mem::size_of::<Vertex2D>() == 24);

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
    /// 顶点布局声明（恒为 [`VertexLayout::standard`]`(AaModel::Analytic, CacheStrategy::PerFrame)`）。
    pub layout: VertexLayout,
    pub kind: TessKind,
}

impl Tessellation2D {
    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }
}

/// 顶点契约布局（本阶段固定）：`Analytic` 四槽 + `PerFrame` 缓存策略。
///
/// 用 `Analytic` 而非 `None`：布局**四槽相同**，将来从 MSAA 切解析 AA 只需换 shader
/// （`attrib2` = `Advance` 也正是解析 AA 模型下声明的语义，见 `xc_vertex::aa`）。
///
/// # `CacheStrategy` 在 2D 路径**仅作声明**
///
/// R1 后几何不含宽度，但仍依赖 `transform`（屏幕空间）与动画内容，故声明 `PerFrame`
/// 才诚实（G7 接 `StaticConsistencyChecker` 时不会把它判成"撒谎"）。
///
/// 不过 `wgpu_2d.rs` 的缓存决策**只读内容哈希**
/// （`ResourceKey::content::<Geometry2DTag>(hash_tessellation(&tess))`），
/// **从不读这个标签** —— 也就是说：改它**不改变任何缓存行为**，改的只是"声明与语义一致"。
pub fn tessellation_layout() -> VertexLayout {
    VertexLayout::standard(AaModel::Analytic, CacheStrategy::PerFrame)
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
    let duplicated =
        points.len() >= 3 && (points[0] - points[points.len() - 1]).length() <= closed_eps();
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
    let map =
        |pts: &[Vec2]| -> Vec<Vec2> { pts.iter().map(|p| transform.apply_point(*p)).collect() };

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
        // `Shape2D` 是 `#[non_exhaustive]`（定义已下沉 kernel/xc-render-ir）：
        // 新增变体时必须在此显式接线，否则该变体静默不产生几何。
        _ => Vec::new(),
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

/// 填充顶点：`normal = [0,0]`、`side = 0`、`advance = 0`
/// （无解析 AA / reveal 语义，文档化行为）。
struct FillCtor;

impl FillVertexConstructor<Vertex2D> for FillCtor {
    fn new_vertex(&mut self, vertex: FillVertex) -> Vertex2D {
        let p = vertex.position();
        let (normal, side, advance) = Vertex2D::FILL_SLOT_ZERO;
        Vertex2D {
            position: [p.x, p.y],
            normal,
            side,
            advance,
        }
    }
}

/// 描边顶点：`position` 取 lyon 的**路径点**（不含宽度），`normal`/`side` 供 shader 补偿，
/// `advance` 取 lyon 的**原始推进量**（由 [`normalize_advance`] 事后归一化到 `[0,1]`）。
struct StrokeCtor;

impl StrokeVertexConstructor<Vertex2D> for StrokeCtor {
    fn new_vertex(&mut self, vertex: StrokeVertex) -> Vertex2D {
        // R1 几何参数化：**不**取 `position()`（它含宽度偏移），宽度由 shader 现场补偿。
        let p = vertex.position_on_path();
        // 保持 lyon 原样：含 miter 缩放，**不归一化**（归一化会在拐角产生错误收缩）。
        let n = vertex.normal();
        let side = if vertex.side().is_positive() {
            1.0
        } else {
            -1.0
        };
        Vertex2D {
            position: [p.x, p.y],
            normal: [n.x, n.y],
            side,
            // lyon `stroke.rs:1359`：新子路径的起点推进量 = 上一条子路径末点的推进量
            // ⇒ **跨子路径累加**（不重置），故下面除以全局最大值即"轮廓依次书写"。
            advance: vertex.advancement(),
        }
    }
}

/// 把 `advance` 归一化到 `[0, 1]`：除以本条几何的最大推进量。
///
/// # 为什么除以"实测最大值"而不是自己算的弧长
///
/// lyon 的 `advancement` 有两条我们**不复制**的内部口径：① 起止点距离过近时
/// 丢点（`square_merge_threshold`）；② 闭合子路径的收尾边如何计入。
/// 除以实测最大值 ⇒ 与 lyon 的口径**同源**，不依赖我对它的推断；
/// 且归一化后 `advance` 恰为 `[0,1]`，reveal 的 `[a,b]` 语义与几何尺度无关。
///
/// 全零（退化几何）时原样保留（不产生 NaN）。
fn normalize_advance(vertices: &mut [Vertex2D]) {
    let total = vertices.iter().map(|v| v.advance).fold(0.0f32, f32::max);
    if total > 1e-9 {
        for v in vertices.iter_mut() {
            v.advance = (v.advance / total).clamp(0.0, 1.0);
        }
    }
}

/// 按固定弧长步长细分轮廓点列（**只补点，原有点全部保留**）。
///
/// lyon 完全不细分直线段，故 reveal 想拿到细粒度的 `s` 必须在**进 lyon 之前**补点。
/// `closed` 时把"回到起点"的那一段也算进去（输出仍不含重复的尾点，与
/// [`contour_of`] 的口径一致）。
fn subdivide_by_step(points: &[Vec2], step: f32, closed: bool) -> Vec<Vec2> {
    if points.len() < 2 {
        return points.to_vec();
    }
    let step = step.max(1e-3);
    let mut out = Vec::with_capacity(points.len() * 2);
    // 首点必须在列首（下面各段只负责"段内插值 + 段端点"）。
    out.push(points[0]);
    let seg_count = if closed {
        points.len()
    } else {
        points.len() - 1
    };
    for i in 0..seg_count {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        let d = b - a;
        let len = d.length();
        // 至少 1 段（零长边也要把端点 b 推进去，否则会丢点）。
        let n = ((len / step).ceil() as usize).max(1);
        for k in 1..=n {
            out.push(a + d * (k as f32 / n as f32));
        }
    }
    if closed {
        // 最后一段的终点恰是起点 ⇒ 去掉重复尾点（与"不含重复尾点"的口径对齐）。
        out.pop();
    }
    out
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
                let (normal, side, advance) = Vertex2D::FILL_SLOT_ZERO;
                indices.push(vertices.len() as u32);
                vertices.push(Vertex2D {
                    position: [p.x, p.y],
                    normal,
                    side,
                    advance,
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
        return tessellate(
            &Shape2D::Path(path),
            &Transform2D::identity(),
            style,
            tolerance,
        );
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
    // R0：把 lyon 的原始推进量归一化为 `s ∈ [0,1]`（无 reveal 时无副作用，见 `normalize_advance`）。
    normalize_advance(&mut buffers.vertices);
    Some(Tessellation2D {
        vertices: buffers.vertices,
        indices: buffers.indices,
        layout: tessellation_layout(),
        kind: TessKind::Stroke,
    })
}

/// **书写动画（reveal）专用**三角化：路径按 `step`（输出空间单位）预细分后，
/// 每个顶点携带归一化弧长 `advance ∈ [0,1]`。
///
/// 与 [`tessellate`] 的关系：几何口径完全一致（同一个 [`tessellate_stroke`]），
/// 只多了一步"进 lyon 之前按弧长补点"。**为什么必须补点**：lyon 不细分直线段，
/// 切口精度 = 顶点间距（见 [`REVEAL_STEP_PX`]）。
///
/// **只做描边**：`FillVertex` 没有 `advancement`，填充路径的 `s` 无从获取
/// （计划 §6 不做项 1；书写动画本来就是描边语义）。
///
/// 返回 `None`：非描边样式、零/负宽度、或路径不产生几何。
pub fn tessellate_reveal(
    path: &Path2D,
    transform: &Transform2D,
    style: &Style2D,
    tolerance: f32,
    step: f32,
) -> Option<Tessellation2D> {
    if style.stroke.is_none() || style.stroke_width <= 0.0 {
        return None;
    }
    let shape = Shape2D::Path(path.clone());
    let contours: Vec<Contour> = contours_of(&shape, transform, tolerance)
        .into_iter()
        .map(|c| Contour {
            points: subdivide_by_step(&c.points, step, c.closed),
            closed: c.closed,
        })
        .collect();
    if contours.is_empty() {
        return None;
    }
    tessellate_stroke(&contours, style.stroke_width)
}

// ─── 测试辅助（也供集成测试/下游复用） ─────────────────────────────────────

/// 顶点的**路径点**（`position` 槽本身）—— 顶点落在原始路径上。
///
/// R1 之前 `position` 是 lyon 的轮廓点（含宽度），此函数用
/// `position - normal * (w * 0.5)` 反算路径点；R1 参数化后 `position` 已是
/// lyon `position_on_path()`，反算不再需要，故直接返回 ——
/// 断言因此直接表达"顶点落在路径上"这个**真实意图**，不依赖宽度。
pub fn position_on_path_of(v: &Vertex2D) -> Vec2 {
    Vec2::new(v.position[0], v.position[1])
}

/// 顶点的**光栅化位置**：`position + normal * (stroke_width * 0.5)`。
///
/// 与 `shape_2d.wgsl` 的顶点偏移**同一口径**（CPU 版镜像）。
/// 填充顶点 `normal = [0,0]` → 恒等于 `position`。
pub fn contour_position_of(v: &Vertex2D, stroke_width: f32) -> Vec2 {
    let k = stroke_width * 0.5;
    Vec2::new(
        v.position[0] + v.normal[0] * k,
        v.position[1] + v.normal[1] * k,
    )
}

/// 顶点三元组的三角形总面积（光栅化面积统计用）。
///
/// 参数化后顶点存的是**路径点**，故面积必须按 shader 的偏移公式还原轮廓，
/// 否则描边的三角形会全部退化成 0（`stroke_width` 对填充无影响，传 0.0 即可）。
pub fn total_area(t: &Tessellation2D, stroke_width: f32) -> f32 {
    t.indices
        .chunks_exact(3)
        .map(|tri| {
            let a = contour_position_of(&t.vertices[tri[0] as usize], stroke_width);
            let b = contour_position_of(&t.vertices[tri[1] as usize], stroke_width);
            let c = contour_position_of(&t.vertices[tri[2] as usize], stroke_width);
            Triangle2D::new(a, b, c).area()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_notion::{Color32, MeasureSpace, Path2D, Rect2D};

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
    fn vertex2d_size_is_24() {
        assert_eq!(std::mem::size_of::<Vertex2D>(), 24);
        assert_eq!(std::mem::align_of::<Vertex2D>(), 4);
    }

    #[test]
    fn stroke_vertices_match_contract_layout() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(10.0, 6.0)));
        let out = tessellate(&shape, &identity(), &style_stroke(2.0), 0.05);
        assert_eq!(out.len(), 1);
        let t = &out[0];
        assert_eq!(t.kind, TessKind::Stroke);
        assert_eq!(
            t.layout,
            VertexLayout::standard(AaModel::Analytic, CacheStrategy::PerFrame)
        );
        assert_eq!(t.layout.stride, 24);
        assert_eq!(t.layout.attribs.len(), 4);
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
            // R1 后 `position` 就是路径点，这里直接断言"顶点落在原路径上"。
            let center = position_on_path_of(v);
            max_err = max_err.max(dist_to_polyline(center, &closing(&poly)));
        }
        assert!(max_err < 1e-3, "顶点偏离原始路径 {max_err}");
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
            let center = position_on_path_of(v);
            assert!(
                dist_to_polyline(center, &expected) < 1e-3,
                "变换后顶点偏离路径"
            );
        }
    }

    // ── R2/R3 门卫：几何与宽度无关（参数化护栏） ──────────────────────────

    /// 🔴 门卫（R2）：同一路径、不同宽度三角化 → **位置与拓扑完全一致**。
    ///
    /// 没有这条测试，R1（`position()` → `position_on_path()`）无法证明真的生效，
    /// 将来也会悄悄退化回"宽度烘进几何"（那会让几何重新依赖 `camera.scale`，
    /// 并使布局声明 `PerFrame` 失去意义）。
    ///
    /// # 关于 `normal` 不逐位相等（实测发现，非回归）
    ///
    /// lyon 把法线定义为"偏移量 ÷ 半宽"（`lyon_tessellation/src/stroke.rs:2037`
    /// `normal = (pos - position_on_path) / half_width`），故不同宽度下
    /// **末位**会因浮点除法舍入差 ~1e-7（实测：0.97628117 vs 0.9762814）。
    /// 这不改变任何几何语义 —— 按 lyon 的恒等式
    /// `position() = position_on_path() + normal() * half_width`（同文件 :2673）
    /// 用各自宽度乘回去，得到的轮廓点仍**逐位一致**（见
    /// [`shader_offset_reproduces_lyon_contour_points`]）。故此处按相对容差断言。
    #[test]
    fn stroke_geometry_is_independent_of_width() {
        let poly = vec![
            Vec2::new(0.0, 0.0),
            Vec2::new(20.0, 0.0),
            Vec2::new(20.0, 12.0),
        ];
        let cases = [
            Shape2D::Path(Path2D::from_polyline(&poly, false)),
            Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(8.0, 5.0))),
            Shape2D::Circle {
                center: Vec2::new(3.0, 4.0),
                radius: 6.0,
            },
        ];
        for shape in &cases {
            let thin = &tessellate(shape, &identity(), &style_stroke(2.0), 0.05)[0];
            let thick = &tessellate(shape, &identity(), &style_stroke(8.0), 0.05)[0];
            assert_eq!(
                thin.vertices.len(),
                thick.vertices.len(),
                "顶点数不得依赖宽度"
            );
            assert_eq!(thin.indices, thick.indices, "索引（拓扑）不得依赖宽度");
            for (a, b) in thin.vertices.iter().zip(&thick.vertices) {
                assert_eq!(a.position, b.position, "路径点不得依赖宽度");
                assert_eq!(a.side, b.side, "side 不得依赖宽度");
                // normal 见上文：按 lyon 的"偏移÷半宽"口径，末位可差 ~1e-7。
                assert!(
                    (a.normal[0] - b.normal[0]).abs() < 1e-5
                        && (a.normal[1] - b.normal[1]).abs() < 1e-5,
                    "normal 不得依赖宽度（除末位舍入）：{a:?} vs {b:?}"
                );
            }
        }
    }

    /// 🔴 R3 的代码级证明（"与旧管线画面一致"）：shader 的偏移公式**逐顶点重建**
    /// R1 之前 lyon 直接给出的轮廓点。
    ///
    /// lyon 的 `position()` 本身就是 `position_on_path() + normal() * half_width`
    /// （`lyon_tessellation/src/stroke.rs:2673`），而 shader 算的是
    /// `position + normal * (params.x * 0.5)`，`params.x` 就是同一个 `stroke_width`
    /// —— 两者**同源**。这条测试把该恒等式钉在数值层：
    /// 若通过，则 GPU 2D 管线光栅化的顶点与旧 CPU 烘焙几何是同一批点，
    /// "画面一致"不需要只靠肉眼。
    #[test]
    fn shader_offset_reproduces_lyon_contour_points() {
        /// R1 之前的口径：直接取 lyon 的轮廓点。
        struct LegacyCtor;
        impl StrokeVertexConstructor<Vertex2D> for LegacyCtor {
            fn new_vertex(&mut self, vertex: StrokeVertex) -> Vertex2D {
                let p = vertex.position();
                let n = vertex.normal();
                let side = if vertex.side().is_positive() {
                    1.0
                } else {
                    -1.0
                };
                Vertex2D {
                    position: [p.x, p.y],
                    normal: [n.x, n.y],
                    side,
                    // R0 新增槽与旧口径无关（这里只比对轮廓点）。
                    advance: vertex.advancement(),
                }
            }
        }

        let poly = [
            Vec2::new(0.0, 0.0),
            Vec2::new(9.0, 0.0),
            Vec2::new(9.0, 7.0),
        ];
        let cases: [(Shape2D, f32); 4] = [
            (
                Shape2D::Line {
                    start: Vec2::ZERO,
                    end: Vec2::new(10.0, 4.0),
                },
                3.0,
            ),
            (
                Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(8.0, 5.0))),
                2.0,
            ),
            (
                Shape2D::Circle {
                    center: Vec2::new(3.0, 4.0),
                    radius: 6.0,
                },
                1.5,
            ),
            (Shape2D::Path(Path2D::from_polyline(&poly, false)), 4.0),
        ];

        for (shape, w) in &cases {
            let contours = contours_of(shape, &identity(), 0.05);
            let path = build_lyon_path(&contours);
            let mut buffers: VertexBuffers<Vertex2D, u32> = VertexBuffers::new();
            StrokeTessellator::new()
                .tessellate_path(
                    &path,
                    &StrokeOptions::default().with_line_width(*w),
                    &mut BuffersBuilder::new(&mut buffers, LegacyCtor),
                )
                .expect("legacy 三角化失败");
            let legacy = buffers.vertices;

            let new = &tessellate(shape, &identity(), &style_stroke(*w), 0.05)[0].vertices;
            assert_eq!(legacy.len(), new.len(), "顶点数应与旧管线一致");
            for (l, n) in legacy.iter().zip(new) {
                let r = contour_position_of(n, *w);
                assert!(
                    (r.x - l.position[0]).abs() < 1e-6 && (r.y - l.position[1]).abs() < 1e-6,
                    "shader 偏移未还原旧轮廓点：{r:?} vs {:?}",
                    l.position
                );
            }
        }
    }

    /// 🔴 门卫（R2）：填充顶点经 shader 偏移公式后**不变**（`normal = [0,0]`）。
    ///
    /// 参数化把宽度交给顶点法线，若将来有人给填充也填 normal，
    /// 填充会被"膨胀"——这条测试把它挡在门外。
    #[test]
    fn fill_vertices_are_unaffected_by_offset() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(10.0, 6.0)));
        let t = &tessellate(&shape, &identity(), &style_fill(), 0.05)[0];
        assert!(!t.vertices.is_empty());
        for v in &t.vertices {
            let raw = Vec2::new(v.position[0], v.position[1]);
            for w in [0.0f32, 2.0, 8.0, 1e3] {
                assert_eq!(contour_position_of(v, w), raw, "填充顶点不得被宽度偏移");
            }
        }
    }

    /// 🔴 R3：描边面积随宽度**线性**增长（线段 = 长 × 宽的矩形）。
    ///
    /// 半宽/全宽、屏幕像素/世界单位搞错一倍，会在这里直接暴露。
    #[test]
    fn stroke_area_scales_linearly_with_width() {
        let shape = Shape2D::Line {
            start: Vec2::ZERO,
            end: Vec2::new(10.0, 0.0),
        };
        let thin = &tessellate(&shape, &identity(), &style_stroke(2.0), 0.05)[0];
        let thick = &tessellate(&shape, &identity(), &style_stroke(8.0), 0.05)[0];
        let a_thin = total_area(thin, 2.0);
        let a_thick = total_area(thick, 8.0);
        // 长 10、宽 2 → 20；长 10、宽 8 → 80（butt 端帽不外扩）。
        assert!(
            (a_thin - 20.0).abs() < 1e-3,
            "宽 2 面积应为 20，实际 {a_thin}"
        );
        assert!(
            (a_thick - 80.0).abs() < 1e-3,
            "宽 8 面积应为 80，实际 {a_thick}"
        );
        assert!(
            (a_thick - a_thin * 4.0).abs() < 1e-2,
            "宽度 4 倍 → 面积 4 倍（{a_thin} vs {a_thick}）"
        );
    }

    // ── 几何性质 ──────────────────────────────────────────────────────────

    #[test]
    fn rect_stroke_area_matches_ring() {
        let (w, h, lw) = (10.0f32, 10.0f32, 2.0f32);
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(w, h)));
        let t = &tessellate(&shape, &identity(), &style_stroke(lw), 0.05)[0];
        let expected = (w + lw) * (h + lw) - (w - lw) * (h - lw);
        let got = total_area(t, lw);
        assert!(
            (got - expected).abs() <= expected * 0.05,
            "描边三角化面积 {got} 应≈外框-内框 {expected}"
        );
    }

    #[test]
    fn fill_area_matches_rect_area() {
        let shape = Shape2D::Rect(Rect2D::new(Vec2::ZERO, Vec2::new(10.0, 4.0)));
        let t = &tessellate(&shape, &identity(), &style_fill(), 0.05)[0];
        let got = total_area(t, 0.0);
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
                    // 参数化后顶点存的是路径点：面积必须按 shader 的偏移公式
                    // 还原轮廓，否则描边三角形会全部退化成 0。
                    for tri in t.indices.chunks_exact(3) {
                        let a =
                            contour_position_of(&t.vertices[tri[0] as usize], style.stroke_width);
                        let b =
                            contour_position_of(&t.vertices[tri[1] as usize], style.stroke_width);
                        let c =
                            contour_position_of(&t.vertices[tri[2] as usize], style.stroke_width);
                        let area = Triangle2D::new(a, b, c).area();
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
            // R0：填充不做 reveal ⇒ attrib2 恒 0（文档化行为）。
            assert_eq!(v.advance, 0.0, "填充顶点的 advance 必须为 0");
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

    // ── R0/R1：书写动画（reveal）的弧长参数 ────────────────────────────────

    /// 两个相距很远的子路径（长度 10 与 30），用于验证"跨子路径累加"。
    fn two_subpath_path() -> Path2D {
        let mut p = Path2D::with_capacity(8);
        p.move_to(Vec2::new(0.0, 0.0)).line_to(Vec2::new(10.0, 0.0));
        p.move_to(Vec2::new(0.0, 100.0))
            .line_to(Vec2::new(30.0, 100.0));
        p
    }

    /// `[0,1]` 两端点齐全，且沿路径**单调不减**（s 是推进量，不能回退）。
    #[test]
    fn reveal_advance_spans_unit_interval_monotonically() {
        let path = Path2D::from_polyline(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(30.0, 0.0),
                Vec2::new(30.0, 40.0),
            ],
            false,
        );
        let t = tessellate_reveal(&path, &identity(), &style_stroke(2.0), 0.05, 1.0)
            .expect("描边路径应产出几何");
        assert!(!t.vertices.is_empty());
        for v in &t.vertices {
            assert!(
                (0.0..=1.0).contains(&v.advance),
                "advance 必须在 [0,1]：{}",
                v.advance
            );
        }
        let min = t
            .vertices
            .iter()
            .map(|v| v.advance)
            .fold(f32::MAX, f32::min);
        let max = t
            .vertices
            .iter()
            .map(|v| v.advance)
            .fold(f32::MIN, f32::max);
        assert_eq!(min, 0.0, "起点必须为 0");
        assert_eq!(max, 1.0, "终点必须为 1（归一化到实测最大推进量）");

        // 按 x 排序（第一段沿 +x）⇒ advance 单调不减。
        let mut by_x: Vec<(f32, f32)> = t
            .vertices
            .iter()
            .map(|v| (v.position[0], v.advance))
            .collect();
        by_x.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        for w in by_x.windows(2) {
            assert!(w[1].1 + 1e-6 >= w[0].1, "advance 沿路径不得回退：{:?}", w);
        }
    }

    /// **多子路径累加**（实测结论，`lyon_tessellation/src/stroke.rs:1359`）：
    /// 后一子路径的起点 `advance` = 上一子路径末点的 `advance`（**中间不补"抬笔"距离**）
    /// ⇒ 区间首尾相接、整体递增，书写顺序是"轮廓依次写出"。
    #[test]
    fn reveal_advance_accumulates_across_subpaths() {
        let t = tessellate_reveal(
            &two_subpath_path(),
            &identity(),
            &style_stroke(2.0),
            0.05,
            1.0,
        )
        .expect("双子路径应产出几何");
        let (mut a_max, mut b_min, mut b_max) = (f32::MIN, f32::MAX, f32::MIN);
        for v in &t.vertices {
            if v.position[1].abs() < 1e-3 {
                a_max = a_max.max(v.advance);
            } else if (v.position[1] - 100.0).abs() < 1e-3 {
                b_min = b_min.min(v.advance);
                b_max = b_max.max(v.advance);
            }
        }
        // 有序：B 完全排在 A 之后（首尾在分界处**相接**，故取 `>=`）。
        assert!(
            b_min >= a_max - 1e-6,
            "第二条子路径不得排在第一条之前（{b_min} vs {a_max}）"
        );
        assert!(b_max > a_max, "第二条子路径必须延伸到更后面");
        // 第一段长 10 / 总长 40 ⇒ 分界 ≈ 0.25（子路径间不补"抬笔"距离）。
        assert!((a_max - 0.25).abs() < 0.02, "首段应约占 0.25，实际 {a_max}");
    }

    /// 预细分（y：lyon 不细分直线段）：切口精度 = `step`，与"顶点间距"同量级。
    ///
    /// 断言相邻 `advance` 的**最大间隔 ≤ step / 总弧长** —— 若不做预细分，
    /// 一条 10 长的直线上只有两个顶点，间隔会是 0.25（10/40）而不是 ≤ 0.025。
    #[test]
    fn reveal_pre_subdivision_bounds_cut_precision() {
        let step = 1.0f32;
        let t = tessellate_reveal(
            &two_subpath_path(),
            &identity(),
            &style_stroke(2.0),
            0.05,
            step,
        )
        .expect("双子路径应产出几何");
        let mut s: Vec<f32> = t.vertices.iter().map(|v| v.advance).collect();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        s.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
        assert!(s.len() >= 40, "应补出足够密的采样点，实际 {} 个", s.len());
        let max_gap = s.windows(2).map(|w| w[1] - w[0]).fold(0.0f32, f32::max);
        // 总长 40 ⇒ step=1.0 对应归一化间隔 0.025。
        assert!(
            max_gap <= 0.025 + 1e-3,
            "切口精度应受 step 约束，实际最大间隔 {max_gap}"
        );
    }

    /// reveal 只做描边：填充样式 / 零宽描边不产出几何（与 `tessellate` 同口径）。
    #[test]
    fn reveal_rejects_fill_and_zero_width() {
        let path = Path2D::from_polyline(&[Vec2::ZERO, Vec2::new(10.0, 0.0)], false);
        assert!(tessellate_reveal(&path, &identity(), &style_fill(), 0.05, 1.0).is_none());
        assert!(tessellate_reveal(&path, &identity(), &style_stroke(0.0), 0.05, 1.0).is_none());
        assert!(tessellate_reveal(&path, &identity(), &style_stroke(1.0), 0.05, 1.0).is_some());
    }

    /// 闭合轮廓也带 `advance`（圆环：`s` 绕一圈回到 1，不出现重复尾点导致的 0 回退）。
    #[test]
    fn reveal_handles_closed_contour() {
        let ring: Vec<Vec2> = (0..16)
            .map(|i| {
                let a = std::f32::consts::TAU * (i as f32) / 16.0;
                Vec2::new(10.0 * a.cos(), 10.0 * a.sin())
            })
            .collect();
        let path = Path2D::from_polyline(&ring, true);
        let t = tessellate_reveal(&path, &identity(), &style_stroke(2.0), 0.05, 1.0)
            .expect("闭合描边应产出几何");
        let max = t
            .vertices
            .iter()
            .map(|v| v.advance)
            .fold(f32::MIN, f32::max);
        assert_eq!(max, 1.0, "闭合轮廓的最大推进量也应归一化到 1");
    }

    /// 细分工具本身：零长边不丢点、闭合轮廓不含重复尾点。
    #[test]
    fn subdivide_by_step_keeps_endpoints() {
        // 零长边（两点重合）也必须把第二个点带出来。
        let degenerate = subdivide_by_step(&[Vec2::ZERO, Vec2::ZERO], 1.0, false);
        assert_eq!(degenerate.len(), 2, "零长边不得丢点：{degenerate:?}");
        // 开放折线：首点保留、末点保留。
        let open = subdivide_by_step(&[Vec2::ZERO, Vec2::new(5.0, 0.0)], 1.0, false);
        assert_eq!(open[0], Vec2::ZERO);
        assert_eq!(*open.last().unwrap(), Vec2::new(5.0, 0.0));
        assert_eq!(open.len(), 6, "5 长 / step 1 ⇒ 5 段 6 点");
        // 闭合：末点不得等于首点（重复尾点会与 `builder.close()` 冲突）。
        let closed = subdivide_by_step(
            &[Vec2::ZERO, Vec2::new(4.0, 0.0), Vec2::new(4.0, 4.0)],
            1.0,
            true,
        );
        assert_ne!(closed[0], *closed.last().unwrap(), "闭合轮廓不得有重复尾点");
    }

    /// 🔴 门卫：**未启用 reveal 的既有路径**逐字节不受 R0 影响（除了新增槽的固定值）。
    ///
    /// 同一路径用 `tessellate`（普通路径）与 `tessellate_reveal`（reveal 路径）
    /// 且 `step` 足够小以致不补点（= 与普通路径同样的点列）时，位置/法线/索引必须一致。
    /// 这证明 reveal 只是"多带了 s"，没有改变几何口径。
    #[test]
    fn reveal_matches_plain_tessellation_geometry() {
        let path = Path2D::from_polyline(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(10.0, 0.0),
                Vec2::new(10.0, 10.0),
            ],
            false,
        );
        let style = style_stroke(3.0);
        let plain = &tessellate(&Shape2D::Path(path.clone()), &identity(), &style, 0.05)[0];
        // step 远大于段长 ⇒ 不补点（每段 1 段）。
        let reveal =
            tessellate_reveal(&path, &identity(), &style, 0.05, 1e6).expect("描边路径应产出几何");
        assert_eq!(plain.vertices.len(), reveal.vertices.len());
        assert_eq!(plain.indices, reveal.indices);
        for (a, b) in plain.vertices.iter().zip(&reveal.vertices) {
            assert_eq!(a.position, b.position);
            assert_eq!(a.side, b.side);
            assert_eq!(a.advance, b.advance, "两条路径的 s 归一化口径必须一致");
        }
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
        let shape = Shape2D::Circle {
            center: c,
            radius: r,
        };
        let tol = 0.05;
        let t = &tessellate(&shape, &identity(), &style_stroke(1.0), tol)[0];
        for v in &t.vertices {
            let center = position_on_path_of(v);
            let d = (center - c).length();
            assert!(
                (d - r).abs() <= tol + 1e-3,
                "半径偏差 {} (d={d})",
                (d - r).abs()
            );
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
        assert!((total_area(fill, 0.0) - std::f32::consts::PI * 8.0).abs() < 0.5);

        let polygon = Shape2D::Polygon {
            points: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(4.0, 0.0),
                Vec2::new(4.0, 4.0),
                Vec2::new(0.0, 4.0),
            ],
        };
        let pf = &tessellate(&polygon, &identity(), &style_fill(), 0.05)[0];
        assert!((total_area(pf, 0.0) - 16.0).abs() < 0.1);
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
        assert!((total_area(t, 0.0) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn custom_to_path_falls_back_to_path_tessellation() {
        let data = CustomShapeData::new(
            std::any::TypeId::of::<u32>(),
            Rect2D::new(Vec2::ZERO, Vec2::new(4.0, 4.0)),
        )
        .with_to_path(|_, _| Path2D::from_polyline(&[Vec2::ZERO, Vec2::new(4.0, 0.0)], false));
        let out = tessellate(
            &Shape2D::Custom(data),
            &identity(),
            &style_stroke(1.0),
            0.05,
        );
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
