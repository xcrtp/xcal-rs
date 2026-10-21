// draw_queue_3d.rs
//
// 3D 绘制命令（B2-1 起并入**统一队列** `DrawQueue`）。
//
// # 与 2D 的关系（D2）
// 计划要求「**一个 DrawQueue**，2D / 3D 命令共存」⇒ `DrawQueue3D` 不再是独立
// 队列，而是 [`DrawQueue`] 的**类型别名**（`pub type DrawQueue3D = DrawQueue`）。
// 3D 命令以 [`DrawItem::Mesh`](crate::draw::DrawItem::Mesh) /
// [`DrawItem::Line`](crate::draw::DrawItem::Line) 存入同一 `Vec`，与 2D 形状共享
// 同一个排序键 [`ZOrder`]（R-C：3D 此前无层序概念，B2-1 补齐）。
//
// 消费方按载荷过滤：2D 后端只取 `Shape`，3D 后端只取 `Mesh` / `Line`。
//
// 坐标类型使用 ze-notion 的 Vec3 / Color32；网格几何（MeshGeometry）与材质
// （Material3D）由 zela-scene 的 ECS 组件体系提供。

use std::sync::Arc;

use ze_notion::Mat4;
use ze_notion::{Color32, Vec3};
use ze_render_ir::ZOrder;
use ze_scene_contract::ObjectId;
use zela_scene::scene::components::{Material3D, MeshGeometry, PrimitiveTopology};

use crate::draw::{DrawItem, DrawQueue};

/// **兼容别名**（B2-1）：3D 队列就是统一队列，不再是独立结构。
pub type DrawQueue3D = DrawQueue;

/// 一次网格绘制命令：共享几何 + 模型变换 + 材质 + 实体身份 + 层序。
///
/// 几何用 [`Arc`] 共享：`clone` 只递增引用计数，不深拷贝顶点数据。
///
/// `entity`（B2-0a，G7 记账前提 2）：绘制该网格的 ECS 实体身份。无实体来源的绘制
/// （调试线框 / 测试）用 [`ObjectId::DANGLING`] 哨兵。
///
/// `z`（B2-1）：统一层序（缺省 [`ZOrder::DEFAULT`]）。深度遮挡仍主要由深度测试
/// 决定；层序用于同深度/透明排序与 B2-4 的 epsilon 推导。
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
    /// 统一层序（B2-1）。
    pub z: ZOrder,
}

impl MeshDraw {
    pub fn new(geometry: Arc<MeshGeometry>, transform: Mat4, material: Material3D) -> Self {
        Self {
            geometry,
            transform,
            material,
            entity: ObjectId::DANGLING,
            z: ZOrder::DEFAULT,
        }
    }

    /// 带实体身份构造（记录系统逐对象录制时用）。
    pub fn with_entity(mut self, entity: ObjectId) -> Self {
        self.entity = entity;
        self
    }

    /// 带层序构造（B2-1）。
    pub fn with_z(mut self, z: ZOrder) -> Self {
        self.z = z;
        self
    }

    /// 拓扑（转发自几何）。
    pub fn topology(&self) -> PrimitiveTopology {
        self.geometry.topology
    }
}

/// 一次线段绘制命令（世界空间两个端点）+ 实体身份 + 层序。
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
    /// 统一层序（B2-1）。
    pub z: ZOrder,
}

impl LineDraw {
    pub fn new(start: Vec3, end: Vec3, color: Color32) -> Self {
        Self {
            start,
            end,
            color,
            entity: ObjectId::DANGLING,
            z: ZOrder::DEFAULT,
        }
    }

    /// 带实体身份构造。
    pub fn with_entity(mut self, entity: ObjectId) -> Self {
        self.entity = entity;
        self
    }

    /// 带层序构造（B2-1）。
    pub fn with_z(mut self, z: ZOrder) -> Self {
        self.z = z;
        self
    }
}

impl DrawQueue {
    /// 追加一个网格绘制命令（无实体来源，entity = `ObjectId::DANGLING`，z 缺省）。
    pub fn draw_mesh(
        &mut self,
        geometry: Arc<MeshGeometry>,
        transform: Mat4,
        material: Material3D,
    ) {
        self.push_item(DrawItem::Mesh(MeshDraw::new(geometry, transform, material)));
    }

    /// 追加一个带实体身份的网格绘制命令（记录系统逐对象录制时用，G7 记账前提 2）。
    pub fn draw_mesh_for(
        &mut self,
        entity: ObjectId,
        geometry: Arc<MeshGeometry>,
        transform: Mat4,
        material: Material3D,
    ) {
        self.push_item(DrawItem::Mesh(
            MeshDraw::new(geometry, transform, material).with_entity(entity),
        ));
    }

    /// 追加一个带实体身份 + 层序的网格绘制命令（B2-1 录制路径）。
    pub fn draw_mesh_with(
        &mut self,
        entity: ObjectId,
        z: ZOrder,
        geometry: Arc<MeshGeometry>,
        transform: Mat4,
        material: Material3D,
    ) {
        self.push_item(DrawItem::Mesh(
            MeshDraw::new(geometry, transform, material)
                .with_entity(entity)
                .with_z(z),
        ));
    }

    /// 追加一条世界空间线段（无实体来源，entity = `ObjectId::DANGLING`，z 缺省）。
    pub fn draw_line(&mut self, start: Vec3, end: Vec3, color: Color32) {
        self.push_item(DrawItem::Line(LineDraw::new(start, end, color)));
    }

    /// 追加一条带实体身份的世界空间线段。
    pub fn draw_line_for(&mut self, entity: ObjectId, start: Vec3, end: Vec3, color: Color32) {
        self.push_item(DrawItem::Line(
            LineDraw::new(start, end, color).with_entity(entity),
        ));
    }

    /// 3D 网格项（按队列顺序）。
    pub fn meshes(&self) -> impl Iterator<Item = &MeshDraw> + '_ {
        self.items().iter().filter_map(|item| match item {
            DrawItem::Mesh(mesh) => Some(mesh),
            _ => None,
        })
    }

    /// 3D 线段项（按队列顺序）。
    pub fn lines(&self) -> impl Iterator<Item = &LineDraw> + '_ {
        self.items().iter().filter_map(|item| match item {
            DrawItem::Line(line) => Some(line),
            _ => None,
        })
    }

    pub fn mesh_count(&self) -> usize {
        self.meshes().count()
    }

    pub fn line_count(&self) -> usize {
        self.lines().count()
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
        assert_eq!(q.meshes().count(), 0);
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

        let cloned = q.meshes().next().unwrap().clone();
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
        assert_eq!(q.lines().count(), 2);
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
        assert_eq!(q.meshes().next().unwrap().entity, ObjectId::DANGLING);
        assert_eq!(q.lines().next().unwrap().entity, ObjectId::DANGLING);

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
        assert_eq!(q.meshes().next().unwrap().entity, id);
        assert_eq!(q.lines().next().unwrap().entity, id);
    }

    /// B2-1：3D 命令携带统一层序（缺省 `ZOrder::DEFAULT`，可显式设置）。
    #[test]
    fn test_3d_commands_carry_zorder() {
        let mut q = DrawQueue3D::new();
        q.draw_mesh(
            Arc::new(wireframe_cube(1.0)),
            Mat4::IDENTITY,
            Material3D::color(Color32::WHITE),
        );
        q.draw_line(Vec3::ZERO, Vec3::X, Color32::RED);
        assert_eq!(q.meshes().next().unwrap().z, ZOrder::DEFAULT);
        assert_eq!(q.lines().next().unwrap().z, ZOrder::DEFAULT);

        q.clear();
        q.draw_mesh_with(
            ObjectId::DANGLING,
            ZOrder::new(-3),
            Arc::new(wireframe_cube(1.0)),
            Mat4::IDENTITY,
            Material3D::color(Color32::WHITE),
        );
        assert_eq!(q.meshes().next().unwrap().z, ZOrder::new(-3));
    }
}
