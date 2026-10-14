// painter_2d.rs
//
// 新风格 2D Painter trait（预留）。
//
// 注意：现有 2D 管线（draw.rs 中的 Painter2D + Shape/Style + record_scene）
// 是冻结 API，保持不变并继续工作。本模块定义的 Painter2DTarget 是面向
// Shape2D（shape.rs）新图元体系的光栅化目标，供新后端（SVG/未来 GPU 2D）
// 实现，后续逐步替换旧管线。

use xc_geometry::{Color32, MeasureSpace, Transform2D, Vec2};

use crate::shape::Shape2D;

/// 2D 绘制样式（新管线：描边 + 填充）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style2D {
    /// 填充色（None = 不填充）。
    pub fill: Option<Color32>,
    /// 描边色（None = 不描边）。
    pub stroke: Option<Color32>,
    /// 描边宽度（单位由 [`Style2D::space`] 声明；默认屏幕像素）。
    pub stroke_width: f32,
    /// 描边宽度所属空间（声明式）。IR 转换层（`content_ir`）在此声明意图，
    /// 由消费端把它转成 `StrokeSolid.space`，最终在 `record_scene`
    /// （**唯一换算点**）结合 `camera.scale` 解析为屏幕像素。
    pub space: MeasureSpace,
}

impl Default for Style2D {
    fn default() -> Self {
        Self {
            fill: Some(Color32::WHITE),
            stroke: None,
            stroke_width: 1.0,
            space: MeasureSpace::Screen,
        }
    }
}

/// 新风格 2D 绘制目标。
///
/// 与旧 `Painter2D::paint(&self, shape, style)` 的区别：
/// - `&mut self`：后端可累积命令（GPU 录制 / SVG 缓冲）；
/// - 显式 transform：图元坐标与画布坐标解耦；
/// - 图元为 [`Shape2D`]，含 Custom 扩展点。
pub trait Painter2DTarget {
    /// 开始一帧（后端可在此清空命令缓冲）。
    fn begin_frame(&mut self, logical_size: Vec2);

    /// 绘制一个图元（应用给定变换与样式）。
    fn draw_shape(&mut self, shape: &Shape2D, transform: &Transform2D, style: &Style2D);

    /// 结束一帧（后端提交/刷新）。
    fn end_frame(&mut self);
}
