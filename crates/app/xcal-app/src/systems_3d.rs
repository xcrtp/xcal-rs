// systems_3d.rs — 3D 渲染的 System
//
// - SceneRecord3DSystem(RenderCommit)：遍历 MeshGeometry 组件（+可选 Material3D）
//   录制到 DrawQueue3D。
//
// B 阶段（UI 解耦）后，原 RenderSubmit3DSystem 的职责（把相机+队列快照封装为
// egui_wgpu 回调提交到中央视口）由 xcal-egui::host::EguiSceneHost::present 承担。
//
// OrbitApplySystem 在 P3 已废弃（由 BindingApplySystem 统一驱动 Camera3D.eye）。

use std::sync::Arc;

use xcal_schedule::context::ExecutorContext;
use xcal_scene::scene::components::{Material3D, MeshGeometry, Transform};
use xcal_schedule::system::System;
use xcal_render::DrawQueue3D;

/// 每帧清空 [`DrawQueue3D`]，把世界中的全部 [`MeshGeometry`]（带可选
/// [`Material3D`] 与可选 [`Transform`]）录制进队列。
pub struct SceneRecord3DSystem;

impl System for SceneRecord3DSystem {
    type Reads = ();
    type Writes = (DrawQueue3D,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        ctx.with_world_and_resource_mut::<DrawQueue3D, _>(|world, queue| {
            queue.clear();
            // hecs 0.11：iter() 直接产出组件元组（无 Entity）。
            for (mesh, material, transform) in world
                .query::<(&Arc<MeshGeometry>, Option<&Material3D>, Option<&Transform>)>()
                .iter()
            {
                let model = transform
                    .map(|t| t.matrix())
                    .unwrap_or(glam::Mat4::IDENTITY);
                let material = material.copied().unwrap_or_default();
                // 几何以 Arc 持有：此处仅克隆引用计数，不再深拷贝顶点数据。
                queue.draw_mesh(Arc::clone(mesh), model, material);
            }
        });
    }
}
