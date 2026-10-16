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
}
