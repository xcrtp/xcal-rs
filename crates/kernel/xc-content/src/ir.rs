// ir.rs — ContentIR / StyledPath / Span / ContentSource 核心数据类型。
// 约定：每个 path 自带 fill/stroke 样式与源码锚点；坐标单位 pt；
// bounds = 墨迹包围盒；baseline 供垂直对齐；nominal_font_size 供消费端换算。

use xc_notion::{Color32, Path2D, Rect2D, Stroke2D};

/// 归一化后的源码 + 内容哈希（跨引擎可比）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContentSource {
    pub text: String,
    pub hash: u64,
}

impl ContentSource {
    pub fn from_raw(raw: &str) -> Self {
        let text = crate::normalize::normalize_source(raw);
        let hash = crate::normalize::hash_source(&text);
        Self { text, hash }
    }

    pub fn from_normalized(normalized: impl Into<String>) -> Self {
        let text: String = normalized.into();
        let hash = crate::normalize::hash_source(&text);
        Self { text, hash }
    }
}

/// 归一化源码中的字节区间（锚点）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        debug_assert!(start <= end);
        Self { start: start as u32, end: end as u32 }
    }

    pub fn is_empty(&self) -> bool { self.start == self.end }

    pub fn len(&self) -> usize { (self.end - self.start) as usize }

    /// 从归一化源码中切出子串（越界安全，返回空串）。
    pub fn slice<'a>(&self, source: &'a str) -> &'a str {
        let s = self.start as usize;
        let e = self.end as usize;
        if s <= e && e <= source.len() { &source[s..e] } else { "" }
    }

    pub fn intersects(&self, other: Span) -> bool {
        self.start < other.end && other.start < self.end
    }

    pub fn intersection(&self, other: Span) -> Option<Span> {
        if !self.intersects(other) { return None; }
        Some(Span { start: self.start.max(other.start), end: self.end.min(other.end) })
    }
}

/// 一条带样式与锚点的路径。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct StyledPath {
    pub path: Path2D,
    pub fill: Option<Color32>,
    pub stroke: Option<Stroke2D>,
    pub anchor: Option<Span>,
}

impl StyledPath {
    pub fn filled(path: Path2D, fill: Color32) -> Self {
        Self { path, fill: Some(fill), stroke: None, anchor: None }
    }

    pub fn stroked(path: Path2D, stroke: Stroke2D) -> Self {
        Self { path, fill: None, stroke: Some(stroke), anchor: None }
    }

    pub fn with_anchor(mut self, anchor: Span) -> Self {
        self.anchor = Some(anchor);
        self
    }
}

/// 内容中间表示（排版产物的最终形态）。几何统一以 pt 为单位。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct ContentIR {
    pub source: ContentSource,
    pub items: Vec<StyledPath>,
    /// 墨迹包围盒（pt）。
    pub bounds: Rect2D,
    /// 基线 y（pt）。
    pub baseline: f32,
    /// 生成时字号（pt）。
    pub nominal_font_size: f32,
}

impl ContentIR {
    pub fn empty(source: ContentSource, nominal_font_size: f32) -> Self {
        Self { source, items: Vec::new(), bounds: Rect2D::default(), baseline: 0.0, nominal_font_size }
    }

    /// 墨迹包围盒 = 所有 path bounds 的并集。
    pub fn recompute_bounds(&mut self) {
        let mut acc: Option<Rect2D> = None;
        for item in &self.items {
            if let Some(pb) = item.path.compute_bounds() {
                acc = Some(match acc { Some(a) => a.union(&pb), None => pb });
            }
        }
        self.bounds = acc.unwrap_or_default();
    }

    /// 是否无可见内容（items 为空或 bounds 退化为零面积点）。
    /// 消费端据此早退，避免除零或退化缩放。
    /// 注意：水平/竖直线（一个分量为 0）不算空——它们仍有可见像素。
    pub fn is_empty(&self) -> bool {
        if self.items.is_empty() {
            return true;
        }
        let s = self.bounds.size();
        s.x.abs() < 1e-6 && s.y.abs() < 1e-6
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_notion::Vec2;

    fn rect_path(x: f32, y: f32, w: f32, h: f32) -> Path2D {
        let mut p = Path2D::new();
        p.move_to(Vec2::new(x, y))
            .line_to(Vec2::new(x + w, y))
            .line_to(Vec2::new(x + w, y + h))
            .line_to(Vec2::new(x, y + h))
            .close();
        p
    }

    #[test]
    fn content_source_normalize_equal() {
        let a = ContentSource::from_raw("x ^  2");
        let b = ContentSource::from_raw("x  ^ 2");
        assert_eq!(a.text, b.text);
        assert_eq!(a.hash, b.hash);
    }

    #[test]
    fn span_slice_and_intersect() {
        assert_eq!(Span::new(2, 5).slice("abcdefgh"), "cde");
        assert!(Span::new(0, 4).intersects(Span::new(3, 6)));
        assert_eq!(Span::new(0, 5).intersection(Span::new(3, 7)), Some(Span::new(3, 5)));
        assert_eq!(Span::new(10, 20).slice("x"), "");
    }

    #[test]
    fn bounds_union_of_paths() {
        let mut ir = ContentIR::empty(ContentSource::from_raw("ab"), 12.0);
        ir.items.push(StyledPath::filled(rect_path(0.0, 0.0, 10.0, 10.0), Color32::BLACK).with_anchor(Span::new(0, 1)));
        ir.items.push(StyledPath::filled(rect_path(5.0, 5.0, 10.0, 15.0), Color32::BLACK).with_anchor(Span::new(1, 2)));
        ir.recompute_bounds();
        assert_eq!(ir.bounds.min, Vec2::new(0.0, 0.0));
        assert_eq!(ir.bounds.max, Vec2::new(15.0, 20.0));
    }

    #[test]
    fn is_empty_detection() {
        // 空 items → is_empty
        let ir = ContentIR::empty(ContentSource::from_raw("x"), 12.0);
        assert!(ir.is_empty());

        // 非空 items + 正面积 bounds → 非空
        let mut ir2 = ContentIR::empty(ContentSource::from_raw("x"), 12.0);
        ir2.items.push(StyledPath::filled(rect_path(0.0, 0.0, 5.0, 5.0), Color32::BLACK));
        ir2.recompute_bounds();
        assert!(!ir2.is_empty());

        // items 非空但路径退化（零面积）→ is_empty
        let mut ir3 = ContentIR::empty(ContentSource::from_raw("x"), 12.0);
        ir3.items.push(StyledPath::filled(Path2D::new(), Color32::BLACK));
        ir3.recompute_bounds();
        assert!(ir3.is_empty());
    }
}
