// ze-content: 内容中间表示（Content IR）内核。
// 层次：kernel（纯数据 + 纯函数，无 egui / 无渲染 / 无后端）。
// 依赖方向：ze-content（ze-notion）← ze-prerender ← zela-render。

pub mod ir;
pub mod normalize;
pub mod pair;
pub mod placeholder;
pub mod stroke;
pub mod svg;

pub use ir::{ContentIR, ContentSource, Span, StyledPath};
pub use normalize::{hash_source, normalize_source};
pub use pair::{degenerate_at, pair_by_anchor};
pub use stroke::{
    CLOSE_EPS, DEFAULT_NOMINAL_FONT_SIZE, DEFAULT_STROKE_WIDTH_PT, FLATTEN_TOLERANCE,
    StrokeOutline, order_by_reading_order, stroke_outlines, stroke_style, text_literal_source,
};
pub use svg::{SvgOptions, to_svg};
