// stroke.rs — 内容 IR → 世界坐标描边轮廓（**唯一实现**）。
//
// 为什么在 kernel：`zela-app::formula` 与 `zela-scene::pod::content_service` 曾各自
// 持有一份逐式相同的实现（含四个常量），靠用例守住两份不漂移。把纯逻辑下沉到这里后，
// 两侧只剩类型适配，常量只有一个真源。
//
// 本模块不依赖 `ze-prerender`（编译引擎在调用方）；只做 IR → 折线轮廓的纯变换。

use ze_notion::{Color32, Path2D, Transform2D, Vec2};

use crate::{ContentIR, StyledPath};

/// 曲线展平容差（pt，相对 **标称字号** 尺度）。
///
/// 因为排版按标称字号（默认 12.0）编译 IR，容差相对字形尺度始终合理
/// （0.1/12 ≈ 0.8%）；最终显示尺寸缩小不会放大该误差。
pub const FLATTEN_TOLERANCE: f32 = 0.1;
/// 只有 fill、没有 stroke 时使用的默认描边宽度（**标称 pt**）。
///
/// 需乘 `target / nominal` 换算为世界单位后使用（描边属世界空间）。
pub const DEFAULT_STROKE_WIDTH_PT: f32 = 1.0;
/// 排版编译用的标称字号（pt）。曲线提取精度以它为基准；
/// 目标显示大小由调用方传入的 `font_size` 缩放得到。
pub const DEFAULT_NOMINAL_FONT_SIZE: f32 = 12.0;
/// 子路径首尾点距离 ≤ 此值视为闭合。
pub const CLOSE_EPS: f32 = 1e-4;

/// 一条已落到世界坐标的描边轮廓。
///
/// 每个子轮廓单独一条——多轮廓字形（`o`/`B`/`8`）不会在轮廓之间产生桥接线。
/// `width` 已是世界单位（标称 pt × `target / nominal`）。
#[derive(Debug, Clone, PartialEq)]
pub struct StrokeOutline {
    pub path: Path2D,
    pub color: Color32,
    pub width: f32,
}

/// IR → 世界坐标描边轮廓列表。
///
/// - `scale = font_size / ir.nominal_font_size`（与几何共用同一系数）；
/// - 变换顺序：先缩放、再平移到 `world_pos`；
/// - `sort_reading_order` 为 `true` 时按包围盒 `min_x` 升序稳定排序。
pub fn stroke_outlines(
    ir: &ContentIR,
    font_size: f32,
    world_pos: Vec2,
    sort_reading_order: bool,
) -> Vec<StrokeOutline> {
    if ir.is_empty() {
        return Vec::new();
    }
    let scale = if ir.nominal_font_size > 0.0 {
        font_size / ir.nominal_font_size
    } else {
        1.0
    };
    let transform =
        Transform2D::scale(Vec2::splat(scale)).compose(Transform2D::translate(world_pos));

    let mut built = Vec::new();
    for item in &ir.items {
        let Some((color, width)) = stroke_style(item, scale) else {
            continue;
        };
        for sub in item.path.flatten_subpaths(FLATTEN_TOLERANCE) {
            if sub.is_empty() {
                continue;
            }
            let closed = is_closed(&sub);
            let world: Vec<Vec2> = sub.iter().map(|p| transform.apply_point(*p)).collect();
            built.push(StrokeOutline {
                path: Path2D::from_polyline(&world, closed),
                color,
                width,
            });
        }
    }

    if sort_reading_order {
        order_by_reading_order(&mut built);
    }
    built
}

/// 描边色 / 宽度回退规则：
/// 1. stroke 存在且宽度 > 0 → 用其颜色与宽度（换算为**世界单位**）；
/// 2. 否则若 fill 存在 → 用 fill 色 + 默认细线宽（`DEFAULT_STROKE_WIDTH_PT × scale`）；
/// 3. 都无 → 跳过该 path。
pub fn stroke_style(item: &StyledPath, scale: f32) -> Option<(Color32, f32)> {
    let default_w = DEFAULT_STROKE_WIDTH_PT * scale;
    match (&item.stroke, item.fill) {
        (Some(stroke), _) if stroke.width > 0.0 => Some((stroke.color, stroke.width * scale)),
        (Some(stroke), _) => Some((stroke.color, default_w)),
        (None, Some(fill)) => Some((fill, default_w)),
        (None, None) => None,
    }
}

/// 子路径组内首尾点相同即视为闭合。
fn is_closed(sub: &[Vec2]) -> bool {
    match (sub.first(), sub.last()) {
        (Some(a), Some(b)) => sub.len() >= 2 && (*a - *b).length() <= CLOSE_EPS,
        _ => false,
    }
}

/// 阅读顺序：包围盒 `min_x` 升序；无包围盒排最后；同 `min_x` 保持输入顺序。
pub fn order_by_reading_order(outlines: &mut [StrokeOutline]) {
    outlines.sort_by(|a, b| {
        let key = |o: &StrokeOutline| o.path.compute_bounds().map(|r| r.min.x);
        match (key(a), key(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
}

/// 纯文本 → Typst 数学模式下的**字符串字面量**（直立正文外观）。
///
/// 只需转义反斜杠与双引号（字面量内部）。
pub fn text_literal_source(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ContentSource, StyledPath};
    use ze_notion::{Path2D, Stroke2D, Vec2};

    fn item(x: f32, h: f32) -> StyledPath {
        let path = Path2D::from_polyline(
            &[
                Vec2::new(x, 0.0),
                Vec2::new(x + 1.0, 0.0),
                Vec2::new(x + 1.0, h),
                Vec2::new(x, h),
            ],
            true,
        );
        StyledPath::stroked(path, Stroke2D::new(Color32::WHITE, 1.0))
    }

    /// 阅读顺序：`min_x` 升序；空路径排最后；同 `min_x` 稳定。
    #[test]
    fn reading_order_is_min_x_ascending_without_bounds_last() {
        let rect = |x: f32, h: f32| StrokeOutline {
            path: Path2D::from_polyline(
                &[
                    Vec2::new(x, 0.0),
                    Vec2::new(x + 1.0, 0.0),
                    Vec2::new(x + 1.0, h),
                    Vec2::new(x, h),
                ],
                true,
            ),
            color: Color32::WHITE,
            width: 1.0,
        };

        // 故意乱序：右 → 左 → 空 → 中（含同 min_x 的两个）。
        let mut outlines = vec![
            rect(2.0, 1.0),
            rect(0.0, 1.0),
            StrokeOutline {
                path: Path2D::new(),
                color: Color32::WHITE,
                width: 1.0,
            },
            rect(1.0, 5.0),
            rect(1.0, 1.0),
        ];
        order_by_reading_order(&mut outlines);
        let xs: Vec<Option<f32>> = outlines
            .iter()
            .map(|o| o.path.compute_bounds().map(|b| b.min.x))
            .collect();
        assert_eq!(
            xs,
            vec![Some(0.0), Some(1.0), Some(1.0), Some(2.0), None],
            "必须 min_x 升序且无包围盒者排最后"
        );
        // 稳定：同 min_x 时保持输入顺序（高的那个先）。
        let height = |o: &StrokeOutline| o.path.compute_bounds().unwrap().max.y;
        assert!((height(&outlines[1]) - 5.0).abs() < 1e-6);
        assert!((height(&outlines[2]) - 1.0).abs() < 1e-6);
    }

    /// `stroke_outlines` 不产出空轮廓（与旧实现一致的早退口径）。
    #[test]
    fn empty_subpaths_are_skipped() {
        let mut ir = ContentIR::empty(ContentSource::from_raw("skip"), 12.0);
        ir.items.push(StyledPath::stroked(
            Path2D::new(),
            Stroke2D::new(Color32::WHITE, 1.0),
        ));
        ir.recompute_bounds();
        assert!(stroke_outlines(&ir, 12.0, Vec2::ZERO, true).is_empty());
    }

    /// 缩放与平移：target 字号翻倍 ⇒ 世界包围盒翻倍；描边宽度同系数换算。
    #[test]
    fn scale_and_translate_are_shared_with_geometry() {
        let mut ir = ContentIR::empty(ContentSource::from_raw("scale"), 12.0);
        ir.items = vec![item(0.0, 12.0)];
        ir.recompute_bounds();

        let one = stroke_outlines(&ir, 12.0, Vec2::ZERO, false);
        let two = stroke_outlines(&ir, 24.0, Vec2::ZERO, false);
        let h = |v: &Vec<StrokeOutline>| v[0].path.compute_bounds().unwrap().size().y;
        assert!((h(&two) - 2.0 * h(&one)).abs() < 1e-3);
        assert!((two[0].width - 2.0 * one[0].width).abs() < 1e-6);

        let moved = stroke_outlines(&ir, 12.0, Vec2::new(3.0, 4.0), false);
        assert!((moved[0].path.compute_bounds().unwrap().min.x - 3.0).abs() < 1e-6);
        assert!((moved[0].path.compute_bounds().unwrap().min.y - 4.0).abs() < 1e-6);
    }

    /// 空 IR ⇒ 无轮廓（与两个旧实现一致的早退口径）。
    #[test]
    fn empty_ir_yields_no_outlines() {
        let ir = ContentIR::empty(ContentSource::from_raw("empty"), 12.0);
        assert!(stroke_outlines(&ir, 12.0, Vec2::ZERO, true).is_empty());
    }

    #[test]
    fn text_literal_escapes_quotes_and_backslashes() {
        assert_eq!(text_literal_source("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
