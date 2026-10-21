// draw_queue_3d.rs
//
// 3D 绘制队列：收集本帧待绘制的网格与线段。
// 与 2D DrawQueue 不同，3D 队列不做排序：遮挡由 3D 后端离屏 pass 的深度测试
// 决定（RC.3，Depth32Float + CompareFunction::Less），队列本身按录制顺序提交。
//
// 坐标类型使用 ze-notion 的 Vec3（= ze_notion::Vec3）/ Color32；
// 网格几何（MeshGeometry）与材质（Material3D）由 zela-scene 的 ECS 组件体系提供。

use std::sync::Arc;

use ze_notion::Mat4;
use ze_notion::{Color32, Vec3};
use ze_scene_contract::ObjectId;
use zela_scene::scene::components::{Material3D, MeshGeometry, PrimitiveTopology};

/// 一次网格绘制命令：共享几何 + 模型变换 + 材质 + **实体身份**。
///
/// 几何用 [`Arc`] 共享：`clone` 只递增引用计数，不深拷贝顶点数据。
///
/// `entity`（B2-0a，G7 记账前提 2）：绘制该网格的 ECS 实体身份。无实体来源的绘制
/// （调试线框 / 测试）用 [`ObjectId::DANGLING`] 哨兵。
#[derive(Debug, Clone)]
pub struct MeshDraw {
    /// 网格几何（顶点/法线/UV/索引/拓扑），多引用者共享。
    pub geometry: Arc<MeshGeometry>,
    /// 模型矩阵（模型空间 → 世界空间）。
    pub transform: Mat4,
    /// 本次绘制材质（颜色/纹理/UV 兜底投影）。
    pub material: Material3D,
    /// 绘制该网格的实体身份（`ObjectId::DANGLING` = 无实体）。
    pub entity: ObjectId,
}

impl MeshDraw {
    pub fn new(geometry: Arc<MeshGeometry>, transform: Mat4, material: Material3D) -> Self {
        Self {
            geometry,
            transform,
            material,
            entity: ObjectId::DANGLING,
        }
    }

    /// 带实体身份构造（记录系统逐对象录制时用）。
    pub fn with_entity(mut self, entity: ObjectId) -> Self {
        self.entity = entity;
        self
    }

    /// 拓扑（转发自几何）。
    pub fn topology(&self) -> PrimitiveTopology {
        self.geometry.topology
    }
}

/// 一次线段绘制命令（世界空间两个端点）+ 实体身份。
#[derive(Debug, Clone, Copy)]
pub struct LineDraw {
    /// 线段起点（世界空间）。
    pub start: Vec3,
    /// 线段终点（世界空间）。
    pub end: Vec3,
    /// 线段颜色。
    pub color: Color32,
    /// 绘制该线段的实体身份（`ObjectId::DANGLING` = 无实体，B2-0a）。
    pub entity: ObjectId,
}

impl LineDraw {
    pub fn new(start: Vec3, end: Vec3, color: Color32) -> Self {
        Self {
            start,
            end,
            color,
            entity: ObjectId::DANGLING,
        }
    }

    /// 带实体身份构造。
    pub fn with_entity(mut self, entity: ObjectId) -> Self {
        self.entity = entity;
        self
    }
}

/// 3D 绘制队列：每帧由 SceneRecord3DSystem 填充、RenderSubmit3DSystem 消费。
///
/// # Example
/// ```
/// use std::sync::Arc;
///
/// use ze_notion::Mat4;
/// use zela_render::DrawQueue3D;
/// use zela_scene::scene::components::Material3D;
/// use zela_scene::scene::primitives::wireframe_cube;
/// use ze_notion::Color32;
///
/// let white = Color32::new(255, 255, 255, 255);
/// let mut queue = DrawQueue3D::new();
/// assert!(queue.is_empty());
///
/// queue.draw_mesh(
///     Arc::new(wireframe_cube(1.0)),
///     Mat4::IDENTITY,
///     Material3D::color(white),
/// );
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

    /// 追加一个网格绘制命令（无实体来源，entity = `ObjectId::DANGLING`）。
    pub fn draw_mesh(
        &mut self,
        geometry: Arc<MeshGeometry>,
        transform: Mat4,
        material: Material3D,
    ) {
        self.meshes
            .push(MeshDraw::new(geometry, transform, material));
    }

    /// 追加一个带实体身份的网格绘制命令（记录系统逐对象录制时用，G7 记账前提 2）。
    pub fn draw_mesh_for(
        &mut self,
        entity: ObjectId,
        geometry: Arc<MeshGeometry>,
        transform: Mat4,
        material: Material3D,
    ) {
        self.meshes
            .push(MeshDraw::new(geometry, transform, material).with_entity(entity));
    }

    /// 追加一条世界空间线段（无实体来源，entity = `ObjectId::DANGLING`）。
    pub fn draw_line(&mut self, start: Vec3, end: Vec3, color: Color32) {
        self.lines.push(LineDraw::new(start, end, color));
    }

    /// 追加一条带实体身份的世界空间线段。
    pub fn draw_line_for(&mut self, entity: ObjectId, start: Vec3, end: Vec3, color: Color32) {
        self.lines
            .push(LineDraw::new(start, end, color).with_entity(entity));
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
    use zela_scene::scene::primitives::wireframe_cube;

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
        let cube = Arc::new(wireframe_cube(1.0));
        q.draw_mesh(
            cube,
            Mat4::IDENTITY,
            Material3D::color(Color32::new(255, 0, 0, 255)),
        );
        assert_eq!(q.mesh_count(), 1);
        assert!(!q.is_empty());

        q.clear();
        assert!(q.is_empty());
        assert_eq!(q.meshes().len(), 0);
    }

    /// `MeshDraw` 的 `clone` 只递增几何引用计数，不深拷贝顶点数据。
    #[test]
    fn test_mesh_draw_clone_shares_geometry() {
        let mut q = DrawQueue3D::new();
        let geometry = Arc::new(wireframe_cube(1.0));
        q.draw_mesh(
            Arc::clone(&geometry),
            Mat4::IDENTITY,
            Material3D::color(Color32::WHITE),
        );
        assert_eq!(Arc::strong_count(&geometry), 2);

        let cloned = q.meshes()[0].clone();
        assert_eq!(Arc::strong_count(&geometry), 3);
        assert!(Arc::ptr_eq(&cloned.geometry, &geometry));
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

    /// B2-0a：队列命令携带实体身份（G7 记账前提 2）。
    ///
    /// 无来源绘制 = `DANGLING`（哨兵），逐对象录制 = 真实 `ObjectId`；
    /// mesh 与 line 两条路径都要有。
    #[test]
    fn test_draw_commands_carry_entity_identity() {
        // 默认构造：哨兵
        let mut q = DrawQueue3D::new();
        q.draw_mesh(
            Arc::new(wireframe_cube(1.0)),
            Mat4::IDENTITY,
            Material3D::color(Color32::WHITE),
        );
        q.draw_line(Vec3::ZERO, Vec3::X, Color32::RED);
        assert_eq!(q.meshes()[0].entity, ObjectId::DANGLING);
        assert_eq!(q.lines()[0].entity, ObjectId::DANGLING);

        // 带身份构造：真实 ObjectId 原样保留
        let id = ObjectId::new(7, 3);
        q.clear();
        q.draw_mesh_for(
            id,
            Arc::new(wireframe_cube(1.0)),
            Mat4::IDENTITY,
            Material3D::color(Color32::WHITE),
        );
        q.draw_line_for(id, Vec3::ZERO, Vec3::Y, Color32::GREEN);
        assert_eq!(q.meshes()[0].entity, id);
        assert_eq!(q.lines()[0].entity, id);
    }
}
