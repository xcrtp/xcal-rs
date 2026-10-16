// style.rs
//
// 2D 样式组件 + 「样式组件 → Style2D」的解析 trait（唯一换算点）。
//
// M2：自 `xcal-render`（原 `components.rs` 的 `render` 子模块 + `draw.rs` 的
// `ResolveStyle`）迁入渲染 IR 层。理由：它们是**载荷声明**（纯数据 + 一条纯函数），
// 与 ECS / 后端无关；渲染管线只是消费者。
//
// ⚠️ **唯一换算点**：描边宽度在此经 `resolve_width` 结合 scale 解析为屏幕像素。
// 项目中其它任何地方都**不得**再乘除 `scale` —— 否则会出现"某些描边乘过、
// 某些没乘"的不一致（见 `MeasureSpace`）。

use xc_notion::{Color32, MeasureSpace, resolve_width};

use crate::shape::Style2D;

/// 描边样式组件：颜色 + 宽度 + 宽度所属空间。
///
/// `space` 声明宽度的空间归属（默认 [`MeasureSpace::Screen`]）；
/// **唯一换算点**是 [`ResolveStyle::resolve_style`]（由 `record_scene` 结合
/// 相机 scale 调用）。
#[derive(Debug, Clone)]
pub struct StrokeSolid {
    pub width: f32,
    pub color: Color32,
    pub space: MeasureSpace,
}

impl StrokeSolid {
    /// 两参构造：`space` 取默认 [`MeasureSpace::Screen`]（既有调用点行为不变）。
    pub fn new(width: f32, color: impl Into<Color32>) -> Self {
        Self {
            width,
            color: color.into(),
            space: MeasureSpace::Screen,
        }
    }

    /// 显式指定度量空间的构造。
    pub fn new_in(width: f32, color: impl Into<Color32>, space: MeasureSpace) -> Self {
        Self {
            width,
            color: color.into(),
            space,
        }
    }
}

/// 填充样式组件：单一颜色。
#[derive(Debug, Clone)]
pub struct FillSolid {
    pub color: Color32,
}

impl FillSolid {
    pub fn new(color: impl Into<Color32>) -> Self {
        Self {
            color: color.into(),
        }
    }
}

// ─── 不透明度（N4：淡入淡出的通道） ────────────────────────────────────────

/// **不透明度组件**：`0.0` 全透明、`1.0` 完全不透明。**缺席 = 1.0**（不调制）。
///
/// # 为什么是独立组件，而不是改 `StrokeSolid::color` 的 alpha
///
/// 1. `Color32` 的 alpha 是 `u8`，而动画值是 `f32` ⇒ 每帧量化，并且要走
///    "读回原色再改 alpha"的往返（对象上的组件是无状态的声明，不是可变缓冲）；
/// 2. 淡入淡出通常要**同时**作用于描边与填充、以及 `Reveal` / `Morph` 项 ⇒
///    独立组件天然覆盖全部绘制路径，施加点只有一处（`xcal-render::draw`）。
///
/// # 预乘 alpha（硬约束，务必按此理解）
///
/// 本管线里 `Style2D` 存的是**直色**（straight，未预乘）；预乘发生在 GPU 侧
/// （`wgpu_2d::uniform_bytes` 写 `rgb * a`）。因此"施加不透明度 `o`"= **把直色的
/// `a` 乘上 `o`（`rgb` 不动）**：预乘后自然得到 `rgb_premult * o` 与 `a * o`，
/// 恰是"整层透明度乘 o"的正确结果。
///
/// - ❌ 反例（本计划的初稿写法）：把 `rgb` 与 `a` **都**乘 `o` ⇒ GPU 再乘一次 `a`
///   ⇒ 预乘后 rgb 变成 `o²` 倍，画面**偏暗**（`o = 0.5` 时应为 `r ≈ a ≈ 128`，
///   那样会得到 `r ≈ 64、a ≈ 128`）；
/// - ⚠️ "rgb 与 a 同乘"这条法则适用于**已经预乘**的数据 —— 即合成器的 per-layer
///   opacity（C 线）—— 不要照搬到直色上。
///
/// # `Default` = 1.0
///
/// 与 `ZOrder` 的"缺席即默认"惯例一致，但中性值不是 0 而是 1.0 ——
/// 故**不 derive** `Default`，显式实现以免得到 `0.0`（全透明）这种危险缺省。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Opacity(pub f32);

impl Opacity {
    /// 完全不透明（= 缺席语义）。
    pub const FULL: Self = Self(1.0);

    /// 以给定不透明度构造（`0.0` 全透明、`1.0` 完全不透明）。
    pub const fn new(value: f32) -> Self {
        Self(value)
    }

    /// 把不透明度施加到一个**直色**上（只缩放 alpha；见类型文档的预乘约束）。
    ///
    /// 这是全仓**唯一**的"不透明度乘法"。`o == 1.0` 时逐位恒等
    /// （`a * 1.0` 取整回来还是 `a`）⇒ 未挂组件的对象画面不变。
    pub fn apply(self, color: Color32) -> Color32 {
        let a = (color.a() as f32 * self.0).round().clamp(0.0, 255.0) as u8;
        Color32::new(color.r(), color.g(), color.b(), a)
    }

    /// 是否等价于"不调制"（唯一真值：恰好 1.0）。
    pub fn is_full(self) -> bool {
        self.0 == 1.0
    }
}

impl Default for Opacity {
    fn default() -> Self {
        Self::FULL
    }
}

/// 填充样式（当前只有纯色）。
pub enum FillStyle {
    Solid(FillSolid),
}

/// 描边样式（当前只有纯色）。
pub enum StrokeStyle {
    Solid(StrokeSolid),
}

/// 把样式组件解析为**屏幕空间**的 [`Style2D`]。
///
/// ⚠️ **本 trait 是唯一换算点**：描边宽度在此经 [`resolve_width`] 结合
/// `scale` 解析为屏幕像素。项目中其它任何地方都**不得**再乘除 `scale`。
///
/// 公开以允许下游 crate 为新样式组件提供实现（IR 层自身覆盖
/// [`StrokeSolid`] / [`FillSolid`]）。
pub trait ResolveStyle {
    fn resolve_style(&self, scale: f32) -> Style2D;
}

impl ResolveStyle for StrokeSolid {
    fn resolve_style(&self, scale: f32) -> Style2D {
        Style2D {
            fill: None,
            stroke: Some(self.color),
            stroke_width: resolve_width(self.width, self.space, scale),
            // 已解析为屏幕像素，不再是世界度量。
            space: MeasureSpace::Screen,
        }
    }
}

impl ResolveStyle for FillSolid {
    fn resolve_style(&self, _scale: f32) -> Style2D {
        Style2D {
            fill: Some(self.color),
            stroke: None,
            stroke_width: 0.0,
            space: MeasureSpace::Screen,
        }
    }
}

/// 队列便捷方法的样式构造：**不做** scale 换算（调用方给的就是屏幕空间量，
/// 与迁移前的队列行为一致；世界→屏幕换算只发生在 `record_scene`）。
pub fn fill_style(fill: &FillSolid) -> Style2D {
    Style2D {
        fill: Some(fill.color),
        stroke: None,
        stroke_width: 0.0,
        space: MeasureSpace::Screen,
    }
}

/// 同 [`fill_style`]，描边版：不高亮、不换算，原样进队列。
pub fn stroke_style_raw(stroke: &StrokeSolid) -> Style2D {
    Style2D {
        fill: None,
        stroke: Some(stroke.color),
        stroke_width: stroke.width,
        space: stroke.space,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两参构造默认 `Screen`（保证现有调用点行为不变）。
    #[test]
    fn stroke_solid_default_construction_is_screen() {
        let s = StrokeSolid::new(2.0, (255, 0, 0, 255));
        assert_eq!(s.space, MeasureSpace::Screen);
        assert_eq!(s.width, 2.0);

        let w = StrokeSolid::new_in(1.0, (0, 0, 0, 255), MeasureSpace::World);
        assert_eq!(w.space, MeasureSpace::World);
    }

    /// 唯一换算点：`World` 宽度按 scale 线性放大，`Screen` 不变。
    #[test]
    fn resolve_style_is_the_single_conversion_point() {
        let world = StrokeSolid::new_in(2.0, (255, 0, 0, 255), MeasureSpace::World);
        assert_eq!(world.resolve_style(10.0).stroke_width, 20.0);
        assert_eq!(world.resolve_style(10.0).space, MeasureSpace::Screen);

        let screen = StrokeSolid::new(2.0, (255, 0, 0, 255));
        assert_eq!(screen.resolve_style(10.0).stroke_width, 2.0);
    }

    #[test]
    fn resolve_fill_solid_has_no_stroke() {
        let s = FillSolid::new((1u8, 2, 3, 4)).resolve_style(5.0);
        assert_eq!(s.fill, Some(Color32::from_rgba(1, 2, 3, 4)));
        assert_eq!(s.stroke, None);
        assert_eq!(s.stroke_width, 0.0);
    }

    /// **N4 核心（可证伪）**：不透明度只缩 **alpha**，`rgb` 一位不动。
    ///
    /// 证伪：改成 `rgb` 与 `a` 同乘 ⇒ 断言失败 —— 而这正是"看起来对、
    /// 实际预乘后偏暗一档"的错法（见 `Opacity` 的预乘约束）。
    #[test]
    fn opacity_scales_alpha_only() {
        let opaque = Color32::new(255, 0, 0, 255);
        let half = Opacity::new(0.5).apply(opaque);
        assert_eq!(
            (half.r(), half.g(), half.b()),
            (255, 0, 0),
            "rgb 不得被不透明度改动（预乘发生在 GPU 侧，rgb 乘了会变 o²）"
        );
        // 255 * 0.5 = 127.5 → 四舍五入 128（与交付验收的 `r ≈ a ≈ 128` 同口径）。
        assert_eq!(half.a(), 128, "alpha 应减半（127.5 四舍五入到 128）");

        // 半透明直色：alpha 再乘一次，rgb 仍不动（预乘后 = 原预乘值 × 0.5）。
        let semi = Color32::new(200, 100, 50, 128);
        let quarter = Opacity::new(0.5).apply(semi);
        assert_eq!((quarter.r(), quarter.g(), quarter.b()), (200, 100, 50));
        assert_eq!(quarter.a(), 64, "128 * 0.5 = 64");
    }

    /// `Opacity::FULL`（= 缺席语义）**逐位恒等**；`Default` 也是 1.0（不是 0）。
    #[test]
    fn opacity_full_is_bitwise_identity_and_default_is_one() {
        assert_eq!(Opacity::default(), Opacity::FULL);
        assert_eq!(Opacity::default().0, 1.0, "Default 必须是 1.0 而不是 0.0");
        assert!(Opacity::FULL.is_full());
        for c in [
            Color32::new(255, 0, 0, 255),
            Color32::new(1, 2, 3, 4),
            Color32::TRANSPARENT,
            Color32::new(10, 20, 30, 128),
        ] {
            assert_eq!(Opacity::FULL.apply(c), c, "不透明度 1.0 必须逐位恒等");
        }
    }

    /// `o = 0` ⇒ alpha 归零（预乘后 rgb 也零 ⇒ 不产生可见像素）。
    #[test]
    fn opacity_zero_clears_alpha() {
        let zero = Opacity::new(0.0).apply(Color32::new(255, 255, 255, 255));
        assert_eq!(zero.a(), 0);
        assert_eq!((zero.r(), zero.g(), zero.b()), (255, 255, 255));
    }
}
