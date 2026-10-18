// placeholder.rs — Placeholder 引擎（依赖 xc-content placeholder 生成器；零重依赖）。
//
// 用于：① typst feature 关闭时仍提供一个可用引擎保证 API 形状可用；
// ② 测试整个 engine/cache/handle 管线；③ 无字体时的降级方案。

use crate::engine::{CompileFn, InMemoryEngine};
use crate::request::{ContentConfig, ContentResult};
use std::sync::Arc;
use xc_content::{ContentIR, ContentSource, placeholder::PlaceholderOptions};

/// Placeholder 编译函数（直接对齐 `CompileFn` 签名）。
pub fn compile(src: &ContentSource, cfg: &ContentConfig) -> ContentResult {
    let opts = PlaceholderOptions {
        nominal_font_size: cfg.font_size,
        fill: cfg.fill,
        ..PlaceholderOptions::default()
    };
    // generate_placeholder 需要 &raw 源码来做归一化，但我们已经有归一化的 source。
    // 为了锚点与归一化文本一致，这里从归一化文本再生成。
    Ok(generate_placeholder_from_normalized(src, &opts))
}

/// 与 `generate_placeholder` 等价，但不再次归一化（避免二次归一化在极少情况下
/// 改变锚点索引——实际规则稳定，但这里保留显式语义）。
fn generate_placeholder_from_normalized(
    src: &ContentSource,
    opts: &PlaceholderOptions,
) -> ContentIR {
    let font_size = opts.nominal_font_size;
    let w = opts.glyph_width.unwrap_or(font_size * 0.6);
    let h = opts.glyph_height.unwrap_or(font_size);
    let baseline = h * 0.8;

    use xc_content::{Span, StyledPath};
    use xc_notion::{Path2D, Vec2};

    let mut ir = ContentIR::empty(src.clone(), font_size);
    ir.baseline = baseline;

    let text: &str = &src.text;
    let mut cursor_x = 0.0f32;
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        if !ch.is_whitespace() {
            let mut path = Path2D::new();
            path.move_to(Vec2::new(cursor_x, 0.0))
                .line_to(Vec2::new(cursor_x + w, 0.0))
                .line_to(Vec2::new(cursor_x + w, h))
                .line_to(Vec2::new(cursor_x, h))
                .close();
            ir.items
                .push(StyledPath::filled(path, opts.fill).with_anchor(Span::new(start, end)));
        }
        cursor_x += w;
    }
    ir.recompute_bounds();
    ir
}

/// 构造 Placeholder 引擎实例。
pub fn engine() -> InMemoryEngine {
    let f: Arc<CompileFn> = Arc::new(|s, c| compile(s, c));
    InMemoryEngine::new("placeholder", f)
}
