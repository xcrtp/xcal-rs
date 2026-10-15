// render_backend.rs（过渡 shim）
//
// 渲染契约层下沉 kernel 后，本模块只保留两件事：
// - 重导出已下沉到 xc-resource 的纯数据类型（GpuHandle / GpuResourceId / AccessError）；
// - 定义需要 wgpu 的后端 trait（RenderBackend / RenderBackend3D），它们引用
//   `wgpu::*` 与 `crate::render_layer::RenderLayer`，无法随纯数据一起下沉。
//
// 具体实现（EframeWgpuBackend / WgpuBackend3D）位于 xcal-render-backends。

// 纯数据（无 wgpu 依赖），始终可用。
pub use xc_resource::render_backend::{AccessError, GpuHandle, GpuResourceId};

#[cfg(feature = "wgpu")]
use crate::render_layer::RenderLayer;

// ─── RenderBackend trait（资源型，wgpu） ──────────────────────────────────

/// 资源型渲染后端：提供 device/queue 句柄与 GPU 资源创建入口。
///
/// 2D/3D 资源缓存（[`crate::ResourceCache`]）通过此 trait 上传资源，
/// 与具体后端（eframe / headless）解耦。
#[cfg(feature = "wgpu")]
pub trait RenderBackend: Send + Sync {
    fn device(&self) -> &wgpu::Device;

    fn queue(&self) -> &wgpu::Queue;

    fn target_format(&self) -> wgpu::TextureFormat;

    fn create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> GpuHandle;

    fn create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> GpuHandle;

    fn create_pipeline(&self, desc: &wgpu::RenderPipelineDescriptor<'_>) -> GpuHandle;

    fn set_target(&mut self, view: Option<wgpu::TextureView>);

    fn target(&self) -> Option<&wgpu::TextureView>;

    fn begin_frame(&mut self) {
        let _ = self;
    }

    fn end_frame(&mut self) {
        let _ = self;
    }
}

// ─── RenderBackend3D trait ─────────────────────────────────────────────────

/// 3D 层提交后端：把一个 [`RenderLayer`] 录制进 command encoder。
///
/// 与 2D 的 [`RenderBackend`] 独立：3D 路径以「层」为提交单元
/// （相机 + 视口 + 绘制队列 + 渲染目标），自包含地 begin/end render pass。
#[cfg(feature = "wgpu")]
pub trait RenderBackend3D: Send + Sync {
    /// 将一个 RenderLayer 提交到 GPU：
    /// begin render pass（目标 = layer.target，clear 黑色）→
    /// 绘制 `layer.draw_queue` 全部内容 → end pass。
    ///
    /// 调用方负责提交 encoder（`queue.submit`）。
    fn submit_layer(&self, layer: &RenderLayer, encoder: &mut wgpu::CommandEncoder);
}
