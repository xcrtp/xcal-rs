// vector_content.rs
//
// 矢量内容 trait（预留）：宿主（节点编辑器 / SVG 录制器）实现该 trait，
// 向 2D 后端提供可遍历的图元流。与 Shape2D::Custom 的区别：
// - Shape2D 是单图元数据；VectorContent 是"可访问的场景内容"，
//   支持命中测试、批量遍历与序列化（SVG 导出）。

use xc_geometry::{Rect2D, Vec2};

use crate::shape::Shape2D;

/// 矢量内容：一块自描述的 2D 可绘制内容。
pub trait VectorContent: Send + Sync {
    /// 内容包围盒（逻辑坐标）。
    fn bounds(&self) -> Rect2D;

    /// 点是否命中内容（用于拾取/交互）。
    fn hit_test(&self, point: Vec2) -> bool;

    /// 遍历内容包含的全部图元（后端光栅化 / SVG 导出用）。
    fn shapes(&self) -> Vec<Shape2D>;
}
