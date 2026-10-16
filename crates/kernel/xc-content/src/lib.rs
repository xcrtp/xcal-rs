// xc-content: 内容中间表示（Content IR）内核。
// 层次：kernel（纯数据 + 纯函数，无 egui / 无渲染 / 无后端）。
// 依赖方向：xc-content（xc-notion）← xc-prerender ← xcal-render。

pub mod ir;
pub mod normalize;
pub mod pair;
pub mod placeholder;
pub mod svg;

pub use ir::{ContentIR, ContentSource, Span, StyledPath};
pub use normalize::{hash_source, normalize_source};
pub use pair::{degenerate_at, pair_by_anchor};
pub use svg::{to_svg, SvgOptions};