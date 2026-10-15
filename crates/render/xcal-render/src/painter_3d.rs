// painter_3d.rs
//
// 3D 渲染器 trait（与 Painter2D 对应）。后端（wgpu / mock / ...）实现该 trait
// 即可消费 DrawQueue3D 中的绘制命令。
//
// 坐标/颜色类型来自 xcal-geometry；网格组件 Mesh3D 由 xcal-scene ECS 体系提供。

use glam::Mat4;
use xcal_scene::scene::components::Mesh3D;
use xc_geometry::{Color32, Vec3};

/// 3D 绘制器 trait。
///
/// 与 [`crate::Painter2D`] 不同，3D 绘制以网格/线段为单位，
/// 坐标变换由模型矩阵 + 相机 VP 矩阵完成，Painter 不做坐标转换。
///
/// # Example
/// ```
/// use glam::Mat4;
/// use xcal_scene::scene::components::Mesh3D;
/// use xc_geometry::{Color32, Vec3};
/// use xcal_render::Painter3D;
///
/// /// 统计绘制次数的 mock painter。
/// struct CountingPainter {
///     meshes: usize,
///     line_batches: usize,
/// }
///
/// impl Painter3D for CountingPainter {
///     fn draw_mesh(&mut self, _mesh: &Mesh3D, _transform: &Mat4, _color: &Color32) {
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
    fn draw_mesh(&mut self, mesh: &Mesh3D, transform: &Mat4, color: &Color32);

    /// 批量绘制线段（端点已在世界空间；`transform` 额外作用于全部端点）。
    fn draw_lines(&mut self, lines: &[(Vec3, Vec3)], transform: &Mat4, color: &Color32);
}
