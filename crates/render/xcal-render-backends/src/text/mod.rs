// text/
//
// 文本渲染引擎后端（预留空壳）：
// - typst_engine：Typst 排版引擎（数学/公式 → 路径或位图）
// - latex_engine：LaTeX 引擎
// 两者后续实现 xcal_render::TextRenderEngine trait。

pub mod latex_engine;
pub mod typst_engine;

pub use latex_engine::LatexEngine;
pub use typst_engine::TypstEngine;
