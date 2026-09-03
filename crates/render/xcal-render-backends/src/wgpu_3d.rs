// wgpu_3d.rs
//
// WGPU 3D 后端（由 xcal-render 迁移，逻辑不变）：
// - WgpuBackend3D：持有 device/queue 与两条管线（triangle list / line list），
//   共享同一套 uniform（mvp + color）绑定布局；
// - PreparedDraw3D / Frame3DData：每帧重建的 GPU 资源（MVE 阶段不做缓存）；
// - RenderBackend3D::submit_layer：离屏/surface 目标的自包含提交。
//
// egui 集成的回调（Layer3DCallback）见 layer_callback.rs。

use std::mem;
use std::num::NonZeroU64;

use glam::Mat4;
use wgpu::util::DeviceExt as _;

use xcal_core::scene::components::PrimitiveTopology;
use xc_geometry::Color32;
use xcal_render::draw_queue_3d::{LineDraw, MeshDraw};
use xcal_render::{RenderBackend3D, RenderLayer};

/// 默认着色器（cube.wgsl）：uniform = mat4 mvp + vec4 color。
pub const DEFAULT_SHADER: &str = include_str!("../shaders/cube.wgsl");

/// uniform 块大小：mat4x4（64 字节）+ vec4（16 字节）。
pub const UNIFORM_SIZE: u64 = 80;

/// 顶点步长：vec3 position = 12 字节。
const VERTEX_STRIDE: u64 = 12;

// ─── 每帧 GPU 资源 ──────────────────────────────────────────────────────────

/// 单次绘制的已上传 GPU 资源（MVE：每帧重建，后续可换 staging/缓存）。
pub struct PreparedDraw3D {
    /// 顶点缓冲（vec3 位置）。
    vertex_buffer: wgpu::Buffer,
    /// 可选索引缓冲（u32）。
    index_buffer: Option<wgpu::Buffer>,
    /// uniform 缓冲；bind group 不持有 buffer 生命周期，需在此保留。
    _uniform_buffer: wgpu::Buffer,
    vertex_count: u32,
    index_count: u32,
    bind_group: wgpu::BindGroup,
    topology: PrimitiveTopology,
}

/// 一帧内全部已准备的绘制资源。
///
/// 作为 `CallbackResources` TypeMap 的条目在 prepare/paint 间传递。
pub struct Frame3DData {
    /// 本帧全部绘制命令（按录制顺序）。
    pub draws: Vec<PreparedDraw3D>,
}

// ─── WgpuBackend3D ──────────────────────────────────────────────────────────

/// WGPU 3D 渲染后端（设备句柄 + 管线 + 绑定布局）。
///
/// 内置 triangle list / line list 两条管线，共享 `mat4 mvp + vec4 color` uniform。
/// eframe 集成中，把它放进 `egui_wgpu::Renderer::callback_resources`，
/// 由 `Layer3DCallback` 的 prepare/paint 回调取用。
///
/// # Example（离屏）
/// ```rust
/// # fn build(device: &wgpu::Device, queue: &wgpu::Queue) {
/// use xcal_render_backends::WgpuBackend3D;
///
/// let backend = WgpuBackend3D::with_default_shader(
///     device,
///     queue,
///     wgpu::TextureFormat::Bgra8UnormSrgb,
/// );
/// // 写入 uniform：mat4(64B) + vec4(16B) = 80B
/// let buf = device.create_buffer(&wgpu::BufferDescriptor {
///     label: Some("uniforms"),
///     size: xcal_render_backends::wgpu_3d::UNIFORM_SIZE,
///     usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
///     mapped_at_creation: false,
/// });
/// WgpuBackend3D::write_uniforms(
///     queue, &buf, glam::Mat4::IDENTITY,
///     xc_geometry::Color32::new(255, 255, 255, 255),
/// );
/// let _ = backend;
/// # }
/// ```
pub struct WgpuBackend3D {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline_triangles: wgpu::RenderPipeline,
    pipeline_lines: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl WgpuBackend3D {
    /// 用给定 WGSL 源码构造后端（创建三角形/线段两条管线）。
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        shader_source: &str,
    ) -> Self {
        let device = device.clone();
        let queue = queue.clone();

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xcal-3d-bind-group-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: Some(NonZeroU64::new(UNIFORM_SIZE).unwrap()),
                },
                count: None,
            }],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("xcal-3d-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("xcal-3d-shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        let vertex_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: VERTEX_STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            }],
        })];

        let make_pipeline = |topology: wgpu::PrimitiveTopology,
                             cull_mode: Option<wgpu::Face>,
                             label: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &vertex_layout,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    cull_mode,
                    ..Default::default()
                },
                depth_stencil: None, // MVE 阶段无深度缓冲
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let pipeline_triangles = make_pipeline(
            wgpu::PrimitiveTopology::TriangleList,
            Some(wgpu::Face::Back),
            "xcal-3d-pipeline-triangles",
        );
        let pipeline_lines = make_pipeline(
            wgpu::PrimitiveTopology::LineList,
            None,
            "xcal-3d-pipeline-lines",
        );

        Self {
            device,
            queue,
            pipeline_triangles,
            pipeline_lines,
            bind_group_layout,
        }
    }

    /// 使用内置 [`DEFAULT_SHADER`] 构造。
    pub fn with_default_shader(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
    ) -> Self {
        Self::new(device, queue, format, DEFAULT_SHADER)
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// 写入 uniform 块（mvp 64 字节 + color 16 字节 = 80 字节）。
    pub fn write_uniforms(queue: &wgpu::Queue, buffer: &wgpu::Buffer, mvp: Mat4, color: Color32) {
        queue.write_buffer(buffer, 0, &uniform_bytes(mvp, color));
    }

    /// 为一次绘制创建 uniform buffer + bind group。
    fn create_uniform_binding(&self, mvp: Mat4, color: Color32) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xcal-3d-uniform"),
                contents: &uniform_bytes(mvp, color),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xcal-3d-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        });
        (buffer, bind_group)
    }

    /// 上传一帧的全部绘制资源（MVE：每帧重建 vertex/uniform buffer）。
    ///
    /// `queue` 预留给后续增量上传（当前统一走 `create_buffer_init`）。
    pub fn prepare_draws(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        meshes: &[MeshDraw],
        lines: &[LineDraw],
    ) -> Frame3DData {
        let mut draws = Vec::with_capacity(meshes.len() + lines.len());

        for md in meshes {
            let mvp = view_proj * md.transform;

            let positions: Vec<[f32; 3]> = md.mesh.vertices.iter().map(|v| v.to_array()).collect();
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xcal-3d-vertices"),
                contents: bytes_of(&positions),
                usage: wgpu::BufferUsages::VERTEX,
            });

            let index_buffer = md.mesh.indices.as_ref().map(|indices| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("xcal-3d-indices"),
                    contents: bytes_of(indices),
                    usage: wgpu::BufferUsages::INDEX,
                })
            });

            let (uniform_buffer, bind_group) = self.create_uniform_binding(mvp, md.color);
            draws.push(PreparedDraw3D {
                vertex_buffer,
                index_buffer,
                _uniform_buffer: uniform_buffer,
                vertex_count: md.mesh.vertices.len() as u32,
                index_count: md
                    .mesh
                    .indices
                    .as_ref()
                    .map(|v| v.len() as u32)
                    .unwrap_or(0),
                bind_group,
                topology: md.mesh.topology,
            });
        }

        for line in lines {
            let positions: [[f32; 3]; 2] = [line.start.to_array(), line.end.to_array()];
            let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("xcal-3d-line-vertices"),
                contents: bytes_of(&positions),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let (uniform_buffer, bind_group) = self.create_uniform_binding(view_proj, line.color);
            draws.push(PreparedDraw3D {
                vertex_buffer,
                index_buffer: None,
                _uniform_buffer: uniform_buffer,
                vertex_count: 2,
                index_count: 0,
                bind_group,
                topology: PrimitiveTopology::Lines,
            });
        }

        let _ = queue; // 见函数文档；MVE 全部走 mapped-at-creation 上传
        Frame3DData { draws }
    }

    /// 把已准备的绘制命令录制进外部 render pass（egui 回调路径）。
    pub fn record_prepared<'a>(&self, draws: &[PreparedDraw3D], rpass: &mut wgpu::RenderPass<'a>) {
        for d in draws {
            match d.topology {
                PrimitiveTopology::Triangles => rpass.set_pipeline(&self.pipeline_triangles),
                PrimitiveTopology::Lines => rpass.set_pipeline(&self.pipeline_lines),
                PrimitiveTopology::Points => continue, // MVE 不支持点拓扑
            }
            rpass.set_bind_group(0, &d.bind_group, &[]);
            rpass.set_vertex_buffer(0, d.vertex_buffer.slice(..));
            if let Some(index_buffer) = &d.index_buffer {
                rpass.set_index_buffer(index_buffer.slice(..), wgpu::IndexFormat::Uint32);
                rpass.draw_indexed(0..d.index_count, 0, 0..1);
            } else {
                rpass.draw(0..d.vertex_count, 0..1);
            }
        }
    }
}

// ─── RenderBackend3D 实现 ───────────────────────────────────────────────────

impl RenderBackend3D for WgpuBackend3D {
    fn submit_layer(&self, layer: &RenderLayer, encoder: &mut wgpu::CommandEncoder) {
        let view_proj = layer.camera.view_projection(layer.viewport.aspect());
        let frame = self.prepare_draws(
            &self.device,
            &self.queue,
            view_proj,
            layer.draw_queue.meshes(),
            layer.draw_queue.lines(),
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("xcal-3d-layer-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &layer.target.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.0,
                        g: 0.0,
                        b: 0.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        self.record_prepared(&frame.draws, &mut pass);
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

/// 把 Copy 切片视为字节切片（顶点/索引上传用）。
fn bytes_of<T: Copy>(data: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, mem::size_of_val(data)) }
}

/// 组装 uniform 字节：mat4 列主序 64 字节 + vec4 16 字节。
fn uniform_bytes(mvp: Mat4, color: Color32) -> [u8; UNIFORM_SIZE as usize] {
    let mut data = [0u8; UNIFORM_SIZE as usize];
    let m = mvp.to_cols_array();
    data[..64].copy_from_slice(bytes_of(&m));
    let c = [
        color.r() as f32 / 255.0,
        color.g() as f32 / 255.0,
        color.b() as f32 / 255.0,
        color.a() as f32 / 255.0,
    ];
    data[64..].copy_from_slice(bytes_of(&c));
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_context::RenderContext;
    use xcal_core::scene::components::{Camera3D, Projection};
    use xcal_core::scene::primitives::wireframe_cube;
    use xc_geometry::Vec3;
    use xcal_render::{DrawQueue3D, Viewport};

    fn headless_backend() -> Option<(WgpuBackend3D, RenderContext)> {
        let ctx = RenderContext::headless().ok()?;
        let backend =
            WgpuBackend3D::with_default_shader(ctx.device(), ctx.queue(), ctx.target_format());
        Some((backend, ctx))
    }

    #[test]
    fn test_backend_creation_does_not_panic() {
        let Some((backend, _ctx)) = headless_backend() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        // 管线与绑定布局已创建；触发一次 uniform 写入验证路径。
        let buffer = backend.device().create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-uniform"),
            size: UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        WgpuBackend3D::write_uniforms(
            backend.queue(),
            &buffer,
            Mat4::IDENTITY,
            Color32::new(255, 255, 255, 255),
        );
    }

    #[test]
    fn test_submit_layer_records_without_panic() {
        let Some((backend, ctx)) = headless_backend() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };

        let camera = Camera3D::new(
            glam::Vec3::new(0.0, 0.0, 5.0),
            glam::Vec3::ZERO,
            glam::Vec3::Y,
            Projection::perspective(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 100.0),
        );
        let viewport = Viewport::full(256, 256);
        let mut layer =
            RenderLayer::new_offscreen(ctx.device(), camera, viewport, ctx.target_format());
        layer.draw_queue.draw_mesh(
            wireframe_cube(1.0, Color32::new(255, 255, 255, 255)),
            Mat4::IDENTITY,
            Color32::new(255, 255, 255, 255),
        );

        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test-encoder"),
            });
        backend.submit_layer(&layer, &mut encoder);
        ctx.submit(encoder.finish());
    }

    #[test]
    fn test_prepare_draws_counts() {
        let Some((backend, ctx)) = headless_backend() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let mut queue = DrawQueue3D::new();
        queue.draw_mesh(
            wireframe_cube(1.0, Color32::new(255, 255, 255, 255)),
            Mat4::IDENTITY,
            Color32::new(255, 255, 255, 255),
        );
        queue.draw_line(Vec3::ZERO, Vec3::X, Color32::new(255, 0, 0, 255));
        let frame = backend.prepare_draws(
            ctx.device(),
            ctx.queue(),
            Mat4::IDENTITY,
            queue.meshes(),
            queue.lines(),
        );
        assert_eq!(frame.draws.len(), 2);
        assert_eq!(frame.draws[0].topology, PrimitiveTopology::Lines);
    }
}
