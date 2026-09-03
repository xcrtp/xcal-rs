// systems_3d.rs — 3D 渲染的两个 System
//
// - SceneRecord3DSystem(RenderCommit)：遍历 Mesh3D 组件录制到 DrawQueue3D
// - RenderSubmit3DSystem(FrameEnd)：把相机+队列快照封装为 egui_wgpu 回调，
//   绘制进 eframe 主 render pass（中央视口，clip 自动排除侧边栏）。
//
// OrbitApplySystem 在 P3 已废弃（由 BindingApplySystem 统一驱动 Camera3D.eye）。

use xcal_core::context::ExecutorContext;
use xcal_core::scene::components::{Camera3D, Mesh3D, Transform};
use xcal_core::system::System;
use xcal_egui::resources::{EguiFrameContext, ViewRect};
use xcal_render::DrawQueue3D;
use xcal_render_backends::Layer3DCallback;

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

// ─── RenderSubmit3DSystem ──────────────────────────────────────────────────

/// 取相机组件 + [`DrawQueue3D`] 快照，通过 egui_wgpu 回调提交到 GPU。
///
/// 回调挂在中央视口的 painter（与 2D 内容同层，shape 顺序在 2D 之后 →
/// 3D 线框绘制于 2D 内容之上；回调 clip 限定在中央区域，侧边面板不受影响）。
pub struct RenderSubmit3DSystem;

impl System for RenderSubmit3DSystem {
    type Reads = (EguiFrameContext, DrawQueue3D, ViewRect);
    type Writes = ();

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 裸指针 Copy 出来即释放对 ctx 的借用（与 2D systems 相同模式）。
        let ui_ptr = ctx
            .get::<EguiFrameContext>()
            .map(|c| c.ui_ptr())
            .unwrap_or(std::ptr::null_mut());
        if ui_ptr.is_null() {
            return;
        }

        // 相机是 World 组件（每帧可能已被 OrbitApplySystem 更新）。
        let Some(camera) = ctx.world.query::<&Camera3D>().iter().next().copied() else {
            return;
        };

        // 中央视口矩形（与 2D 渲染区域一致，自动避开侧边面板）。
        let ui = unsafe { &*ui_ptr };
        let view_rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| xcal_core::types::Rect::from(ui.max_rect()));
        let egui_rect = view_rect.into_egui_rect();
        let aspect = egui_rect.width() / egui_rect.height().max(1.0);

        // 队列快照进回调；空队列直接跳过（无 3D 内容时不注册回调）。
        let Some(queue) = ctx.get::<DrawQueue3D>() else {
            return;
        };
        if queue.is_empty() {
            return;
        }
        let callback = Layer3DCallback::new(camera, aspect, queue);

        let painter = ui.painter_at(egui_rect);
        painter.add(egui::epaint::Shape::Callback(
            egui_wgpu::Callback::new_paint_callback(egui_rect, callback),
        ));
    }
}
