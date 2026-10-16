// placeholder.rs — 占位 IR 生成器（无字体、无真排版）。
//
// 目的：在没有 Typst / 字体的情况下，把 IR / 锚点 / SVG 序列化整条链路跑通，
// 并建立 P1 测试基线。策略：每字符一个矩形，矩形按字符索引横向排列；
// 每个矩形的 anchor 指回归一化源码中的对应字节区间。

use crate::ir::{ContentIR, ContentSource, Span, StyledPath};
use xc_notion::{Color32, Path2D, Vec2};

/// 占位 IR 生成选项。
#[derive(Debug, Clone)]
pub struct PlaceholderOptions {
    /// 生成字号（pt）；占位生成器以其为单位输出几何。
    pub nominal_font_size: f32,
    /// 每个字符的宽度（pt）。默认 0.6 * font_size。
    pub glyph_width: Option<f32>,
    /// 每个字符的高度（pt）。默认 1.0 * font_size。
    pub glyph_height: Option<f32>,
    /// 填充色。
    pub fill: Color32,
}

impl Default for PlaceholderOptions {
    fn default() -> Self {
        Self {
            nominal_font_size: 12.0,
            glyph_width: None,
            glyph_height: None,
            fill: Color32::BLACK,
        }
    }
}

/// 从源码生成占位 IR。每个归一化后的字符对应一个矩形路径，锚点指向该字符字节区间。
pub fn generate_placeholder(raw: &str, opts: &PlaceholderOptions) -> ContentIR {
    let source = ContentSource::from_raw(raw);
    let font_size = opts.nominal_font_size;
    let w = opts.glyph_width.unwrap_or(font_size * 0.6);
    let h = opts.glyph_height.unwrap_or(font_size);
    // baseline 放在字形高度 80% 处（大致模拟拉丁字母基线）。
    let baseline = h * 0.8;

    let mut ir = ContentIR::empty(source.clone(), font_size);
    ir.baseline = baseline;

    let text: &str = &source.text;
    let bytes = text.as_bytes();
    let mut cursor_x = 0.0f32;
    let mut byte_idx = 0usize;
    // 按 UTF-8 字符迭代，记录每个字符的字节边界。
    for (ch_i, ch) in text.char_indices() {
        let start = ch_i;
        let end = ch_i + ch.len_utf8();
        // 跳过空白字符的可视占位（依然推进 cursor，保留间距）。
        let is_whitespace = ch.is_whitespace();
        if !is_whitespace {
            let mut path = Path2D::new();
            path.move_to(Vec2::new(cursor_x, 0.0))
                .line_to(Vec2::new(cursor_x + w, 0.0))
                .line_to(Vec2::new(cursor_x + w, h))
                .line_to(Vec2::new(cursor_x, h))
                .close();
            let anchor = Span::new(start, end);
            ir.items.push(StyledPath::filled(path, opts.fill).with_anchor(anchor));
        }
        cursor_x += w;
        byte_idx = end;
    }
    // 保证 byte_idx 消费完（仅作 sanity，实际无用）。
    debug_assert_eq!(byte_idx, bytes.len());
    ir.recompute_bounds();
    ir
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svg::{to_svg, SvgOptions};

    #[test]
    fn placeholder_maps_each_visible_char_to_rect_with_anchor() {
        let ir = generate_placeholder("abc", &PlaceholderOptions::default());
        // 空白被归一化后仍非空。3 个可见字符 -> 3 个 StyledPath。
        assert_eq!(ir.items.len(), 3);
        // 锚点能切回正确子串。
        assert_eq!(ir.items[0].anchor.unwrap().slice(&ir.source.text), "a");
        assert_eq!(ir.items[1].anchor.unwrap().slice(&ir.source.text), "b");
        assert_eq!(ir.items[2].anchor.unwrap().slice(&ir.source.text), "c");
    }

    #[test]
    fn placeholder_unicode_handles_multi_byte_characters() {
        let ir = generate_placeholder("αβ", &PlaceholderOptions::default());
        assert_eq!(ir.items.len(), 2);
        assert_eq!(ir.items[0].anchor.unwrap().slice(&ir.source.text), "α");
        assert_eq!(ir.items[1].anchor.unwrap().slice(&ir.source.text), "β");
    }

    #[test]
    fn placeholder_bounds_equals_union_of_rects() {
        let opts = PlaceholderOptions { nominal_font_size: 10.0, ..Default::default() };
        let ir = generate_placeholder("ab", &opts);
        // 两字符宽 0.6 * 10 = 6 共 12；高 10。
        let approx_w = 12.0f32;
        let approx_h = 10.0f32;
        assert!((ir.bounds.size().x - approx_w).abs() < 1e-4);
        assert!((ir.bounds.size().y - approx_h).abs() < 1e-4);
        assert!((ir.bounds.min.x).abs() < 1e-4);
        assert!((ir.bounds.min.y).abs() < 1e-4);
    }

    #[test]
    fn placeholder_skipping_whitespace_still_keeps_layout_width() {
        // "a b" 归一化为 "a b"（单空格），a 之后有空格，然后 b。
        let ir = generate_placeholder("a b", &PlaceholderOptions::default());
        assert_eq!(ir.source.text, "a b");
        // a、空格（跳过可视化）、b → 两个 item，b 的 x 偏移 = 两字符宽度。
        assert_eq!(ir.items.len(), 2);
        // 第二个 item (b) 的起点 x = 2 * 单个字符宽度。
        let fs = 12.0f32;
        let w = fs * 0.6;
        let b_rect_min_x = ir.items[1].path.compute_bounds().unwrap().min.x;
        assert!((b_rect_min_x - 2.0 * w).abs() < 1e-4, "b 的 x 应为 2w，实际 {b_rect_min_x}");
    }

    #[test]
    fn placeholder_nominal_font_size_does_not_change_pt_numerics_when_override_width_height() {
        // 直接通过 glyph_width/glyph_height 固定几何；字号改变不影响 bounds 的 pt 值。
        let a = generate_placeholder(
            "x",
            &PlaceholderOptions {
                nominal_font_size: 12.0,
                glyph_width: Some(10.0),
                glyph_height: Some(20.0),
                ..Default::default()
            },
        );
        let b = generate_placeholder(
            "x",
            &PlaceholderOptions {
                nominal_font_size: 42.0,
                glyph_width: Some(10.0),
                glyph_height: Some(20.0),
                ..Default::default()
            },
        );
        assert_eq!(a.bounds, b.bounds);
        assert_eq!(a.nominal_font_size, 12.0);
        assert_eq!(b.nominal_font_size, 42.0);
    }

    #[test]
    fn placeholder_svg_contains_expected_path_and_anchors() {
        let ir = generate_placeholder("x^2", &PlaceholderOptions::default());
        let svg = to_svg(&ir, &SvgOptions::default());
        // "x^2" 归一化为 "x^2"，3 个可见字符 → 3 个 path。
        assert_eq!(svg.matches("<path").count(), 3);
        assert!(svg.contains("data-span=\"0:1\"")); // x
        assert!(svg.contains("data-span=\"1:2\"")); // ^
        assert!(svg.contains("data-span=\"2:3\"")); // 2
    }
}
