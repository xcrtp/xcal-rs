// render_backend.rs
//
// 渲染后端抽象：RenderBackend trait + GpuHandle 类型擦除句柄。
// 为资源缓存管理器（ResourceCache）提供统一的 GPU 资源创建入口。
//
// 设计要点：
// - wgpu 的 Buffer / Texture / TextureView / RenderPipeline 在 native 后端
//   均满足 Send + Sync + 'static，可安全装箱进 GpuHandle。
// - begin_frame / end_frame 仅是生命周期钩子：egui/eframe 路径下
//   surface 的 acquire/present 由 egui_wgpu renderer 处理，
//   后端不得触碰 surface。
// - desc 参数直接透传 wgpu 描述符，与 Device::create_* 一一对应。
// - EframeWgpuBackend 为 owning 版（内部持有 RenderContext，'static），
//   可 Box<dyn RenderBackend> 存入 Resources。

use std::any::{Any, TypeId};

use crate::render_context::RenderContext;

// ─── GpuHandle ────────────────────────────────────────────────────────────

/// 类型擦除的 GPU 资源句柄。
///
/// 通过 [`downcast_ref`](Self::downcast_ref) / [`downcast_mut`](Self::downcast_mut)
/// 取回具体类型；[`into_inner`](Self::into_inner) 消费句柄取回所有权。
pub struct GpuHandle(Box<dyn Any + Send + Sync>);

impl GpuHandle {
    /// 装箱任意 GPU 资源（wgpu 资源类型均满足 Any + Send + Sync）。
    pub fn new<T: Any + Send + Sync>(value: T) -> Self {
        Self(Box::new(value))
    }

    /// 不可变借用内部资源。
    pub fn downcast_ref<T: Any + Send + Sync>(&self) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }

    /// 可变借用内部资源。
    pub fn downcast_mut<T: Any + Send + Sync>(&mut self) -> Option<&mut T> {
        self.0.downcast_mut::<T>()
    }

    /// 消费句柄，取回资源所有权。
    pub fn into_inner<T: Any + Send + Sync>(self) -> Option<T> {
        self.0.downcast::<T>().ok().map(|b| *b)
    }

    /// 内部资源的 TypeId（用于缓存 key / 调试）。
    pub fn type_id(&self) -> TypeId {
        self.0.type_id()
    }

    /// 判断内部资源是否为指定类型。
    pub fn is<T: Any>(&self) -> bool {
        self.0.is::<T>()
    }
}

// ─── RenderBackend trait ──────────────────────────────────────────────────

/// 渲染后端最小接口。
///
/// 约束：
/// - `Send + Sync`：为后续并行调度与多线程资源准备预留。
/// - [`begin_frame`](Self::begin_frame) / [`end_frame`](Self::end_frame)
///   是生命周期钩子，默认空实现；不负责 surface acquire/present。
/// - [`set_target`](Self::set_target)：离屏路径注入自己的 TextureView；
///   egui 主帧缓冲路径（egui_wgpu renderer 或 CallbackTrait）target 置 None。
pub trait RenderBackend: Send + Sync {
    fn device(&self) -> &wgpu::Device;

    fn queue(&self) -> &wgpu::Queue;

    fn target_format(&self) -> wgpu::TextureFormat;

    /// 创建 GPU buffer（返回装箱 `wgpu::Buffer`）。
    fn create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> GpuHandle;

    /// 创建 GPU texture（返回装箱 `wgpu::Texture`；
    /// 需要视图时先 downcast 再 `texture.create_view(..)`）。
    fn create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> GpuHandle;

    /// 创建渲染管线（返回装箱 `wgpu::RenderPipeline`）。
    fn create_pipeline(&self, desc: &wgpu::RenderPipelineDescriptor<'_>) -> GpuHandle;

    /// 设置当帧渲染目标视图（owning，旧 view 自动 drop）。
    fn set_target(&mut self, view: Option<wgpu::TextureView>);

    /// 当前渲染目标视图。
    fn target(&self) -> Option<&wgpu::TextureView>;

    /// 帧开始钩子（默认空实现）。
    fn begin_frame(&mut self) {
        let _ = self;
    }

    /// 帧结束钩子（默认空实现；离屏 encoder 可在此统一提交）。
    fn end_frame(&mut self) {
        let _ = self;
    }
}

// ─── EframeWgpuBackend ────────────────────────────────────────────────────

/// 基于 eframe/egui_wgpu 的 owning 渲染后端。
///
/// 内部持有 [`RenderContext`]（owning + Clone + 'static），因此整个后端
/// 满足 `'static`，可 `Box<dyn RenderBackend>` 存入 Resources。
///
/// 构造路径：
/// - App 启动：[`EframeWgpuBackend::from_render_state`]；
/// - 测试/离屏：[`EframeWgpuBackend::headless`]。
pub struct EframeWgpuBackend {
    ctx: RenderContext,
    target: Option<wgpu::TextureView>,
    frame_active: bool,
}

impl EframeWgpuBackend {
    /// 从 egui_wgpu::RenderState 构造（App 启动路径）。
    pub fn from_render_state(rs: &egui_wgpu::RenderState) -> Self {
        Self {
            ctx: RenderContext::from_render_state(rs),
            target: None,
            frame_active: false,
        }
    }

    /// headless 构造（测试/离屏路径；无 GPU 环境返回错误）。
    pub fn headless() -> crate::Result<Self> {
        Ok(Self {
            ctx: RenderContext::headless()?,
            target: None,
            frame_active: false,
        })
    }

    /// 从已有 RenderContext 构造。
    pub fn from_context(ctx: RenderContext) -> Self {
        Self {
            ctx,
            target: None,
            frame_active: false,
        }
    }

    /// 只读访问内部 RenderContext。
    pub fn render_context(&self) -> &RenderContext {
        &self.ctx
    }

    /// 是否处于 begin_frame / end_frame 之间。
    pub fn is_frame_active(&self) -> bool {
        self.frame_active
    }
}

impl RenderBackend for EframeWgpuBackend {
    fn device(&self) -> &wgpu::Device {
        self.ctx.device()
    }

    fn queue(&self) -> &wgpu::Queue {
        self.ctx.queue()
    }

    fn target_format(&self) -> wgpu::TextureFormat {
        self.ctx.target_format()
    }

    fn create_buffer(&self, desc: &wgpu::BufferDescriptor<'_>) -> GpuHandle {
        GpuHandle::new(self.ctx.device().create_buffer(desc))
    }

    fn create_texture(&self, desc: &wgpu::TextureDescriptor<'_>) -> GpuHandle {
        GpuHandle::new(self.ctx.device().create_texture(desc))
    }

    fn create_pipeline(&self, desc: &wgpu::RenderPipelineDescriptor<'_>) -> GpuHandle {
        GpuHandle::new(self.ctx.device().create_render_pipeline(desc))
    }

    fn set_target(&mut self, view: Option<wgpu::TextureView>) {
        self.target = view;
    }

    fn target(&self) -> Option<&wgpu::TextureView> {
        self.target.as_ref()
    }

    fn begin_frame(&mut self) {
        self.frame_active = true;
    }

    fn end_frame(&mut self) {
        self.frame_active = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn test_backend_types_are_send_sync() {
        assert_send_sync::<GpuHandle>();
        assert_send_sync::<EframeWgpuBackend>();
    }

    #[test]
    fn test_gpu_handle_roundtrip() {
        let handle = GpuHandle::new(42_u32);
        assert!(handle.is::<u32>());
        assert_eq!(handle.downcast_ref::<u32>(), Some(&42));
        assert!(handle.downcast_ref::<i64>().is_none());

        let mut handle = handle;
        if let Some(v) = handle.downcast_mut::<u32>() {
            *v = 100;
        }
        assert_eq!(handle.downcast_ref::<u32>(), Some(&100));
        assert_eq!(handle.into_inner::<u32>(), Some(100));
    }

    #[test]
    fn test_gpu_handle_into_inner_wrong_type() {
        let handle = GpuHandle::new("hello".to_string());
        assert_eq!(handle.into_inner::<u32>(), None);
    }

    /// headless 后端完整生命周期：buffer / texture / view / target / 钩子。
    /// 无 GPU 环境静默跳过。
    #[test]
    fn test_headless_backend_lifecycle() {
        let Ok(mut backend) = EframeWgpuBackend::headless() else { return };

        // 初始无 target
        assert!(backend.target().is_none());
        assert!(!backend.is_frame_active());

        // 钩子
        backend.begin_frame();
        assert!(backend.is_frame_active());

        // create_buffer
        let buf = backend.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-buffer"),
            size: 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        assert!(buf.downcast_ref::<wgpu::Buffer>().is_some());

        // create_texture → create_view → set_target
        let tex = backend.create_texture(&wgpu::TextureDescriptor {
            label: Some("test-texture"),
            size: wgpu::Extent3d { width: 4, height: 4, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: backend.target_format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let texture = tex.downcast_ref::<wgpu::Texture>().expect("texture");
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        backend.set_target(Some(view));
        assert!(backend.target().is_some());

        // end_frame
        backend.end_frame();
        assert!(!backend.is_frame_active());

        // 消费 target
        backend.set_target(None);
        assert!(backend.target().is_none());
    }

    /// create_pipeline：最小三角 shader，验证装箱 RenderPipeline。
    /// 无 GPU 环境静默跳过。
    #[test]
    fn test_headless_backend_create_pipeline() {
        let Ok(backend) = EframeWgpuBackend::headless() else { return };

        let source = r#"
            @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
                var p = array(vec2f(-1.0, -1.0), vec2f(3.0, -1.0), vec2f(-1.0, 3.0));
                return vec4f(p[i], 0.0, 1.0);
            }
            @fragment fn fs() -> @location(0) vec4f {
                return vec4f(1.0, 0.0, 0.0, 1.0);
            }
        "#;
        let shader = backend.device().create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("test-shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });

        let pipeline = backend.create_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("test-pipeline"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: backend.target_format(),
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        assert!(pipeline.downcast_ref::<wgpu::RenderPipeline>().is_some());
    }
}
