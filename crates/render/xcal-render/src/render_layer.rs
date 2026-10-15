// render_layer.rs
//
// 3D 渲染层：相机 + 视口 + 绘制队列 + 渲染目标 的完整提交单元。
// 仅在 feature = "wgpu" 下编译（RenderTarget 依赖 wgpu）。

use xcal_scene::scene::components::Camera3D;

use crate::draw_queue_3d::DrawQueue3D;
use crate::render_target::RenderTarget;
use crate::viewport::Viewport;

/// 一个 3D 渲染层：相机 + 视口 + 绘制队列 + 渲染目标。
///
/// 后端通过 [`crate::RenderBackend3D::submit_layer`] 把整层提交到 GPU。
pub struct RenderLayer {
    /// 本层相机。
    pub camera: Camera3D,
    /// 本层视口（目标表面上的矩形区域）。
    pub viewport: Viewport,
    /// 本帧绘制队列。
    pub draw_queue: DrawQueue3D,
    /// 渲染目标。
    pub target: RenderTarget,
}

impl RenderLayer {
    pub fn new(target: RenderTarget, camera: Camera3D, viewport: Viewport) -> Self {
        Self {
            camera,
            viewport,
            draw_queue: DrawQueue3D::new(),
            target,
        }
    }

    /// 面向 surface 当前帧纹理构造（视口默认覆盖整张纹理）。
    ///
    /// `surface_texture` 必须在本层提交完成前保持存活。
    pub fn for_surface(surface_texture: &wgpu::SurfaceTexture, camera: Camera3D) -> Self {
        let size = surface_texture.texture.size();
        let target = RenderTarget::from_surface(surface_texture);
        Self::new(target, camera, Viewport::full(size.width, size.height))
    }

    /// 面向离屏纹理构造。
    pub fn new_offscreen(
        device: &wgpu::Device,
        camera: Camera3D,
        viewport: Viewport,
        format: wgpu::TextureFormat,
    ) -> Self {
        let size = wgpu::Extent3d {
            width: viewport.w,
            height: viewport.h,
            depth_or_array_layers: 1,
        };
        let target = RenderTarget::new_offscreen(device, size, format);
        Self::new(target, camera, viewport)
    }
}
