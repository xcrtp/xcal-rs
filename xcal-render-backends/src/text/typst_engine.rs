// text/typst_engine.rs
//
// Typst 文本/公式引擎（预留空壳）。
// 后续：依赖 typst crate，把 Typst 源码编译为 Path2D 或位图。

/// Typst 文本渲染引擎（预留）。
#[derive(Debug, Default)]
pub struct TypstEngine;

impl TypstEngine {
    pub fn new() -> Self {
        Self
    }
}
