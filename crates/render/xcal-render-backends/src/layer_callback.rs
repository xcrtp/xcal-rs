// layer_callback.rs
//
// egui_wgpu 回调：把 3D 内容绘制进 eframe 主 render pass。
// （eframe 不暴露 surface texture，官方扩展点即 CallbackTrait。）
// 由 xcal-render/src/backends/wgpu_3d.rs 迁移，逻辑不变。

use xcal_core::scene::components::Camera3D;
use xcal_render::DrawQueue3D;
use xcal_render::draw_queue_3d::{LineDraw, MeshDraw};

use crate::wgpu_3d::{Frame3DData, WgpuBackend3D};

/// 每帧快照：相机 + 宽高比 + 本帧绘制内容。
///
/// 通过 [`egui_wgpu::Callback::new_paint_callback`] 加入 egui 绘制列表；
/// `prepare` 阶段上传 GPU 资源，`paint` 阶段在 eframe 主 render pass 中绘制。
pub struct Layer3DCallback {
    camera: Camera3D,
    aspect: f32,
    meshes: Vec<MeshDraw>,
    lines: Vec<LineDraw>,
}

impl Layer3DCallback {
    /// 从相机与绘制队列抓取快照（队列在 RenderCommit 阶段已录制完成）。
    pub fn new(camera: Camera3D, aspect: f32, queue: &DrawQueue3D) -> Self {
        Self {
            camera,
            aspect,
            meshes: queue.meshes().to_vec(),
            lines: queue.lines().to_vec(),
        }
    }
}

impl egui_wgpu::CallbackTrait for Layer3DCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen_descriptor: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(backend) = callback_resources.get::<WgpuBackend3D>() else {
            return Vec::new();
        };
        let view_proj = self.camera.view_projection(self.aspect);
        let frame = backend.prepare_draws(device, queue, view_proj, &self.meshes, &self.lines);
        callback_resources.insert(frame);
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(backend) = callback_resources.get::<WgpuBackend3D>() else {
            return;
        };
        let Some(frame) = callback_resources.get::<Frame3DData>() else {
            return;
        };
        // 裁剪到回调区域，避免 3D 内容覆盖 egui 面板。
        let clip = info.clip_rect_in_pixels();
        render_pass.set_scissor_rect(
            clip.left_px.max(0) as u32,
            clip.top_px.max(0) as u32,
            clip.width_px.max(0) as u32,
            clip.height_px.max(0) as u32,
        );
        backend.record_prepared(&frame.draws, render_pass);
    }
}
