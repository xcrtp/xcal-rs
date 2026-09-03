// systems_3d.rs — 3D 渲染的 System
//
// - SceneRecord3DSystem(RenderCommit)：遍历 Mesh3D 组件录制到 DrawQueue3D。
//
// B 阶段（UI 解耦）后，原 RenderSubmit3DSystem 的职责（把相机+队列快照封装为
// egui_wgpu 回调提交到中央视口）由 xcal-egui::host::EguiSceneHost::present 承担。
//
// OrbitApplySystem 在 P3 已废弃（由 BindingApplySystem 统一驱动 Camera3D.eye）。

use xcal_core::context::ExecutorContext;
use xcal_core::scene::components::{Mesh3D, Transform};
use xcal_core::system::System;
use xcal_render::DrawQueue3D;

/// 每帧清空 [`DrawQueue3D`]，把世界中的全部 [`Mesh3D`]（带可选 [`Transform`]）
/// 录制进队列。
pub struct SceneRecord3DSystem;

impl System for SceneRecord3DSystem {
    type Reads = ();
    type Writes = (DrawQueue3D,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        ctx.with_world_and_resource_mut::<DrawQueue3D, _>(|world, queue| {
            queue.clear();
            // hecs 0.11：iter() 直接产出组件元组（无 Entity）。
            for (mesh, transform) in world.query::<(&Mesh3D, Option<&Transform>)>().iter() {
                let model = transform
                    .map(|t| t.matrix())
                    .unwrap_or(glam::Mat4::IDENTITY);
                queue.draw_mesh(mesh.clone(), model, mesh.color);
            }
        });
    }
}
