// painter_2d.rs
//
// 新风格 2D Painter trait（预留）。
//
// 注意：`draw.rs` 中的 `Painter2D` + `Shape2D`/`Style2D` + `record_scene` 是既有
// 录制 API，保持不变并继续工作。本模块定义的 `Painter2DTarget` 是面向
// `Shape2D` 新图元体系的光栅化目标，供新后端实现。
// S4.1 后生产 2D 路径为 `DrawQueue` + `wgpu_2d`（lyon 三角化），本研究性
// trait 尚无实现。

use xc_notion::{Transform2D, Vec2};

use crate::shape::{Shape2D, Style2D};

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
