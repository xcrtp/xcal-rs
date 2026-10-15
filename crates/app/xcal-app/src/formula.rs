// formula.rs — Typst 内容管线 → 旧 2D 描边路径管线的桥接层。
//
// 数据流：
//   ContentIR →(ir_to_draw_cmds)→ ContentDrawCmd
//             →(flatten_subpaths + transform.apply_point)→ Path 组件（Path2D）
//             →(spawn)→ SceneRecordSystem → DrawQueue → EguiPainter。
//
// 走描边（Stroke）而非填充：旧管线 Fill 分支用 `convex_polygon`，非凸字形
// （o/a/e/B 有孔洞与凹角）会被画成凸包，视觉错误。详见执行计划 §2.2。
//
// 本模块是接线，不新增架构抽象：缩放与平移完全复用 `ir_to_draw_cmds`。

use xcal_scene::types::Color32;
use xcal_render::{ContentDrawCmd, MeasureSpace, Shape2D, ir_to_draw_cmds};
use xc_content::ContentIR;
use xc_geometry::Path2D;
use xc_prerender::{ContentConfig, ContentEngine, InMemoryEngine};

/// 曲线展平容差（pt，相对 **标称字号** 尺度）。
///
/// 因为 `rebuild()` 用标称字号（默认 12.0）编译 IR，容差相对字形尺度始终
/// 合理（0.1/12 ≈ 0.8%）；最终显示尺寸缩小不会放大该误差。
const FLATTEN_TOLERANCE: f32 = 0.1;
/// 只有 fill、没有 stroke 时使用的默认描边宽度（**标称 pt**）。
///
/// 需乘 `target / nominal` 换算为世界单位后使用（描边属世界空间）。
const DEFAULT_STROKE_WIDTH_PT: f32 = 1.0;
/// Typst 编译用的标称字号（pt）。曲线提取精度以它为基准；
/// 目标显示大小由 [`FormulaSpec::font_size`] 经 `ir_to_draw_cmds` 缩放得到。
const DEFAULT_NOMINAL_FONT_SIZE: f32 = 12.0;
/// 子路径首尾点距离 ≤ 此值视为闭合。
const CLOSE_EPS: f32 = 1e-4;

/// 公式源描述（静态配置；将来接交互时改为可编辑字段）。
#[derive(Debug, Clone)]
pub struct FormulaSpec {
    /// Typst 源码。
    pub source: String,
    /// 目标显示字号（pt ≈ 世界单位高度）。经 `ir_to_draw_cmds` 缩放到世界坐标。
    pub font_size: f32,
    /// 世界坐标锚点（Y-up；IR 原点位于墨迹包围盒左下方）。
    pub world_pos: glam::Vec2,
    /// Typst 编译用的标称字号（pt）：决定曲线提取精度与 IR 本地点尺度。
    /// 与 [`font_size`](Self::font_size) 解耦——后者只决定最终显示大小。
    pub nominal_font_size: f32,
}

impl FormulaSpec {
    /// 用默认标称字号（12.0）构造。
    pub fn new(source: impl Into<String>, font_size: f32, world_pos: glam::Vec2) -> Self {
        Self {
            source: source.into(),
            font_size,
            world_pos,
            nominal_font_size: DEFAULT_NOMINAL_FONT_SIZE,
        }
    }
}

/// 按 feature 选择引擎：typst 开启时用 Typst 引擎（初始化失败则回退占位引擎），
/// 关闭时用占位引擎（显示方块，链路仍可跑通）。
pub fn build_engine() -> InMemoryEngine {
    #[cfg(feature = "typst")]
    {
        match xc_prerender::typst_engine() {
            Ok(engine) => return engine,
            Err(e) => eprintln!("[xcal-app] typst 引擎初始化失败，回退 placeholder：{e}"),
        }
    }
    xc_prerender::placeholder_engine()
}

/// 编译公式源码为 IR（将来接交互时可在帧内调用；本计划仅启动时调用一次）。
///
/// 注意：`ContentConfig.font_size` 填的是**标称字号**（决定曲线提取精度），
/// 不是目标显示大小；显示用的缩放由 `ir_to_draw_cmds` 的
/// `scale = target / nominal` 完成，勿绕过。
pub fn rebuild(engine: &InMemoryEngine, spec: &FormulaSpec) -> Option<ContentIR> {
    let config = ContentConfig {
        font_size: spec.nominal_font_size,
        fill: Color32::WHITE,
        scale_factor: 1.0,
    };
    let handle = engine.request(&spec.source, &config);
    engine.take(handle).and_then(|r| r.ok())
}

/// 一条可 spawn 的描边路径：`(路径, 颜色, 线宽, 线宽所属空间)`。
///
/// 每个子轮廓单独一条——多轮廓字形（`o`/`B`/`8`）不会在轮廓之间
/// 产生横穿字形内部的桥接线。
///
/// `space` 由 `ir_to_draw_cmds` 声明（当前恒为 `MeasureSpace::World`：
/// 宽度已乘 `target / nominal` 换算为世界单位，与字形一同随缩放变化）。
pub type StrokePath = (Path2D, Color32, f32, MeasureSpace);

/// IR → 可 spawn 的描边路径列表。
///
/// 复用 [`ir_to_draw_cmds`] 完成缩放与平移；再用 `Path2D::flatten_subpaths`
/// 按子路径分组，逐组应用变换落到世界坐标。描边色回退规则见 [`stroke_style`]。
pub fn compile_to_stroke_paths(ir: &ContentIR, spec: &FormulaSpec) -> Vec<StrokePath> {
    // 与 `ir_to_draw_cmds` 相同的换算系数：标称 pt → 世界单位。
    // 用于把"默认细线宽"（以标称 pt 表述）换算到世界空间。
    let scale = if ir.nominal_font_size > 0.0 {
        spec.font_size / ir.nominal_font_size
    } else {
        1.0
    };

    let mut out = Vec::new();
    for cmd in ir_to_draw_cmds(ir, spec.font_size, spec.world_pos) {
        let Shape2D::Path(path) = &cmd.shape else {
            continue;
        };
        let Some((color, width)) = stroke_style(&cmd, scale) else {
            continue;
        };

        for sub in path.flatten_subpaths(FLATTEN_TOLERANCE) {
            if sub.is_empty() {
                continue;
            }
            let closed = is_closed(&sub);
            let world: Vec<glam::Vec2> = sub.iter().map(|p| cmd.transform.apply_point(*p)).collect();
            out.push((Path2D::from_polyline(&world, closed), color, width, cmd.style.space));
        }
    }
    out
}

/// 语义形变（S1b）：按源码锚点配对两个公式的路径，并把每对路径变换到世界坐标。
///
/// - 配对规则见 [`xc_content::pair_by_anchor`]（锚点相同 → 配对；单侧 → 淡入/淡出；
///   无锚点 → 索引配对）；
/// - 变换**复用** `ir_to_draw_cmds` 的 transform（缩放 + 平移），不另写一套。
///
/// **前提**：`from_ir` 与 `to_ir` 必须用**相同标称字号**编译 —— transform 的
/// `scale = target / nominal_font_size` 只取自 `from_ir`，两端 nominal 不同则 `to`
/// 的路径会被错误缩放（不报错，只画错）。
///
/// 产出交给 `Path2D::morph` / `Curve::PathMorph` 逐对插值（配对知识不进 xc-time）。
pub fn compile_morph_pairs(
    from_ir: &ContentIR,
    to_ir: &ContentIR,
    spec: &FormulaSpec,
) -> Vec<(Path2D, Path2D)> {
    // 防护：两端 nominal 必须一致（见上方"前提"）。
    // 注意：**不要**改成"各用各的 transform"——那会让两端锚点位置各被平移一次，反而错位。
    debug_assert!(
        (from_ir.nominal_font_size - to_ir.nominal_font_size).abs() < 1e-3,
        "形变两端必须用相同标称字号编译（from={}, to={}）",
        from_ir.nominal_font_size,
        to_ir.nominal_font_size
    );

    // ir_to_draw_cmds 对所有 item 使用同一个 transform（scale ∘ translate）。
    let Some(transform) = ir_to_draw_cmds(from_ir, spec.font_size, spec.world_pos)
        .next()
        .map(|c| c.transform)
    else {
        return Vec::new();
    };
    xc_content::pair_by_anchor(from_ir, to_ir)
        .into_iter()
        .map(|(a, b)| (a.transformed(&transform), b.transformed(&transform)))
        .collect()
}

/// 描边色 / 宽度回退规则（计划 §3.3）：
/// 1. stroke 存在且宽度 > 0 → 用其颜色与宽度（`content_ir` 已换算为**世界单位**）；
/// 2. 否则若 fill 存在 → 用 fill 色 + 默认细线宽（`DEFAULT_STROKE_WIDTH_PT × scale`，
///    同样换算为世界单位——描边是世界空间，须与字形成比例）；
/// 3. 都无 → 跳过该 path。
fn stroke_style(cmd: &ContentDrawCmd, scale: f32) -> Option<(Color32, f32)> {
    let style = &cmd.style;
    let default_w = DEFAULT_STROKE_WIDTH_PT * scale;
    match (style.stroke, style.fill) {
        (Some(color), _) if style.stroke_width > 0.0 => Some((color, style.stroke_width)),
        (Some(color), _) => Some((color, default_w)),
        (None, Some(color)) => Some((color, default_w)),
        (None, None) => None,
    }
}

/// 子路径组内首尾点相同即视为闭合。
fn is_closed(sub: &[glam::Vec2]) -> bool {
    match (sub.first(), sub.last()) {
        (Some(a), Some(b)) => sub.len() >= 2 && (*a - *b).length() <= CLOSE_EPS,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_content::{ContentSource, StyledPath};
    use xc_geometry::{Color32 as GColor, Path2D, Stroke2D, Vec2 as GVec2};

    /// 构造指定标称字号的 FormulaSpec（标称字号只影响编译精度，不影响显示尺寸）。
    fn spec_with_nominal(source: &str, font_size: f32, nominal: f32) -> FormulaSpec {
        FormulaSpec {
            nominal_font_size: nominal,
            ..FormulaSpec::new(source, font_size, glam::Vec2::ZERO)
        }
    }

    /// 用占位引擎生成一条 IR（标称字号 = nominal），不依赖 typst feature。
    fn placeholder_ir(source: &str, nominal: f32) -> ContentIR {
        let engine = xc_prerender::placeholder_engine();
        let spec = spec_with_nominal(source, nominal, nominal);
        rebuild(&engine, &spec).expect("placeholder IR")
    }

    /// 取路径的折线点（`MoveTo` / `LineTo` 端点），用于断言几何。
    fn path_points(path: &Path2D) -> Vec<glam::Vec2> {
        path.commands
            .iter()
            .filter_map(|c| match c {
                xc_geometry::PathCommand::MoveTo(p) | xc_geometry::PathCommand::LineTo(p) => {
                    Some(*p)
                }
                _ => None,
            })
            .collect()
    }

    /// 顶点集合的包围盒尺寸（宽, 高）。
    fn bounds_of(paths: &[StrokePath]) -> (f32, f32) {
        let mut min = glam::Vec2::splat(f32::INFINITY);
        let mut max = glam::Vec2::splat(f32::NEG_INFINITY);
        for (path, _, _, _) in paths {
            for p in path_points(path) {
                min = min.min(p);
                max = max.max(p);
            }
        }
        (max.x - min.x, max.y - min.y)
    }

    #[test]
    fn path_count_matches_items() {
        let ir = placeholder_ir("abc", 12.0);
        let spec = FormulaSpec::new("abc", 12.0, glam::Vec2::ZERO);
        let paths = compile_to_stroke_paths(&ir, &spec);
        // 占位字形每个都是单轮廓闭合矩形 → 每条 item 恰好一条路径。
        assert_eq!(paths.len(), ir.items.len());
        assert_eq!(paths.len(), 3);
        for (path, _, _, space) in &paths {
            assert!(
                matches!(path.commands.last(), Some(xc_geometry::PathCommand::Close)),
                "占位矩形应为闭合子路径"
            );
            // 公式描边恒为世界空间（随缩放成比例变化）。
            assert_eq!(*space, MeasureSpace::World);
        }
    }

    #[test]
    fn every_path_has_points() {
        let ir = placeholder_ir("xy", 12.0);
        let spec = FormulaSpec::new("xy", 12.0, glam::Vec2::ZERO);
        let paths = compile_to_stroke_paths(&ir, &spec);
        assert!(!paths.is_empty());
        for (path, _, width, _) in &paths {
            assert!(!path_points(path).is_empty(), "每条路径应至少有一个点");
            assert!(*width > 0.0);
        }
    }

    /// 默认细线宽也是**世界单位**：`1pt × (target / nominal)`；
    /// camera.scale = 40 时应约 4.3px（计划 §3.6 的数值校验）。
    #[test]
    fn default_width_is_world_units() {
        let ir = placeholder_ir("a", 12.0);
        let spec = FormulaSpec::new("a", 1.3, glam::Vec2::ZERO);
        let paths = compile_to_stroke_paths(&ir, &spec);
        let world_w = paths[0].2;
        assert!((world_w - 1.0 * 1.3 / 12.0).abs() < 1e-6, "got {world_w}");
        assert_eq!(paths[0].3, MeasureSpace::World);

        let px = xc_geometry::resolve_width(world_w, MeasureSpace::World, 40.0);
        assert!((px - 4.333).abs() < 0.01, "屏幕宽度应约 4.3px，实际 {px}");
    }

    #[test]
    fn font_size_doubles_bounds() {
        // 固定 IR（nominal = 12），仅改变 target 字号，隔离缩放逻辑。
        let ir = placeholder_ir("abc", 12.0);
        let spec1 = FormulaSpec::new("abc", 12.0, glam::Vec2::ZERO);
        let spec2 = FormulaSpec::new("abc", 24.0, glam::Vec2::ZERO);
        let (w1, h1) = bounds_of(&compile_to_stroke_paths(&ir, &spec1));
        let (w2, h2) = bounds_of(&compile_to_stroke_paths(&ir, &spec2));
        assert!((w2 - 2.0 * w1).abs() < 1e-3, "宽度未按 2 倍缩放: {w1} -> {w2}");
        assert!((h2 - 2.0 * h1).abs() < 1e-3, "高度未按 2 倍缩放: {h1} -> {h2}");
    }

    /// nominal 只影响曲线提取精度，不影响最终显示尺寸：
    /// 同样的 target、不同的 nominal → 世界包围盒一致。
    #[test]
    fn nominal_change_does_not_affect_display_size() {
        let target = 24.0;
        // IR 以 nominal=12 提取，再缩放到 target=24（scale 2）。
        let ir12 = placeholder_ir("abc", 12.0);
        let spec12 = spec_with_nominal("abc", target, 12.0);
        // IR 以 nominal=24 提取，再缩放到 target=24（scale 1）。
        let ir24 = placeholder_ir("abc", 24.0);
        let spec24 = spec_with_nominal("abc", target, 24.0);

        let (w_a, h_a) = bounds_of(&compile_to_stroke_paths(&ir12, &spec12));
        let (w_b, h_b) = bounds_of(&compile_to_stroke_paths(&ir24, &spec24));
        assert!((w_a - w_b).abs() < 1e-3, "宽度应一致: {w_a} vs {w_b}");
        assert!((h_a - h_b).abs() < 1e-3, "高度应一致: {h_a} vs {h_b}");
        // 且确实等于 target（placeholder 高度 = 字号）。
        assert!((h_a - target).abs() < 1e-3, "高度应等于 target: {h_a}");
    }

    /// Y 轴方向确认（计划 §3.4 / §5）：IR 与场景世界都是 Y-up，
    /// 桥接层不得额外翻转——世界坐标下所有顶点应在锚点上方。
    #[test]
    fn geometry_stays_y_up_from_anchor() {
        let ir = placeholder_ir("abc", 12.0);
        let anchor = glam::Vec2::new(-8.0, 2.5);
        let spec = FormulaSpec::new("abc", 12.0, anchor);
        let paths = compile_to_stroke_paths(&ir, &spec);
        for (path, _, _, _) in &paths {
            for v in path_points(path) {
                assert!(v.x >= anchor.x - 1e-3, "x 越界: {}", v.x);
                assert!(v.y >= anchor.y - 1e-3, "y 应位于锚点上方: {}", v.y);
            }
        }
    }

    /// 多轮廓字形（`o`/`B`/`8` 的内外圈）必须拆成多条独立路径，
    /// 否则会在轮廓间画出横穿字形内部的桥接线。
    #[test]
    fn multi_contour_splits_into_separate_paths() {
        let mut path = Path2D::new();
        // 外圈矩形 + 内圈矩形（模拟 `o`）。
        path.move_to(GVec2::new(0.0, 0.0))
            .line_to(GVec2::new(10.0, 0.0))
            .line_to(GVec2::new(10.0, 10.0))
            .line_to(GVec2::new(0.0, 10.0))
            .close()
            .move_to(GVec2::new(3.0, 3.0))
            .line_to(GVec2::new(7.0, 3.0))
            .line_to(GVec2::new(7.0, 7.0))
            .line_to(GVec2::new(3.0, 7.0))
            .close();

        let mut ir = ContentIR::empty(ContentSource::from_raw("o"), 10.0);
        ir.items.push(StyledPath::stroked(
            path,
            Stroke2D::new(GColor::from_rgb(0xFFFFFF), 1.0),
        ));
        ir.recompute_bounds();

        let spec = FormulaSpec::new("o", 10.0, glam::Vec2::ZERO);
        let paths = compile_to_stroke_paths(&ir, &spec);
        assert_eq!(paths.len(), 2, "双轮廓应拆成两条独立路径");
        for (path, _, _, _) in &paths {
            assert_eq!(path_points(path).len(), 5, "每个轮廓：4 顶点 + 回起点");
            assert!(
                matches!(path.commands.last(), Some(xc_geometry::PathCommand::Close)),
                "轮廓应闭合"
            );
        }
        // 两条路径顶点完全不重叠（没有跨轮廓的桥接点）。
        let outer = path_points(&paths[0].0);
        let inner = path_points(&paths[1].0);
        assert_ne!(
            (outer[0].x, outer[0].y),
            (inner[0].x, inner[0].y),
            "内外圈起点应不同"
        );
    }

    /// stroke 存在时优先用 stroke 色与宽度。
    #[test]
    fn stroke_style_prefers_stroke() {
        let mut path = Path2D::new();
        path.move_to(GVec2::new(0.0, 0.0))
            .line_to(GVec2::new(4.0, 0.0));
        let mut ir = ContentIR::empty(ContentSource::from_raw("s"), 10.0);
        ir.items.push(StyledPath::stroked(
            path,
            Stroke2D::new(GColor::from_rgb(0x00FF00), 2.5),
        ));
        ir.recompute_bounds();

        let spec = FormulaSpec::new("s", 10.0, glam::Vec2::ZERO);
        let paths = compile_to_stroke_paths(&ir, &spec);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].1, Color32::from_rgb(0x00FF00));
        assert!((paths[0].2 - 2.5).abs() < 1e-6);
        assert!(
            !matches!(paths[0].0.commands.last(), Some(xc_geometry::PathCommand::Close)),
            "开放折线不应标记为闭合"
        );
    }

    /// 无色（fill/stroke 都无）的 path 被跳过。
    #[test]
    fn colorless_path_is_skipped() {
        let mut path = Path2D::new();
        path.move_to(GVec2::new(0.0, 0.0))
            .line_to(GVec2::new(4.0, 4.0));
        let mut ir = ContentIR::empty(ContentSource::from_raw("n"), 10.0);
        ir.items.push(StyledPath {
            path,
            fill: None,
            stroke: None,
            anchor: None,
        });
        ir.recompute_bounds();

        let spec = FormulaSpec::new("n", 10.0, glam::Vec2::ZERO);
        assert!(compile_to_stroke_paths(&ir, &spec).is_empty());
    }
}
