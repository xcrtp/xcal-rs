// content_ir.rs — ze-content ContentIR → zela-render Shape2D + Style2D + Transform2D。
//
// 设计（对应规划 §4.7 消费侧转换）：
// - ContentIR 单位是 pt，nominal_font_size 是"排版时字号"（Typst/Placeholder 都设）。
// - 渲染目标字号 target_font_size 由调用方（zela-app 场景）传入；
//   整体缩放 scale = target_font_size / ir.nominal_font_size。
// - 我们不把 scale 写进 Path 的每个点（会放大浮点误差）；
//   而是用 Transform2D::from_scale 统一应用到 transform。
// - 填充/描边：StyledPath.fill → Style2D.fill；
//   StyledPath.stroke: Stroke2D.color → Style2D.stroke；
//   Stroke2D.width (pt，标称尺度) **乘以与几何相同的 scale**
//   （target_font_size / nominal_font_size）换算为世界单位，
//   并标记 Style2D.space = MeasureSpace::World —— 使描边与字形一起随缩放变化。
//   若不乘 scale，1.0pt 会被当成 1.0 世界单位，在 scale=40 下渲染成 40px（粗得离谱）。
// - 锚点 (anchor/Span) 不送入渲染（后端不需要）；调用方会在 UI 层
//   通过 ContentIR + hit-test 自己处理选中高亮。

use ze_content::{ContentIR, StyledPath};
use ze_notion::{MeasureSpace, Stroke2D, Transform2D};

use crate::shape::{Shape2D, Style2D};

/// 一个绘制指令：形状 + 样式 + 从 IR 本地坐标到画布坐标的 transform。
#[derive(Debug, Clone)]
pub struct ContentDrawCmd {
    pub shape: Shape2D,
    pub style: Style2D,
    pub transform: Transform2D,
}

/// 把 ContentIR 展平为绘制命令列表。
///
/// `target_font_size`：渲染目标字号（pt）。通常对应 UI 里给公式预留的字体大小。
/// `world_pos`：IR 原点 (0,0) 放在画布上的位置。IR 自身坐标原点位于
///  baseline 左下方（与 placeholder.rs 一致），调用方若要做"文本块
///  baseline 对齐某个世界坐标"，就把 world_pos 作为那个点。
///
/// 空内容（items 为空或 bounds 退化为零面积）时不产出任何绘制命令，
/// 避免消费端对退化 bounds 做除零或无效缩放。
pub fn ir_to_draw_cmds<'a>(
    ir: &'a ContentIR,
    target_font_size: f32,
    world_pos: ze_notion::Vec2,
) -> Box<dyn Iterator<Item = ContentDrawCmd> + 'a> {
    if ir.is_empty() {
        return Box::new(std::iter::empty());
    }
    let scale = if ir.nominal_font_size > 0.0 {
        target_font_size / ir.nominal_font_size
    } else {
        1.0
    };
    // T = translate(world_pos) ∘ scale(scale)。
    // 先把 IR 本地点按 scale 缩放到目标字号，再平移到 world_pos。
    let s = Transform2D::scale(ze_notion::Vec2::splat(scale));
    let t = Transform2D::translate(world_pos);
    // compose: self 之后再应用 other。这里先 scale，再 translate ⇒ s.compose(t)。
    let combined = s.compose(t);

    Box::new(
        ir.items
            .iter()
            .map(move |item| styled_path_to_cmd(item, combined, scale)),
    )
}

fn styled_path_to_cmd(p: &StyledPath, transform: Transform2D, scale: f32) -> ContentDrawCmd {
    let shape = Shape2D::Path(p.path.clone());
    let style = Style2D {
        fill: p.fill,
        stroke: p.stroke.as_ref().map(|s: &Stroke2D| s.color),
        // pt（标称尺度）→ 世界单位：与几何共用同一系数，使描边随字形一同缩放。
        stroke_width: p.stroke.as_ref().map(|s| s.width * scale).unwrap_or(0.0),
        space: MeasureSpace::World,
        // 内容 IR 目前不携带 cap / join（`Stroke2D` 只有宽度与颜色）⇒ 用默认形态。
        stroke_shape: ze_render_ir::StrokeShape::DEFAULT,
    };
    ContentDrawCmd {
        shape,
        style,
        transform,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_content::{ContentSource, Span, StyledPath};
    use ze_notion::{Color32, Path2D, Rect2D, Vec2};

    fn make_ir(nominal: f32, items: Vec<StyledPath>) -> ContentIR {
        let mut ir = ContentIR::empty(ContentSource::from_raw("placeholder"), nominal);
        ir.items = items;
        ir.recompute_bounds();
        ir
    }

    /// scale = 2.0: nominal 12 → target 24；IR 点 (6, 0) → 画布 (12, 3)（平移 +3/+3）。
    #[test]
    fn transform_is_scale_then_translate() {
        let mut path = Path2D::new();
        // 6pt × 6pt 矩形，原点左下。
        path.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(6.0, 0.0))
            .line_to(Vec2::new(6.0, 6.0))
            .line_to(Vec2::new(0.0, 6.0))
            .close();
        let p = StyledPath::filled(path, Color32::BLACK).with_anchor(Span::new(0, 1));
        let ir = make_ir(12.0, vec![p]);

        let cmds: Vec<ContentDrawCmd> = ir_to_draw_cmds(&ir, 24.0, Vec2::new(3.0, 3.0)).collect();
        assert_eq!(cmds.len(), 1);
        // 校验：shape 是 Path。
        let bounds = cmds[0].shape.bounds().unwrap();
        assert_eq!(bounds, Rect2D::new(Vec2::ZERO, Vec2::new(6.0, 6.0)));
        // 校验 transform：scale 2，再 translate (3,3)。
        // 直接乘积：点 (6, 6) → 先 scale 2 → (12, 12)；再 +3 → (15, 15)。
        let result = cmds[0].transform.apply_point(Vec2::new(6.0, 6.0));
        assert!(
            (result.x - 15.0).abs() < 1e-4,
            "transformed x = {}",
            result.x
        );
        assert!(
            (result.y - 15.0).abs() < 1e-4,
            "transformed y = {}",
            result.y
        );
    }

    /// fill + stroke 字段一一对应；无 stroke 时 stroke_width 为 0。
    #[test]
    fn style_round_trips_fill_and_stroke() {
        let fill = Color32::from_rgba(1, 2, 3, 4);
        let stroke = Stroke2D::new(Color32::from_rgba(5, 6, 7, 8), 1.5);
        let mut path = Path2D::new();
        path.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(2.0, 2.0));
        let p = StyledPath {
            path,
            fill: Some(fill),
            stroke: Some(stroke),
            anchor: None,
        };
        let ir = make_ir(10.0, vec![p]);
        let cmds: Vec<ContentDrawCmd> = ir_to_draw_cmds(&ir, 10.0, Vec2::ZERO).collect();
        let s = &cmds[0].style;
        assert_eq!(s.fill, Some(fill));
        assert_eq!(s.stroke, Some(stroke.color));
        // 此处 nominal = target = 10 → scale = 1，宽度数值不变；
        // 换算效果在 scale ≠ 1 时体现，见 `stroke_width_scales_to_world_units`。
        assert!((s.stroke_width - stroke.width).abs() < 1e-6);
        assert_eq!(s.space, MeasureSpace::World);
    }

    /// 描边宽度与几何共用同一 scale（pt → 世界单位），并标记为 `World`。
    #[test]
    fn stroke_width_scales_to_world_units() {
        let mut path = Path2D::new();
        path.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(2.0, 0.0));
        let p = StyledPath::stroked(path, Stroke2D::new(Color32::BLACK, 2.0));
        let ir = make_ir(12.0, vec![p]);

        // target = 6 → scale = 0.5 → 2.0pt × 0.5 = 1.0 世界单位
        let cmds: Vec<ContentDrawCmd> = ir_to_draw_cmds(&ir, 6.0, Vec2::ZERO).collect();
        let s = &cmds[0].style;
        assert!(
            (s.stroke_width - 1.0).abs() < 1e-6,
            "got {}",
            s.stroke_width
        );
        assert_eq!(s.space, MeasureSpace::World);

        // 数值校验（计划 §3.6）：nominal=12 / target=1.3 → s≈0.1083；
        // 1.0pt × 0.1083 = 0.1083 世界单位；camera.scale=40 → 约 4.3px。
        let p2 = StyledPath::stroked(
            {
                let mut q = Path2D::new();
                q.move_to(Vec2::new(0.0, 0.0)).line_to(Vec2::new(1.0, 0.0));
                q
            },
            Stroke2D::new(Color32::BLACK, 1.0),
        );
        let ir2 = make_ir(12.0, vec![p2]);
        let cmds2: Vec<ContentDrawCmd> = ir_to_draw_cmds(&ir2, 1.3, Vec2::ZERO).collect();
        let world_w = cmds2[0].style.stroke_width;
        assert!((world_w - 1.3 / 12.0).abs() < 1e-6, "got {world_w}");
        let px = ze_notion::resolve_width(world_w, MeasureSpace::World, 40.0);
        assert!((px - 4.333).abs() < 0.01, "屏幕宽度应约 4.3px，实际 {px}");
    }

    /// 无 stroke 时 stroke_width 为 0（不受 scale 影响）。
    #[test]
    fn no_stroke_yields_zero_width() {
        let mut path = Path2D::new();
        path.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(2.0, 0.0));
        let ir = make_ir(12.0, vec![StyledPath::filled(path, Color32::BLACK)]);
        let cmds: Vec<ContentDrawCmd> = ir_to_draw_cmds(&ir, 24.0, Vec2::ZERO).collect();
        assert_eq!(cmds[0].style.stroke_width, 0.0);
    }

    /// nominal_font_size 为 0 的退化情况 → scale = 1.0（不 NaN）。
    #[test]
    fn zero_nominal_font_size_preserves_scale_1() {
        let mut path = Path2D::new();
        path.move_to(Vec2::new(0.0, 0.0))
            .line_to(Vec2::new(1.0, 0.0));
        let ir = make_ir(0.0, vec![StyledPath::filled(path, Color32::BLACK)]);
        let cmds: Vec<ContentDrawCmd> = ir_to_draw_cmds(&ir, 24.0, Vec2::ZERO).collect();
        // scale = 1.0（nominal=0 时退化保护）
        let v1 = cmds[0].transform.apply_point(Vec2::new(3.0, 4.0));
        assert!((v1.x - 3.0).abs() < 1e-6);
        assert!((v1.y - 4.0).abs() < 1e-6);
    }

    /// 空内容（items 为空或 bounds 退化）→ 不产出绘制命令。
    #[test]
    fn empty_ir_produces_no_cmds() {
        // items 为空
        let ir = make_ir(12.0, vec![]);
        let cmds: Vec<ContentDrawCmd> = ir_to_draw_cmds(&ir, 24.0, Vec2::ZERO).collect();
        assert!(cmds.is_empty());

        // items 非空但路径退化（零面积 bounds）
        let ir2 = make_ir(
            12.0,
            vec![StyledPath::filled(Path2D::new(), Color32::BLACK)],
        );
        let cmds2: Vec<ContentDrawCmd> = ir_to_draw_cmds(&ir2, 24.0, Vec2::ZERO).collect();
        assert!(cmds2.is_empty(), "零面积 bounds 应早退");
    }
}
