// typst/convert.rs — Frame → ContentIR。
//
// 覆盖三类 FrameItem：
//   - FrameItem::Text  ：逐个 glyph 取 ttf 字形轮廓；每个 glyph 一个 StyledPath，
//                       并带上它在**原始归一化源码**中的锚点（TypstWorld.span_to_range 驱动）。
//   - FrameItem::Shape ：Geometry 直接转 Path2D（Line / Rect / Curve）。
//   - FrameItem::Group ：递归，累积 transform。
//
// 坐标系约定：Typst frame 坐标为 Y-down，原点在页左上角。
// 为了 IR 语义「baseline y = 0，向上 y 增大」，我们对整页做一次翻转：
//     y_ir = -(y_typst - baseline_guess_from_min_y)
// 实际策略：所有点先按 Y-down 累积；最后把整个 items 的 y 翻转并平移使最小 y → 0
// （这样 bounds 以 (0,0) 为左下角，和 placeholder 语义一致）。

use std::ops::Range;

use ttf_parser::{GlyphId, OutlineBuilder};
use typst::layout::{Abs, Frame, FrameItem, Point, Transform};
use typst::syntax::Span;
use typst::text::TextItem;
use typst::visualize::{CurveItem, FixedStroke, Geometry, Paint};
use xc_content::{ContentIR, ContentSource, Span as XSpan, StyledPath};
use xc_notion::{Color32, Path2D, Stroke2D, Vec2};

use super::world::TypstWorld;
use crate::request::{ContentConfig, ContentError};

const PT_UNIT: typst::layout::AbsUnit = typst::layout::AbsUnit::Pt;

fn abs_to_pt(a: Abs) -> f32 {
    a.to_unit(PT_UNIT) as f32
}

fn point_to_v2(p: Point) -> Vec2 {
    Vec2::new(abs_to_pt(p.x), abs_to_pt(p.y))
}

/// 3x3 仿射矩阵（行优先，与 xc-notion Transform2D 对齐）：
/// [ sx  kx  tx ]
/// [ ky  sy  ty ]
/// [ 0   0   1  ]
#[derive(Debug, Clone, Copy)]
struct M3 {
    pub sx: f32,
    pub kx: f32,
    pub tx: f32,
    pub ky: f32,
    pub sy: f32,
    pub ty: f32,
}

impl M3 {
    const IDENTITY: Self = Self {
        sx: 1.0,
        kx: 0.0,
        tx: 0.0,
        ky: 0.0,
        sy: 1.0,
        ty: 0.0,
    };

    fn from_typst(t: Transform) -> Self {
        // typst Ratio 是纯数（1.0 = 100%），通过 .get() 取得 f64。
        let sx = t.sx.get() as f32;
        let kx = t.kx.get() as f32;
        let ky = t.ky.get() as f32;
        let sy = t.sy.get() as f32;
        let tx = abs_to_pt(t.tx);
        let ty = abs_to_pt(t.ty);
        Self {
            sx,
            kx,
            tx,
            ky,
            sy,
            ty,
        }
    }

    fn translate(x: f32, y: f32) -> Self {
        Self {
            sx: 1.0,
            kx: 0.0,
            tx: x,
            ky: 0.0,
            sy: 1.0,
            ty: y,
        }
    }

    fn mul(self, b: Self) -> Self {
        // self 应用后再应用 b：combined = b ∘ self
        // (x', y') = self (x, y) ; (x'', y'') = b (x', y')
        Self {
            sx: b.sx * self.sx + b.kx * self.ky,
            kx: b.sx * self.kx + b.kx * self.sy,
            tx: b.sx * self.tx + b.kx * self.ty + b.tx,
            ky: b.ky * self.sx + b.sy * self.ky,
            sy: b.ky * self.kx + b.sy * self.sy,
            ty: b.ky * self.tx + b.sy * self.ty + b.ty,
        }
    }

    fn apply(self, p: Vec2) -> Vec2 {
        Vec2::new(
            self.sx * p.x + self.kx * p.y + self.tx,
            self.ky * p.x + self.sy * p.y + self.ty,
        )
    }
}

/// 把 Paint（typst 支持纯色/渐变/贴图，此处只解纯色，其他情况回退为配置颜色）。
fn paint_to_color(p: &Paint, fallback: Color32) -> Option<Color32> {
    match p {
        Paint::Solid(c) => {
            // typst Color → sRGB Rgb struct { red, green, blue, alpha } (f32, 0..=1)。
            let rgb = c.to_rgb();
            let r = (rgb.red * 255.0).clamp(0.0, 255.0) as u8;
            let g = (rgb.green * 255.0).clamp(0.0, 255.0) as u8;
            let b = (rgb.blue * 255.0).clamp(0.0, 255.0) as u8;
            let a = (rgb.alpha * 255.0).clamp(0.0, 255.0) as u8;
            Some(Color32::from_rgba(r, g, b, a))
        }
        _ => Some(fallback),
    }
}

fn fixed_stroke_to_stroke2d(s: &FixedStroke, fallback: Color32) -> Option<Stroke2D> {
    let color = paint_to_color(&s.paint, fallback)?;
    if color.a == 0 || s.thickness.to_raw() <= 0.0 {
        return None;
    }
    Some(Stroke2D::new(color, abs_to_pt(s.thickness)))
}

/// ttf-parser outline builder → Path2D（ttf 单位，之后调用者再缩放）。
struct PathSink {
    path: Path2D,
    // 单位坐标；builder 给出的点是原始字体单位（整数 upem 坐标），
    // 我们原样写入，之后再整体应用 transform。
}

impl PathSink {
    fn new() -> Self {
        Self {
            path: Path2D::new(),
        }
    }
    fn finish(self) -> Path2D {
        self.path
    }
}

impl OutlineBuilder for PathSink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.path.move_to(Vec2::new(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.path.line_to(Vec2::new(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.path.quad_to(Vec2::new(x1, y1), Vec2::new(x, y));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.path
            .cubic_to(Vec2::new(x1, y1), Vec2::new(x2, y2), Vec2::new(x, y));
    }
    fn close(&mut self) {
        self.path.close();
    }
}

/// span（与 main 文件绑定）→ 原始归一化源码中的字节区间。
///
/// 算法：body_offset 是用户原始归一化源码在 wrapped_src 中的字节偏移
///（由 compile 时算好写入 world）。typst range 给出 wrapped_src 坐标 [ws, we)，
/// 直接减去 body_offset 即得原始坐标——O(1) 且精确，不依赖 find 首命中。
/// 若 ws < body_offset（命中包裹层 #set/$ 而非 body），返回 None。
fn span_to_normalized_range(
    world: &TypstWorld,
    span: Span,
    original: &str,
) -> Option<Range<usize>> {
    let main_range = world.span_to_range(span)?;
    let body_offset = world.body_offset();
    // 命中包裹层（#set / $ $）而非 body → 无法映射，返回 None。
    if main_range.start < body_offset {
        return None;
    }
    let orig_start = main_range.start - body_offset;
    let orig_end = main_range.end.saturating_sub(body_offset);
    // clamp 到原始归一化源码长度（越界 → None，锚点可空不阻塞）。
    let len = original.len();
    if orig_start > len || orig_end > len {
        return None;
    }
    Some(orig_start..orig_end)
}

/// 把 Typst CurveItem 序列 → xc-notion Path2D。
fn curve_to_path(curve_items: &[CurveItem]) -> Path2D {
    let mut p = Path2D::with_capacity(curve_items.len());
    for item in curve_items {
        match item {
            CurveItem::Move(pt) => {
                p.move_to(point_to_v2(*pt));
            }
            CurveItem::Line(pt) => {
                p.line_to(point_to_v2(*pt));
            }
            CurveItem::Cubic(c1, c2, end) => {
                p.cubic_to(point_to_v2(*c1), point_to_v2(*c2), point_to_v2(*end));
            }
            CurveItem::Close => {
                p.close();
            }
        };
    }
    p
}

/// 按 M3 变换 Path2D 中所有端点和控制点。
fn transform_path(path: &Path2D, m: M3) -> Path2D {
    let mut out = Path2D::with_capacity(path.commands.len());
    for cmd in &path.commands {
        use xc_notion::PathCommand::*;
        out.commands.push(match *cmd {
            MoveTo(p) => MoveTo(m.apply(p)),
            LineTo(p) => LineTo(m.apply(p)),
            QuadTo { ctrl, end } => QuadTo {
                ctrl: m.apply(ctrl),
                end: m.apply(end),
            },
            CubicTo { ctrl1, ctrl2, end } => CubicTo {
                ctrl1: m.apply(ctrl1),
                ctrl2: m.apply(ctrl2),
                end: m.apply(end),
            },
            Arc {
                center,
                radii,
                rotation,
                start_angle,
                end_angle,
            } => {
                // typst Geometry 曲线不产 Arc 命令；仅 Shape 会产 Line/Rect/Curve，
                // 但 Curve 里没有 Arc。所以这里保守处理：仅对 center 做仿射，
                // radii / 角度不变（一般遇到 Group 里的 Arc 极少）。
                Arc {
                    center: m.apply(center),
                    radii,
                    rotation,
                    start_angle,
                    end_angle,
                }
            }
            Close => Close,
        });
    }
    out
}

fn geometry_to_path(g: &Geometry) -> Path2D {
    match g {
        Geometry::Line(end) => {
            let mut p = Path2D::with_capacity(2);
            p.move_to(Vec2::ZERO);
            p.line_to(point_to_v2(*end));
            p
        }
        Geometry::Rect(size) => {
            let w = abs_to_pt(size.x);
            let h = abs_to_pt(size.y);
            let mut p = Path2D::with_capacity(5);
            p.move_to(Vec2::ZERO);
            p.line_to(Vec2::new(w, 0.0));
            p.line_to(Vec2::new(w, h));
            p.line_to(Vec2::new(0.0, h));
            p.close();
            p
        }
        Geometry::Curve(curve) => curve_to_path(&curve.0),
    }
}

/// 主入口：Frame → ContentIR。
pub fn frame_to_ir(
    world: &TypstWorld,
    frame: Frame,
    src: &ContentSource,
    cfg: &ContentConfig,
) -> Result<ContentIR, ContentError> {
    let original_norm = world.original_source();

    let mut items: Vec<StyledPath> = Vec::new();

    // Typst Y-down → 先收集所有 Y-down 坐标；最后再整体翻转到 Y-up 并 min.y → 0。
    walk_frame(
        world,
        &frame,
        M3::IDENTITY,
        src,
        &original_norm,
        cfg,
        &mut items,
    )?;

    // 先算一次 Y-down bounds 做翻转参考。
    // y_min 仅用于判定有限性：当 bounds 非有限时将 y_max 设为 0 避免 NaN 矩阵。
    let y_min;
    let mut y_max = f32::NEG_INFINITY;
    {
        let mut y_min_mut = f32::INFINITY;
        for it in &items {
            if let Some(b) = it.path.compute_bounds() {
                y_min_mut = y_min_mut.min(b.min.y);
                y_max = y_max.max(b.max.y);
            }
        }
        y_min = y_min_mut;
    }
    if !(y_min.is_finite() && y_max.is_finite()) {
        y_max = 0.0;
    }

    // 翻转：y' = -(y - y_max) = y_max - y  → 原页顶 y_min → y_max - y_min，原页底 y_max → 0。
    // 再平移让最小 y'（= 0）保持 0；最大 y' = y_max - y_min。
    let flip = M3 {
        sx: 1.0,
        kx: 0.0,
        tx: 0.0,
        ky: 0.0,
        sy: -1.0,
        ty: y_max,
    };
    for it in items.iter_mut() {
        it.path = transform_path(&it.path, flip);
    }

    let mut ir = ContentIR::empty(src.clone(), cfg.font_size);
    ir.items = items;
    ir.recompute_bounds();

    // baseline = 原页顶 y_min 翻转后对应 y = y_max - y_min 的位置。
    // 实际上我们没有 Typst 给的 true baseline，这里给一个保守估计：
    // 若只有一行公式，baseline ≈ max.y（即翻转后高度减去 ascent 的 80%）。
    // 为避免瞎猜，我们设 baseline = ir.bounds.max.y * 0.8，
    // 与 placeholder.rs 中的近似相同，足以做对齐。
    let total_h = ir.bounds.size().y;
    ir.baseline = if total_h > 0.0 { total_h * 0.8 } else { 0.0 };

    Ok(ir)
}

fn walk_frame(
    world: &TypstWorld,
    frame: &Frame,
    outer: M3,
    src: &ContentSource,
    original_norm: &str,
    cfg: &ContentConfig,
    out: &mut Vec<StyledPath>,
) -> Result<(), ContentError> {
    for (pos, item) in frame.items() {
        let local = M3::translate(abs_to_pt(pos.x), abs_to_pt(pos.y));
        let combined = outer.mul(local);
        match item {
            FrameItem::Group(g) => {
                let inner = combined.mul(M3::from_typst(g.transform));
                // Note: clip 暂不实现（数学/短文本很少用到）。
                walk_frame(world, &g.frame, inner, src, original_norm, cfg, out)?;
            }
            FrameItem::Text(t) => {
                convert_text(world, combined, t, original_norm, cfg, out);
            }
            FrameItem::Shape(s, span) => {
                let path = geometry_to_path(&s.geometry);
                let path = transform_path(&path, combined);
                let fill = s.fill.as_ref().and_then(|p| paint_to_color(p, cfg.fill));
                let stroke = s
                    .stroke
                    .as_ref()
                    .and_then(|s| fixed_stroke_to_stroke2d(s, cfg.fill));
                let anchor = span_to_normalized_range(world, *span, original_norm)
                    .map(|r| XSpan::new(r.start, r.end));
                // 如果 fill/stroke 同时没给，按计划 1.3 至少保留一种：这里给个 fallback fill。
                let (fill, stroke) = if fill.is_none() && stroke.is_none() {
                    (Some(cfg.fill), None)
                } else {
                    (fill, stroke)
                };
                out.push(StyledPath {
                    path,
                    fill,
                    stroke,
                    anchor,
                });
            }
            FrameItem::Image(..) | FrameItem::Link(..) | FrameItem::Tag(..) => {
                // 数学/短文本主场景：不处理
            }
        }
    }
    Ok(())
}

fn convert_text(
    world: &TypstWorld,
    combined: M3,
    t: &TextItem,
    original_norm: &str,
    cfg: &ContentConfig,
    out: &mut Vec<StyledPath>,
) {
    let fill = paint_to_color(&t.fill, cfg.fill).unwrap_or(cfg.fill);
    let stroke = t
        .stroke
        .as_ref()
        .and_then(|s| fixed_stroke_to_stroke2d(s, cfg.fill));

    let face = t.font.ttf();
    let units_per_em = t.font.units_per_em().max(1.0);
    let size_pt = abs_to_pt(t.size);
    // Em → pt = size_pt * em_value。
    // 字体单位 (upem) → pt = size_pt / units_per_em。
    let font_scale = size_pt / units_per_em as f32;

    // 每个 glyph 我们需要累计 x 位移：
    //   glyph 起点 x = Σ x_advance（before） + x_offset
    // typst 的 TextItem::glyphs 的坐标基准是 TextItem 的 origin：pos.x 已经在 combined 里。
    // 为了使用 ttf 轮廓，我们对每个 glyph 做：
    //   outline 的 y 轴在 ttf 中是 Y-up（origin 左侧，baseline = 0），
    //   但 Typst Y-down 框架把 origin 放在 baseline 上、Y 向下增大，
    //   所以对 ttf 的 y 我们做翻转：y_typst_pt = - y_upem * font_scale + x_offset.y。
    //   最终再叠加 combined（来自 frame）。
    let mut advance_cursor = 0.0f32; // pt 单位累计推进。

    for g in &t.glyphs {
        let xoff_pt = g.x_offset.at(t.size).to_unit(PT_UNIT) as f32;
        let yoff_pt = g.y_offset.at(t.size).to_unit(PT_UNIT) as f32;
        let xadv_pt = g.x_advance.at(t.size).to_unit(PT_UNIT) as f32;

        // 锚点：glyph.span.0 → TypstWorld.span_to_range → 减 body_offset 得原始坐标。
        // body_offset 由 compile 时算好（prefix + "$ " 偏移），O(1) 精确，不依赖 find。
        let anchor = span_to_normalized_range(world, g.span.0, original_norm)
            .map(|r| XSpan::new(r.start, r.end));

        let glyph_transform = M3 {
            // ttf x → 右（正）；font_scale；加上 x 推进和 glyph 自身 offset。
            sx: font_scale,
            kx: 0.0,
            tx: advance_cursor + xoff_pt,
            // ttf y_up 为正 → Typst Y-down 需要翻转（y = -upem*scale），再叠加 glyph yoff。
            ky: 0.0,
            sy: -font_scale,
            ty: yoff_pt,
        }
        .mul(combined);

        let mut sink = PathSink::new();
        // 取不到 outline（字体缺该字形）时跳过，但要留诊断：静默吞掉会导致
        // 公式缺笔画却无从排查。
        if face.outline_glyph(GlyphId(g.id), &mut sink).is_none() {
            tracing::debug!(
                "[xc-prerender::typst] glyph {} 无轮廓（字体缺字形），已跳过",
                g.id
            );
        }
        let local_path = sink.finish();
        if !local_path.commands.is_empty() {
            let path = transform_path(&local_path, glyph_transform);
            out.push(StyledPath {
                path,
                fill: Some(fill),
                stroke: stroke.clone(),
                anchor,
            });
        }

        advance_cursor += xadv_pt;
    }
}
