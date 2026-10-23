// formula.rs — Typst 内容管线 → 旧 2D 描边路径管线的桥接层。
//
// 数据流：
//   ContentIR →(ir_to_draw_cmds)→ ContentDrawCmd
//             →(flatten_subpaths + transform.apply_point)→ Path 组件（Path2D）
//             →(spawn)→ SceneRecordSystem → DrawQueue → WgpuBackend2D（GPU 离屏）。
//
// 走描边（Stroke）而非填充：公式字形以**空心字**呈现。原本是为了规避旧 CPU 管线
// Fill 分支用 `convex_polygon` 把非凸字形（o/a/e/B 的有孔洞与凹角）画成凸包的
// 缺陷；S4.1 删除该路径后缺陷不复存在，此处保留空心字形作为**既定视觉**。
//
// 本模块是接线，不新增架构抽象：缩放与平移完全复用 `ir_to_draw_cmds`。

#[cfg(test)]
use ze_content::ContentIR;
#[cfg(test)]
use ze_notion::Path2D;
use ze_prerender::InMemoryEngine;
#[cfg(test)]
use ze_prerender::{ContentConfig, ContentEngine};
#[cfg(test)]
use zela_render::{MeasureSpace, ir_to_draw_cmds};
#[cfg(test)]
use zela_scene_ecs::types::Color32;

/// 公式源描述（静态配置；将来接交互时改为可编辑字段）。
///
/// A1/A2 后生产路径不再消费它（分镜已搬到 `zela-scene`）；保留作等价性/
/// 几何用例的类型夹具。
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct FormulaSpec {
    /// Typst 源码。
    pub source: String,
    /// 目标显示字号（pt ≈ 世界单位高度）。经 `ir_to_draw_cmds` 缩放到世界坐标。
    pub font_size: f32,
    /// 世界坐标锚点（Y-up；IR 原点位于墨迹包围盒左下方）。
    pub world_pos: ze_notion::Vec2,
    /// Typst 编译用的标称字号（pt）：决定曲线提取精度与 IR 本地点尺度。
    /// 与 [`font_size`](Self::font_size) 解耦——后者只决定最终显示大小。
    pub nominal_font_size: f32,
}

#[cfg(test)]
impl FormulaSpec {
    /// 用默认标称字号（12.0）构造。
    pub fn new(source: impl Into<String>, font_size: f32, world_pos: ze_notion::Vec2) -> Self {
        Self {
            source: source.into(),
            font_size,
            world_pos,
            nominal_font_size: ze_content::DEFAULT_NOMINAL_FONT_SIZE,
        }
    }
}

/// 按 feature 选择引擎：typst 开启时用 Typst 引擎（初始化失败则回退占位引擎），
/// 关闭时用占位引擎（显示方块，链路仍可跑通）。
pub fn build_engine() -> InMemoryEngine {
    #[cfg(feature = "typst")]
    {
        match ze_prerender::typst_engine() {
            Ok(engine) => return engine,
            Err(e) => eprintln!("[zela-app] typst 引擎初始化失败，回退 placeholder：{e}"),
        }
    }
    ze_prerender::placeholder_engine()
}

/// 编译公式源码为 IR（将来接交互时可在帧内调用；本计划仅启动时调用一次）。
///
/// 注意：`ContentConfig.font_size` 填的是**标称字号**（决定曲线提取精度），
/// 不是目标显示大小；显示用的缩放由 `ir_to_draw_cmds` 的
/// `scale = target / nominal` 完成，勿绕过。
#[cfg(test)]
pub fn rebuild(engine: &InMemoryEngine, spec: &FormulaSpec) -> Option<ContentIR> {
    let config = ContentConfig {
        font_size: spec.nominal_font_size,
        fill: Color32::WHITE,
        scale_factor: 1.0,
    };
    let handle = engine.request(&spec.source, &config);
    engine.take(handle).and_then(|r| r.ok())
}

/// 纯文本标注的源描述（T3）：与 [`FormulaSpec`] 同形，但源码按**正文**语义编译。
///
/// # 核查结论（Typst 模式：math vs 正文）
///
/// `ze_prerender::typst::compile` 的 `wrap_if_math` 规则是：**源码不以 `$` 开头 ⇒
/// 一律包成 `$ 源码 $`（inline math）**。故既有机制里**没有**"正文模式"入口
/// —— 任何非 `$` 源码都会被当数学排版（标识符变斜体）。
///
/// 本轮**不改内核**，改用数学模式下的**字符串字面量**：把文本包成 `"文本"`
/// ⇒ Typst 渲染为**直立正文**（非斜体、空格原样保留），观感与正文一致。
/// 于是 `rebuild` / `compile_to_stroke_paths` 一行不动 ⇒ 自动继承
/// `Reveal`（书写）/ `Opacity`（淡入）/ `Visibility` / `Group` / `Transform`。
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct TextSpec {
    /// 纯文本内容（无需 Typst 语法）。
    pub source: String,
    /// 目标显示字号（pt ≈ 世界单位高度），语义同 [`FormulaSpec::font_size`]。
    pub font_size: f32,
    /// 世界坐标锚点（Y-up；IR 原点位于墨迹包围盒左下方）。
    pub world_pos: ze_notion::Vec2,
    /// Typst 编译用的标称字号（pt）。
    pub nominal_font_size: f32,
}

#[cfg(test)]
impl TextSpec {
    /// 用默认标称字号（12.0）构造。
    pub fn new(source: impl Into<String>, font_size: f32, world_pos: ze_notion::Vec2) -> Self {
        Self {
            source: source.into(),
            font_size,
            world_pos,
            nominal_font_size: ze_content::DEFAULT_NOMINAL_FONT_SIZE,
        }
    }

    /// 转成等价的 [`FormulaSpec`]：源码包装为 Typst 数学模式下的字符串字面量。
    ///
    /// 其余字段原样保留 ⇒ 缩放 / 锚点 / 标称字号口径与公式完全一致。
    pub fn to_formula_spec(&self) -> FormulaSpec {
        FormulaSpec {
            source: ze_content::text_literal_source(&self.source),
            font_size: self.font_size,
            world_pos: self.world_pos,
            nominal_font_size: self.nominal_font_size,
        }
    }
}

/// 一条可 spawn 的描边路径：`(路径, 颜色, 线宽, 线宽所属空间)`。
///
/// 每个子轮廓单独一条——多轮廓字形（`o`/`B`/`8`）不会在轮廓之间
/// 产生横穿字形内部的桥接线。
///
/// `space` 由 `ir_to_draw_cmds` 声明（当前恒为 `MeasureSpace::World`：
/// 宽度已乘 `target / nominal` 换算为世界单位，与字形一同随缩放变化）。
#[cfg(test)]
pub type StrokePath = (Path2D, Color32, f32, MeasureSpace);

/// IR → 可 spawn 的描边路径列表。
///
/// 纯逻辑（展平 / 缩放 / 平移 / 描边回退）在 `ze-content`，两侧共用同一实现；
/// 这里只做形状适配。
#[cfg(test)]
pub fn compile_to_stroke_paths(ir: &ContentIR, spec: &FormulaSpec) -> Vec<StrokePath> {
    ze_content::stroke_outlines(ir, spec.font_size, spec.world_pos, false)
        .into_iter()
        .map(|outline| {
            (
                outline.path,
                outline.color,
                outline.width,
                MeasureSpace::World,
            )
        })
        .collect()
}

/// 语义形变（S1b）：按源码锚点配对两个公式的路径，并把每对路径变换到世界坐标。
///
/// - 配对规则见 [`ze_content::pair_by_anchor`]（锚点相同 → 配对；单侧 → 淡入/淡出；
///   无锚点 → 索引配对）；
/// - 变换**复用** `ir_to_draw_cmds` 的 transform（缩放 + 平移），不另写一套。
///
/// **前提**：`from_ir` 与 `to_ir` 必须用**相同标称字号**编译 —— transform 的
/// `scale = target / nominal_font_size` 只取自 `from_ir`，两端 nominal 不同则 `to`
/// 的路径会被错误缩放（不报错，只画错）。
///
/// 产出交给 `Path2D::morph` / `Curve::PathMorph` 逐对插值（配对知识不进 ze-time）。
///
/// ⚠️ **当前无生产调用点**（P5 记账）：它的唯一消费方是原 S1b「`x²` ↔ `x³` 语义形变」
/// demo，本轮分镜脚本（`storyboard.rs`）取代了那批 2D demo。函数与文档一并保留 ——
/// 它是"语义配对形变"这条既有能力的接线（配对知识在 `ze_content`，不在 `ze-time`），
/// 删除属于独立的下游 churn，不属于本轮范围。
#[cfg(test)]
#[allow(dead_code)]
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
    ze_content::pair_by_anchor(from_ir, to_ir)
        .into_iter()
        .map(|(a, b)| (a.transformed(&transform), b.transformed(&transform)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_content::{ContentSource, StyledPath};
    use ze_notion::{Color32 as GColor, Path2D, Stroke2D, Vec2 as GVec2};

    /// 构造指定标称字号的 FormulaSpec（标称字号只影响编译精度，不影响显示尺寸）。
    fn spec_with_nominal(source: &str, font_size: f32, nominal: f32) -> FormulaSpec {
        FormulaSpec {
            nominal_font_size: nominal,
            ..FormulaSpec::new(source, font_size, ze_notion::Vec2::ZERO)
        }
    }

    /// 用占位引擎生成一条 IR（标称字号 = nominal），不依赖 typst feature。
    fn placeholder_ir(source: &str, nominal: f32) -> ContentIR {
        let engine = ze_prerender::placeholder_engine();
        let spec = spec_with_nominal(source, nominal, nominal);
        rebuild(&engine, &spec).expect("placeholder IR")
    }

    /// 取路径的折线点（`MoveTo` / `LineTo` 端点），用于断言几何。
    fn path_points(path: &Path2D) -> Vec<ze_notion::Vec2> {
        path.commands
            .iter()
            .filter_map(|c| match c {
                ze_notion::PathCommand::MoveTo(p) | ze_notion::PathCommand::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect()
    }

    /// 顶点集合的包围盒尺寸（宽, 高）。
    fn bounds_of(paths: &[StrokePath]) -> (f32, f32) {
        let mut min = ze_notion::Vec2::splat(f32::INFINITY);
        let mut max = ze_notion::Vec2::splat(f32::NEG_INFINITY);
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
        let spec = FormulaSpec::new("abc", 12.0, ze_notion::Vec2::ZERO);
        let paths = compile_to_stroke_paths(&ir, &spec);
        // 占位字形每个都是单轮廓闭合矩形 → 每条 item 恰好一条路径。
        assert_eq!(paths.len(), ir.items.len());
        assert_eq!(paths.len(), 3);
        for (path, _, _, space) in &paths {
            assert!(
                matches!(path.commands.last(), Some(ze_notion::PathCommand::Close)),
                "占位矩形应为闭合子路径"
            );
            // 公式描边恒为世界空间（随缩放成比例变化）。
            assert_eq!(*space, MeasureSpace::World);
        }
    }

    #[test]
    fn every_path_has_points() {
        let ir = placeholder_ir("xy", 12.0);
        let spec = FormulaSpec::new("xy", 12.0, ze_notion::Vec2::ZERO);
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
        let spec = FormulaSpec::new("a", 1.3, ze_notion::Vec2::ZERO);
        let paths = compile_to_stroke_paths(&ir, &spec);
        let world_w = paths[0].2;
        assert!((world_w - 1.0 * 1.3 / 12.0).abs() < 1e-6, "got {world_w}");
        assert_eq!(paths[0].3, MeasureSpace::World);

        let px = ze_notion::resolve_width(world_w, MeasureSpace::World, 40.0);
        assert!((px - 4.333).abs() < 0.01, "屏幕宽度应约 4.3px，实际 {px}");
    }

    #[test]
    fn font_size_doubles_bounds() {
        // 固定 IR（nominal = 12），仅改变 target 字号，隔离缩放逻辑。
        let ir = placeholder_ir("abc", 12.0);
        let spec1 = FormulaSpec::new("abc", 12.0, ze_notion::Vec2::ZERO);
        let spec2 = FormulaSpec::new("abc", 24.0, ze_notion::Vec2::ZERO);
        let (w1, h1) = bounds_of(&compile_to_stroke_paths(&ir, &spec1));
        let (w2, h2) = bounds_of(&compile_to_stroke_paths(&ir, &spec2));
        assert!(
            (w2 - 2.0 * w1).abs() < 1e-3,
            "宽度未按 2 倍缩放: {w1} -> {w2}"
        );
        assert!(
            (h2 - 2.0 * h1).abs() < 1e-3,
            "高度未按 2 倍缩放: {h1} -> {h2}"
        );
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
        let anchor = ze_notion::Vec2::new(-8.0, 2.5);
        let spec = FormulaSpec::new("abc", 12.0, anchor);
        let paths = compile_to_stroke_paths(&ir, &spec);
        for (path, _, _, _) in &paths {
            for v in path_points(path) {
                assert!(v.x >= anchor.x - 1e-3, "x 越界: {}", v.x);
                assert!(v.y >= anchor.y - 1e-3, "y 应位于锚点上方: {}", v.y);
            }
        }
    }

    /// 多轮廓字形（`o`/`B`/`8` 的内外圈）必须留在**同一条路径**里：
    /// 同一路径 ⇒ 填充按 nonzero 规则把内圈挖成孔洞；不同子路径之间由
    /// `MoveTo` 分隔 ⇒ 不会画出横穿字形内部的桥接线。
    #[test]
    fn multi_contour_shares_one_path_without_bridges() {
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

        let spec = FormulaSpec::new("o", 10.0, ze_notion::Vec2::ZERO);
        let paths = compile_to_stroke_paths(&ir, &spec);
        assert_eq!(paths.len(), 1, "一个字形只该产出一条路径（counter 同路径才能挖孔）");
        let path = &paths[0].0;
        assert_eq!(path_points(path).len(), 8, "两个轮廓各 4 顶点（重复尾点已去掉）");
        let moves = path
            .commands
            .iter()
            .filter(|c| matches!(c, ze_notion::PathCommand::MoveTo(_)))
            .count();
        let closes = path
            .commands
            .iter()
            .filter(|c| matches!(c, ze_notion::PathCommand::Close))
            .count();
        assert_eq!((moves, closes), (2, 2), "两条子轮廓由 MoveTo 分隔且都闭合");
        // 第二个子轮廓起点必须是 MoveTo（而不是从上一条轮廓末尾 LineTo 过去）。
        assert!(
            matches!(path.commands[5], ze_notion::PathCommand::MoveTo(_)),
            "内圈起点应为 MoveTo（不得有桥接线）：{:?}",
            path.commands
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

        let spec = FormulaSpec::new("s", 10.0, ze_notion::Vec2::ZERO);
        let paths = compile_to_stroke_paths(&ir, &spec);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].1, Color32::from_rgb(0x00FF00));
        assert!((paths[0].2 - 2.5).abs() < 1e-6);
        assert!(
            !matches!(
                paths[0].0.commands.last(),
                Some(ze_notion::PathCommand::Close)
            ),
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

        let spec = FormulaSpec::new("n", 10.0, ze_notion::Vec2::ZERO);
        assert!(compile_to_stroke_paths(&ir, &spec).is_empty());
    }

    // ─── T3：纯文本（复用 Typst 数学模式下的字符串字面量）──────────────────

    /// 文本源码被包装为**字符串字面量**（直立正文外观），且能编译出描边路径。
    ///
    /// T3 验收 1：`路径数 > 0`（headless，不依赖 typst feature）。
    #[test]
    fn text_spec_compiles_to_nonempty_stroke_paths() {
        let text = TextSpec::new("Hello world", 1.0, ze_notion::Vec2::ZERO);
        let fs = text.to_formula_spec();
        // 包装形态：`"Hello world"`（非斜体正文的关键）。
        assert_eq!(fs.source, "\"Hello world\"");

        let engine = ze_prerender::placeholder_engine();
        let ir = rebuild(&engine, &fs).expect("placeholder IR");
        let paths = compile_to_stroke_paths(&ir, &fs);
        assert!(!paths.is_empty(), "文本应编译出描边路径，实际为空");
        for (path, _, width, space) in &paths {
            assert!(!path_points(path).is_empty(), "每条文本路径应至少有一个点");
            assert!(*width > 0.0);
            assert_eq!(*space, MeasureSpace::World);
        }
    }

    /// 引号 / 反斜杠转义（否则 Typst 字面量会被提前闭合 / 转义错）。
    #[test]
    fn text_spec_escapes_quotes_and_backslashes() {
        let fs = TextSpec::new("a\"b\\c", 1.0, ze_notion::Vec2::ZERO).to_formula_spec();
        assert_eq!(fs.source, "\"a\\\"b\\\\c\"");
    }

    /// T3 验收 4：文字路径经 `record_scene` 受 **`Transform` 摆放** + **`Opacity` 淡入**。
    #[test]
    fn text_paths_honor_transform_and_opacity() {
        use ze_scene_core::{Camera2D, Transform};
        use zela_render::components::Path;
        use zela_render::{DrawQueue, Opacity, StrokeSolid, record_scene};
        use zela_scene_ecs::contract::WorldView;

        let engine = ze_prerender::placeholder_engine();
        let text = TextSpec::new("hi", 1.0, ze_notion::Vec2::ZERO);
        let fs = text.to_formula_spec();
        let ir = rebuild(&engine, &fs).expect("placeholder IR");
        let paths = compile_to_stroke_paths(&ir, &fs);
        assert!(!paths.is_empty());
        let (path, color, width, space) = paths[0].clone();

        let rect =
            ze_notion::Rect2D::new(ze_notion::Vec2::ZERO, ze_notion::Vec2::new(800.0, 600.0));
        let camera = Camera2D::new(ze_notion::Vec2::ZERO, 40.0, rect);

        let mut scene = zela_scene_ecs::scene::Scene::new();
        // 基准：仅淡入（不摆放）。
        scene.spawn((
            Path::new(path.clone()),
            StrokeSolid::new_in(width, color, space),
            Opacity::new(0.5),
        ));
        // 叠加 T1：右移 1 世界单位。
        scene.spawn((
            Path::new(path),
            StrokeSolid::new_in(width, color, space),
            Opacity::new(0.5),
            Transform {
                position: ze_notion::Vec3::new(1.0, 0.0, 0.0),
                ..Default::default()
            },
        ));
        let world = scene.into_world();

        let mut q = DrawQueue::new();
        record_scene(&WorldView::new(&world), &camera, &mut q);
        assert_eq!(q.len(), 2);

        let alpha = |i: usize| q.commands()[i].style.stroke.expect("文本应有描边").a();
        assert_eq!(alpha(0), 128, "opacity = 0.5 ⇒ alpha 255→128");
        assert_eq!(alpha(1), 128);

        let b0 = q.commands()[0].shape.bounds().expect("文本有包围盒");
        let b1 = q.commands()[1].shape.bounds().expect("文本有包围盒");
        assert!(
            (b1.min.x - b0.min.x - 40.0).abs() < 1e-3,
            "Transform 右移 1 世界单位 = 40px（{b0:?} → {b1:?}）"
        );
    }

    /// T3 验收 3：文字路径能被 **`Reveal` 书写**（沿用公式那套，headless 断言）。
    #[test]
    fn text_paths_can_be_revealed() {
        use zela_render::{RevealQueue, RevealRecordSystem, StrokeSolid, ViewRect};
        use zela_scene_ecs::scene::components::Reveal;

        let engine = ze_prerender::placeholder_engine();
        let text = TextSpec::new("ab", 1.0, ze_notion::Vec2::ZERO);
        let fs = text.to_formula_spec();
        let ir = rebuild(&engine, &fs).expect("placeholder IR");
        let (path, _, _, _) = compile_to_stroke_paths(&ir, &fs)
            .into_iter()
            .next()
            .expect("至少一条文本路径");

        let mut scene = zela_scene_ecs::scene::Scene::new();
        let cam = ze_scene_core::spawn_camera2d(
            &mut scene,
            ze_scene_core::Framing2D::new(ze_notion::Vec2::ZERO, 40.0),
        )
        .expect("Framing2D 已登记");
        scene.spawn((
            Reveal::visible(path),
            StrokeSolid::new(2.0, (255, 255, 255, 255)),
        ));
        let world = scene.into_world();

        let mut res = zela_schedule::resource::Resources::new();
        res.insert(cam);
        res.insert(ViewRect {
            rect: ze_notion::Rect2D::new(ze_notion::Vec2::ZERO, ze_notion::Vec2::new(800.0, 600.0)),
        });
        res.insert(RevealQueue::default());

        let mut sched = zela_schedule::scheduler::Scheduler::new(world, res);
        sched.add_system(
            zela_schedule::scheduler::Phase::RenderCommit,
            RevealRecordSystem,
        );
        sched.init_resources();
        sched.run_frame(0.0);

        let items = &sched.resources().get::<RevealQueue>().unwrap().0;
        assert_eq!(items.len(), 1, "文本路径应录进书写队列");
        assert_eq!(
            (items[0].start, items[0].end),
            (0.0, 1.0),
            "沿用 Reveal 区间"
        );
        assert!(!items[0].path.commands.is_empty(), "书写项应带真实几何");
    }
}
