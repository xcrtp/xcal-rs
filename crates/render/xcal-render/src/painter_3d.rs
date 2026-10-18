// painter_3d.rs
//
// 3D 渲染器 trait（与 Painter2D 对应）。后端（wgpu / mock / ...）实现该 trait
// 即可消费 DrawQueue3D 中的绘制命令。
//
// 坐标/颜色类型来自 xc-notion；几何/材质组件由 xcal-scene ECS 体系提供。

use std::sync::Arc;

use glam::Mat4;
use xc_notion::{Color32, Vec3};
use xcal_scene::scene::components::{Material3D, MeshGeometry};

/// 3D 绘制器 trait。
///
/// 与 [`crate::Painter2D`] 不同，3D 绘制以网格/线段为单位，
/// 坐标变换由模型矩阵 + 相机 VP 矩阵完成，Painter 不做坐标转换。
///
/// # 当前状态
///
/// 本 trait **零生产实现**（仅下方示例中的 mock）：实际 3D 渲染路径是
/// [`crate::DrawQueue3D`] 录制 + 后端直接消费 [`crate::MeshDraw`]，
/// 从未经由 `Painter3D`。`mesh` 参数已与
/// [`crate::DrawQueue3D::draw_mesh`] 对齐为 `&Arc<MeshGeometry>`
/// （RB/RC 起几何以 `Arc` 持有，避免每帧深拷贝）。本 trait 的去留待 S3-4 决定。
///
/// # Example
/// ```
/// use std::sync::Arc;
/// use glam::Mat4;
/// use xcal_scene::scene::components::{Material3D, MeshGeometry};
/// use xc_notion::{Color32, Vec3};
/// use xcal_render::Painter3D;
///
/// /// 统计绘制次数的 mock painter。
/// struct CountingPainter {
///     meshes: usize,
///     line_batches: usize,
/// }
///
/// impl Painter3D for CountingPainter {
///     fn draw_mesh(
///         &mut self,
///         _mesh: &Arc<MeshGeometry>,
///         _transform: &Mat4,
///         _material: &Material3D,
///     ) {
///         self.meshes += 1;
///     }
///     fn draw_lines(&mut self, lines: &[(Vec3, Vec3)], _transform: &Mat4, _color: &Color32) {
///         self.line_batches += lines.len();
///     }
/// }
///
/// let mut painter = CountingPainter { meshes: 0, line_batches: 0 };
/// painter.draw_lines(
///     &[(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0))],
///     &Mat4::IDENTITY,
///     &Color32::new(255, 0, 0, 255),
/// );
/// assert_eq!(painter.line_batches, 1);
/// ```
pub trait Painter3D {
    /// 绘制一个网格（模型空间顶点，经 `transform` 变换到世界空间）。
    ///
    /// `mesh` 以 `&Arc<MeshGeometry>` 传入，与 [`crate::DrawQueue3D::draw_mesh`]
    /// 保持一致：几何由调用方以 `Arc` 持有，绘制路径只做引用计数而非深拷贝。
    fn draw_mesh(&mut self, mesh: &Arc<MeshGeometry>, transform: &Mat4, material: &Material3D);

    /// 批量绘制线段（端点已在世界空间；`transform` 额外作用于全部端点）。
    fn draw_lines(&mut self, lines: &[(Vec3, Vec3)], transform: &Mat4, color: &Color32);
}
