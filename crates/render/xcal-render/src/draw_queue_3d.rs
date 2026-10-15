// draw_queue_3d.rs
//
// 3D 绘制队列：收集本帧待绘制的网格与线段。
// 与 2D DrawQueue 不同，3D 队列不做排序（深度由相机/后续 depth buffer 处理，
// MVE 阶段无深度测试，按录制顺序提交）。
//
// 坐标类型使用 xcal-geometry 的 Vec3（= glam::Vec3）/ Color32；
// 网格组件 Mesh3D 仍由 xcal-scene 的 ECS 组件体系提供。

use glam::Mat4;
use xcal_scene::scene::components::{Mesh3D, PrimitiveTopology};
use xc_geometry::{Color32, Vec3};

/// 一次网格绘制命令：网格 + 模型变换 + 覆盖颜色。
#[derive(Debug, Clone)]
pub struct MeshDraw {
    /// 网格数据（顶点/索引/拓扑）。
    pub mesh: Mesh3D,
    /// 模型矩阵（模型空间 → 世界空间）。
    pub transform: Mat4,
    /// 本次绘制颜色（通常等于 `mesh.color`）。
    pub color: Color32,
}

impl MeshDraw {
    pub fn new(mesh: Mesh3D, transform: Mat4, color: Color32) -> Self {
        Self {
            mesh,
            transform,
            color,
        }
    }

    /// 拓扑（转发自网格）。
    pub fn topology(&self) -> PrimitiveTopology {
        self.mesh.topology
    }
}

/// 一次线段绘制命令（世界空间两个端点）。
#[derive(Debug, Clone, Copy)]
pub struct LineDraw {
    /// 线段起点（世界空间）。
    pub start: Vec3,
    /// 线段终点（世界空间）。
    pub end: Vec3,
    /// 线段颜色。
    pub color: Color32,
}

impl LineDraw {
    pub fn new(start: Vec3, end: Vec3, color: Color32) -> Self {
        Self { start, end, color }
    }
}

/// 3D 绘制队列：每帧由 SceneRecord3DSystem 填充、RenderSubmit3DSystem 消费。
///
/// # Example
/// ```
/// use glam::Mat4;
/// use xcal_scene::scene::primitives::wireframe_cube;
/// use xc_geometry::Color32;
/// use xcal_render::DrawQueue3D;
///
/// let white = Color32::new(255, 255, 255, 255);
/// let mut queue = DrawQueue3D::new();
/// assert!(queue.is_empty());
///
/// queue.draw_mesh(wireframe_cube(1.0, white), Mat4::IDENTITY, white);
/// assert_eq!(queue.mesh_count(), 1);
///
/// queue.clear();
/// assert!(queue.is_empty());
/// ```
#[derive(Default)]
pub struct DrawQueue3D {
    meshes: Vec<MeshDraw>,
    lines: Vec<LineDraw>,
}

impl DrawQueue3D {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_capacity(mesh_cap: usize, line_cap: usize) -> Self {
        Self {
            meshes: Vec::with_capacity(mesh_cap),
            lines: Vec::with_capacity(line_cap),
        }
    }

    /// 追加一个网格绘制命令。
    pub fn draw_mesh(&mut self, mesh: Mesh3D, transform: Mat4, color: Color32) {
        self.meshes.push(MeshDraw::new(mesh, transform, color));
    }

    /// 追加一条世界空间线段。
    pub fn draw_line(&mut self, start: Vec3, end: Vec3, color: Color32) {
        self.lines.push(LineDraw::new(start, end, color));
    }

    /// 清空队列（每帧录制前调用）。
    pub fn clear(&mut self) {
        self.meshes.clear();
        self.lines.clear();
    }

    pub fn meshes(&self) -> &[MeshDraw] {
        &self.meshes
    }

    pub fn lines(&self) -> &[LineDraw] {
        &self.lines
    }

    pub fn mesh_count(&self) -> usize {
        self.meshes.len()
    }

    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.meshes.is_empty() && self.lines.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_scene::scene::primitives::wireframe_cube;

    #[test]
    fn test_queue_starts_empty() {
        let q = DrawQueue3D::new();
        assert!(q.is_empty());
        assert_eq!(q.mesh_count(), 0);
        assert_eq!(q.line_count(), 0);
    }

    #[test]
    fn test_push_mesh_then_clear() {
        let mut q = DrawQueue3D::new();
        let cube = wireframe_cube(1.0, Color32::new(255, 255, 255, 255));
        q.draw_mesh(cube, Mat4::IDENTITY, Color32::new(255, 0, 0, 255));
        assert_eq!(q.mesh_count(), 1);
        assert!(!q.is_empty());

        q.clear();
        assert!(q.is_empty());
        assert_eq!(q.meshes().len(), 0);
    }

    #[test]
    fn test_push_lines() {
        let mut q = DrawQueue3D::new();
        q.draw_line(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Color32::new(255, 255, 255, 255),
        );
        q.draw_line(
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Color32::new(255, 255, 255, 255),
        );
        assert_eq!(q.line_count(), 2);
        assert_eq!(q.lines().len(), 2);
    }
}
