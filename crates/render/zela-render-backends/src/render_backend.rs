// render_backend.rs（B3-0.5e：从 `zela-render` 迁入）
//
// 渲染契约层下沉 kernel 后，本模块只保留两件事：
// - 重导出已下沉到 ze-resource 的纯数据类型（GpuHandle / GpuResourceId / AccessError）；
// - 定义需要 wgpu 的后端 trait（RenderBackend / RenderBackend3D）。
//
// 具体实现（EframeWgpuBackend / WgpuBackend3D）位于本 crate。

// 纯数据（无 wgpu 依赖），始终可用。
pub use ze_resource::render_backend::{AccessError, GpuHandle, GpuResourceId};

#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
use crate::render_layer::RenderLayer;
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
use zela_render::light_queue::{DirectionalLightRecord, PointLightRecord};

// ─── RenderBackend trait（资源型，wgpu） ──────────────────────────────────

/// 资源型渲染后端：提供 device/queue 句柄与 GPU 资源创建入口。
///
/// 2D/3D 资源缓存（[`crate::ResourceCache`]）通过此 trait 上传资源，
/// 与具体后端（eframe / headless）解耦。
///
/// **不要求 `Send + Sync`**：wasm 上 wgpu 资源含 `Rc<RefCell<…>>`（只能在创建线程用），
/// 而本项目单线程、主线程唯一 ⇒ GPU 资源本就不跨线程，跨线程需求由调用方局部声明。
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub trait RenderBackend {
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
///
/// **不要求 `Send + Sync`**（同 [`RenderBackend`]）。
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub trait RenderBackend3D {
    /// 将一个 RenderLayer 提交到 GPU：
    /// begin render pass（目标 = layer.target，clear 黑色）→
    /// 绘制 `layer.draw_queue` 全部内容 → end pass。
    ///
    /// `lights` / `point_lights` 为本帧场景光源快照（空 = 默认光照；B1-A/B 起
    /// 显式传入，保证 `--script == --bundle` 下灯光可对拍）。
    ///
    /// 调用方负责提交 encoder（`queue.submit`）。
    fn submit_layer(
        &self,
        layer: &RenderLayer,
        encoder: &mut wgpu::CommandEncoder,
        lights: &[DirectionalLightRecord],
        point_lights: &[PointLightRecord],
    );
}

// ─── B3-0.5d：后端自述的 capability ────────────────────────────────────────
//
// 契约签名集在 `ze_render_ir::caps`（零 wgpu 类型）；这里只声明**本后端实现了哪些**
// namespace，并复用 IR 的 `negotiate`（全有或全无，缺一项即显式失败）。
//
// wgpu 当前实现 render2d / render3d / mixed；`zela.render.shapes`（直接吃含控制点的
// `Path2D`，需要解析抗锯齿 / 矢量化渲染）**尚未实现** —— 需要它的调用方会拿到
// 显式协商失败，而不是"能调用但画不出来"。

/// 本后端实现的 capability namespace（顺序 = 报告顺序）。
pub const IMPLEMENTED_NAMESPACES: &[&str] = &[
    ze_render_ir::caps::render2d::NAMESPACE,
    ze_render_ir::caps::render3d::NAMESPACE,
    ze_render_ir::caps::mixed::NAMESPACE,
];

/// 本后端实现的 capability 集合（对应契约函数 `capabilities()`）。
///
/// B10 Step 1：另自述接受的网格形态 —— wgpu 的 3D 上传是"三角形面 + 索引"
/// （`standard_3d()`），只报 [`MeshForms::tri`]。
pub fn implemented_capabilities() -> ze_render_ir::caps::RenderCapabilitySet {
    ze_render_ir::caps::RenderCapabilitySet::from_names(IMPLEMENTED_NAMESPACES)
        .with_mesh(ze_render_ir::caps::MeshForms::tri())
}

/// 按所需 namespace 协商；任一缺失 ⇒ 显式 [`CapabilityNegotiationError`]。
///
/// B10 Step 1：协商结果也要带上网格形态 —— 它与 [`implemented_capabilities`] 描述的
/// 是**同一个后端**，只差"被要求了哪些 namespace"。
pub fn negotiate_capabilities(
    required: &[&str],
) -> Result<ze_render_ir::caps::RenderCapabilitySet, ze_render_ir::caps::CapabilityNegotiationError>
{
    ze_render_ir::caps::negotiate(IMPLEMENTED_NAMESPACES, required)
        .map(|set| set.with_mesh(ze_render_ir::caps::MeshForms::tri()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 声明集合覆盖 2D / 3D / mixed，且**不含** shapes（未实现 ≠ 假装实现）。
    #[test]
    fn declared_capabilities_match_wgpu_reality() {
        let caps = implemented_capabilities();
        assert!(caps.render2d && caps.render3d && caps.mixed);
        assert!(!caps.shapes);
        assert_eq!(caps.missing(), vec![ze_render_ir::caps::shapes::NAMESPACE]);
    }

    /// 请求已实现集合 ⇒ 成功。
    #[test]
    fn negotiation_succeeds_for_implemented() {
        let caps = negotiate_capabilities(IMPLEMENTED_NAMESPACES).expect("wgpu 应满足自身声明");
        assert_eq!(caps, implemented_capabilities());
    }

    /// 只实现部分 capability 的后端请求 shapes ⇒ **显式失败**并列出缺失项。
    #[test]
    fn negotiation_fails_explicitly_for_shapes() {
        let err = negotiate_capabilities(&[
            ze_render_ir::caps::render2d::NAMESPACE,
            ze_render_ir::caps::shapes::NAMESPACE,
        ])
        .expect_err("wgpu 未实现 shapes，协商必须失败");
        assert_eq!(err.missing, vec![ze_render_ir::caps::shapes::NAMESPACE]);
        assert!(err.to_string().contains("zela.render.shapes"));
    }
}
