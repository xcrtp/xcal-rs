// wgpu_3d.rs
//
// WGPU 3D 后端（G2/G5/G6 资源生命周期改造）：
// - WgpuBackend3D：持有 device/queue 与两条管线（triangle list / line list），
//   共享一套 dynamic-offset uniform 绑定布局；
// - G2：hash_mesh — 基于 Mesh3D 公有字段（顶点/索引/拓扑）的内容哈希，颜色排除；
// - G5：UniformArena — 256 字节对齐的 dynamic uniform staging，flush 一次性 write_buffer；
// - G6：prepare_draws — mesh/line 顶点缓冲走 `mesh_cache`（按内容去重），
//   uniform 走 arena，缓存命中时不再 per-mesh per-frame 调用 `create_buffer_init`；
// - PreparedDraw3D / Frame3DData：只持有句柄克隆（wgpu::Buffer 是 ref-counted），
//   arena 对应的 bind_group 所有 draw 共享（clone 浅拷贝）。
//
// egui 集成的回调（Layer3DCallback）见 layer_callback.rs。
//
// # G7 回校（R6）：为什么这里**不**接 `StaticConsistencyChecker`
//
// 计划倾向于"只接 3D 侧"，但本仓库的 3D 路径缺少接线所需的两个前提：
//   1. **没有顶点布局契约声明** —— 3D pipeline 的顶点布局是硬编码 bespoke
//      （`Float32x3` / stride 12），`VertexLayout` 从未参与，checker 的
//      `(layout, identity)` 对没有输入可比；
//   2. **没有稳定 identity** —— `MeshDraw` 按值携带 `Mesh3D`，`DrawQueue3D`
//      不携带 entity id；唯一现成的 identity 就是内容哈希本身，而用它当 key
//      两帧必然一致 → checker **恒不报警**，属于"假接线"。
// 2D 侧则相反：内容（形变 / 相机 / group）每帧都变，接上必然大量误报。
// 因此 R6 决策 = **两侧都不接**，把事实记在此处；将来 3D 引入 VertexLayout 契约
// 声明 + 稳定 entity identity（或 `MeshDraw` 带 id）后，可在 `prepare_draws` /
// `record_prepared` 入口补齐。

use std::hash::{DefaultHasher, Hash, Hasher};
use std::mem;
use std::num::NonZeroU64;
use std::sync::Mutex;

use glam::Mat4;
use wgpu::util::DeviceExt as _;

use xcal_core::scene::components::{Mesh3D, PrimitiveTopology};
use xc_geometry::Color32;
use xcal_render::draw_queue_3d::{LineDraw, MeshDraw};
use xcal_render::{GpuHandle, RenderBackend3D, RenderLayer, ResourceCache, ResourceKey};

/// 默认着色器（cube.wgsl）：uniform = mat4 mvp + vec4 color。
pub const DEFAULT_SHADER: &str = include_str!("../shaders/cube.wgsl");

/// uniform 块大小：mat4x4（64 字节）+ vec4（16 字节）。
pub const UNIFORM_SIZE: u64 = 80;

/// wgpu dynamic uniform offset 要求的最小对齐。
pub const DYNAMIC_OFFSET_ALIGNMENT: u64 = 256;

/// 顶点步长：vec3 position = 12 字节。
const VERTEX_STRIDE: u64 = 12;

// ─── G2：hash_mesh ─────────────────────────────────────────────────────────

/// 基于 [`Mesh3D`] 公有字段的内容哈希。
///
/// - 顶点：按 `to_bits()` 散列（f32 本身不实现 Hash）；
/// - 索引：`Some` 分支写入长度+每个索引；`None` 单独写入 discriminant；
/// - 拓扑：按 `as u8` 写入；
/// - **颜色 intentionally excluded**（颜色走 per-draw uniform，不影响 mesh 复用）。
pub fn hash_mesh(mesh: &Mesh3D) -> u64 {
    let mut h = DefaultHasher::new();
    (mesh.vertices.len() as u64).hash(&mut h);
    for v in &mesh.vertices {
        v.x.to_bits().hash(&mut h);
        v.y.to_bits().hash(&mut h);
        v.z.to_bits().hash(&mut h);
    }
    match &mesh.indices {
        Some(idx) => {
            1u8.hash(&mut h);
            (idx.len() as u64).hash(&mut h);
            for &i in idx {
                i.hash(&mut h);
            }
        }
        None => 0u8.hash(&mut h),
    }
    (mesh.topology as u8).hash(&mut h);
    h.finish()
}

// ─── G5：UniformArena ──────────────────────────────────────────────────────

/// Dynamic uniform arena。stride 强制 256 字节对齐（wgpu dynamic uniform offset 的规范）。
///
/// 典型使用：
/// 1. `begin_frame()` 重置 used_blocks（staging 底层容量不释放，避免抖动）；
/// 2. `push(data)` 追加一帧内每个 draw 的 uniform 字节，返回块 index（0..used_blocks）；
/// 3. `flush(device, queue, bgl)`：必要时扩容 buffer，一次性 `write_buffer` 上传 staging
///    的全部数据，并按需重建 bind group。返回 (&buffer, &bind_group, stride_bytes)。
pub struct UniformArena {
    buffer: Option<wgpu::Buffer>,
    bind_group: Option<wgpu::BindGroup>,
    capacity_blocks: u32,
    used_blocks: u32,
    stride: u64,
    uniform_size: u64,
    staging: Vec<u8>,
}

impl UniformArena {
    pub const MIN_BLOCKS: u32 = 16;

    pub fn new(uniform_size: u64, stride: u64) -> Self {
        assert!(
            stride >= uniform_size,
            "stride ({stride}) must be >= uniform_size ({uniform_size})"
        );
        assert_eq!(
            stride & 0xFF,
            0,
            "stride must be 256-byte aligned (wgpu dynamic uniform offset), got {stride}"
        );
        Self {
            buffer: None,
            bind_group: None,
            capacity_blocks: 0,
            used_blocks: 0,
            stride,
            uniform_size,
            staging: Vec::new(),
        }
    }

    /// 新帧开始：重置 used_blocks（staging 容量保留，避免每帧 realloc）。
    pub fn begin_frame(&mut self) {
        self.used_blocks = 0;
    }

    /// 追加 uniform 数据到 staging。返回本块的 block index（0..used_blocks），
    /// 调用方随后通过 `index * stride` 得到 dynamic uniform 的 byte offset。
    pub fn push(&mut self, data: &[u8]) -> u32 {
        assert_eq!(
            data.len() as u64,
            self.uniform_size,
            "push() data size mismatch"
        );
        let slot = self.used_blocks;
        self.used_blocks = slot.checked_add(1).expect("too many draws in frame");
        let needed = (self.used_blocks as u64 * self.stride) as usize;
        if self.staging.len() < needed {
            // 按 256 B 填充，让 staging 的已用区间完全填满，避免 write_buffer
            // 读到未初始化数据（wgpu 对 &[u8] 没有特殊要求，但保持干净）。
            self.staging.resize(needed, 0u8);
        }
        let off = (slot as u64 * self.stride) as usize;
        self.staging[off..off + data.len()].copy_from_slice(data);
        slot
    }

    /// 保证 arena 的 GPU 缓冲容量 ≥ needed_blocks（MIN_BLOCKS 下限），
    /// 写回 staging 数据，并按需重建 bind_group（buffer 身份变化时）。
    pub fn flush<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bgl: &wgpu::BindGroupLayout,
    ) -> (&'a wgpu::Buffer, &'a wgpu::BindGroup, u32) {
        let needed_blocks = self.used_blocks.max(Self::MIN_BLOCKS);

        if needed_blocks > self.capacity_blocks {
            let new_blocks = needed_blocks.next_power_of_two();
            let new_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("xcal-3d-uniform-arena"),
                size: new_blocks as u64 * self.stride,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // 缓冲身份切换 -> bind_group 必须重建。
            self.bind_group = None;
            self.buffer = Some(new_buf);
            self.capacity_blocks = new_blocks;
        }

        let buffer = self.buffer.as_ref().expect("buffer must be created");
        let used_bytes = (self.used_blocks as u64 * self.stride) as usize;
        if used_bytes > 0 {
            queue.write_buffer(buffer, 0, &self.staging[..used_bytes]);
        }

        if self.bind_group.is_none() {
            let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("xcal-3d-arena-bind-group"),
                layout: bgl,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(self.uniform_size),
                    }),
                }],
            });
            self.bind_group = Some(bg);
        }

        let bg = self.bind_group.as_ref().expect("bind_group must be created");
        (buffer, bg, self.stride as u32)
    }

    #[inline]
    pub fn capacity_blocks(&self) -> u32 {
        self.capacity_blocks
    }
    #[inline]
    pub fn used_blocks(&self) -> u32 {
        self.used_blocks
    }
    #[inline]
    pub fn stride(&self) -> u64 {
        self.stride
    }
}

// ─── Cached mesh buffers + type tags ───────────────────────────────────────

/// 缓存内持有的 mesh 顶点/索引缓冲元数据。
struct CachedMeshBuffers {
    vertex_buffer: wgpu::Buffer,
    index_buffer: Option<wgpu::Buffer>,
    vertex_count: u32,
    index_count: u32,
    topology: PrimitiveTopology,
}

// Safety: wgpu::Buffer / Option<wgpu::Buffer> 都是 Send + Sync。
unsafe impl Send for CachedMeshBuffers {}
unsafe impl Sync for CachedMeshBuffers {}

/// ResourceKey 类型标签：用于 mesh（顶点+索引）缓冲内容缓存。
#[derive(Debug)]
struct MeshVertexTag;
/// ResourceKey 类型标签：用于 line（2 顶点）缓冲内容缓存。
#[derive(Debug)]
struct LineVertexTag;

// ─── 每帧 GPU 资源 ──────────────────────────────────────────────────────────

/// 单次绘制的已准备 GPU 句柄。
///
/// - `vertex_buffer` / `index_buffer`：`mesh_cache` 中的 CachedMeshBuffers 的 clone
///   （wgpu::Buffer 是内部 ref-counted 句柄，clone 是浅拷贝且非常廉价）。
/// - `bind_group`：本帧 `arena.flush()` 返回的 bind group 的 clone（所有 draw 共享同一个）；
/// - `uniform_offset`：dynamic uniform offset（字节，256 对齐）。
pub struct PreparedDraw3D {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: Option<wgpu::Buffer>,
    pub vertex_count: u32,
    pub index_count: u32,
    pub topology: PrimitiveTopology,
    pub uniform_offset: u32,
    pub bind_group: wgpu::BindGroup,
}

/// 一帧内全部已准备的绘制资源。
///
/// 作为 `CallbackResources` TypeMap 的条目在 prepare/paint 间传递。
/// 额外持有 `_arena_buffer` 以保证所有 draw 的 bind_group 引用的 uniform buffer 在本帧内保持 alive。
pub struct Frame3DData {
    pub draws: Vec<PreparedDraw3D>,
    pub _arena_buffer: wgpu::Buffer,
}

// ─── WgpuBackend3D ──────────────────────────────────────────────────────────

/// WGPU 3D 渲染后端（设备句柄 + 管线 + 绑定布局 + mesh 缓存 + uniform arena）。
///
/// 内部状态（cache / arena）用 `Mutex` 保护：`RenderBackend3D` 要求 `Send + Sync`，
/// 而 prepare_draws / record_prepared 是 `&self`，但需要修改缓存和 arena。
pub struct WgpuBackend3D {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline_triangles: wgpu::RenderPipeline,
    pipeline_lines: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    /// mesh/line 顶点（+可选索引）缓冲缓存。key = 内容哈希 + type_tag；
    /// meta = ()；实际 GPU 句柄以 `GpuHandle<CachedMeshBuffers>` 形式存于 ResourceCache。
    mesh_cache: Mutex<ResourceCache<()>>,
    /// 256 对齐 dynamic uniform arena。
    arena: Mutex<UniformArena>,
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

        // G6: binding 0 = uniform with dynamic offset。
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xcal-3d-bind-group-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
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
                depth_stencil: None,
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
            mesh_cache: Mutex::new(ResourceCache::new()),
            arena: Mutex::new(UniformArena::new(UNIFORM_SIZE, DYNAMIC_OFFSET_ALIGNMENT)),
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

    /// 推进 mesh_cache 的「延迟销毁」提交帧计数器（destroyer.on_submit）。
    ///
    /// 每次把 command encoder 提交到 GPU queue 之后调用一次（每帧一次或多次）。
    pub fn notify_submit(&self) {
        self.mesh_cache.lock().expect("mesh_cache poisoned").notify_submit();
    }

    // ── prepare_draws：G6 核心 ────────────────────────────────────────

    /// 上传一帧的全部绘制资源。
    ///
    /// - mesh/line 顶点缓冲：按内容哈希，命中则复用，miss 才 create_buffer_init 一次；
    /// - uniform：**全部**走 `UniformArena.push`，flush 时一次性 write_buffer；
    /// - bind_group：每帧最多创建一次（arena 未扩容时复用 bind_group）。
    pub fn prepare_draws(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        meshes: &[MeshDraw],
        lines: &[LineDraw],
    ) -> Frame3DData {
        // Mutex 独立，互不冲突。按 Mutex 风格获取。
        let mut arena_guard = self.arena.lock().expect("arena poisoned");
        arena_guard.begin_frame();
        let mut cache_guard = self.mesh_cache.lock().expect("mesh_cache poisoned");
        cache_guard.begin_frame();

        let total_draws = meshes.len().saturating_add(lines.len());
        // skeleton：暂存 draw 信息，bind_group 在 flush 之后再统一 clone。
        struct Skeleton {
            vertex_buffer: wgpu::Buffer,
            index_buffer: Option<wgpu::Buffer>,
            vertex_count: u32,
            index_count: u32,
            topology: PrimitiveTopology,
            uniform_offset: u32,
        }
        let mut skeletons: Vec<Skeleton> = Vec::with_capacity(total_draws);

        // 辅助：从 CachedMeshBuffers 句柄取出绘制元 + VB/IB（wgpu::Buffer 自身是 Clone）。
        fn extract_buffers(handle: &GpuHandle) -> CachedMeshBuffersCopy {
            let cached = handle
                .downcast_ref::<CachedMeshBuffers>()
                .expect("cached GpuHandle must wrap CachedMeshBuffers");
            CachedMeshBuffersCopy {
                vertex_buffer: cached.vertex_buffer.clone(),
                index_buffer: cached.index_buffer.clone(),
                vertex_count: cached.vertex_count,
                index_count: cached.index_count,
                topology: cached.topology,
            }
        }
        struct CachedMeshBuffersCopy {
            vertex_buffer: wgpu::Buffer,
            index_buffer: Option<wgpu::Buffer>,
            vertex_count: u32,
            index_count: u32,
            topology: PrimitiveTopology,
        }

        // ── MeshDraws ───────────────────────────────────────────────────
        for md in meshes {
            let hash = hash_mesh(&md.mesh);
            let key = ResourceKey::content::<MeshVertexTag>(hash);

            // 先查命中；命中直接取缓存里的字段副本；未命中创建 VB/IB 并 insert_handle。
            let cached_copy = if let Some(handle_ref) = cache_guard.get(key) {
                extract_buffers(handle_ref)
            } else {
                let positions: Vec<[f32; 3]> =
                    md.mesh.vertices.iter().map(|v| v.to_array()).collect();
                let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("xcal-3d-mesh-vertices"),
                    contents: bytes_of(&positions),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let ib = md.mesh.indices.as_ref().map(|indices| {
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("xcal-3d-mesh-indices"),
                        contents: bytes_of(indices),
                        usage: wgpu::BufferUsages::INDEX,
                    })
                });
                let cached = CachedMeshBuffers {
                    vertex_buffer: vb,
                    index_buffer: ib,
                    vertex_count: md.mesh.vertices.len() as u32,
                    index_count: md
                        .mesh
                        .indices
                        .as_ref()
                        .map(|v| v.len() as u32)
                        .unwrap_or(0),
                    topology: md.mesh.topology,
                };
                let copy = CachedMeshBuffersCopy {
                    vertex_buffer: cached.vertex_buffer.clone(),
                    index_buffer: cached.index_buffer.clone(),
                    vertex_count: cached.vertex_count,
                    index_count: cached.index_count,
                    topology: cached.topology,
                };
                let owned = GpuHandle::new(cached);
                let _ = cache_guard.insert_handle(key, (), owned);
                copy
            };

            let mvp = view_proj * md.transform;
            let bytes = uniform_bytes(mvp, md.color);
            let block_index = arena_guard.push(&bytes);
            let offset_bytes = block_index as u32 * arena_guard.stride() as u32;

            skeletons.push(Skeleton {
                vertex_buffer: cached_copy.vertex_buffer,
                index_buffer: cached_copy.index_buffer,
                vertex_count: cached_copy.vertex_count,
                index_count: cached_copy.index_count,
                topology: cached_copy.topology,
                uniform_offset: offset_bytes,
            });
        }

        // ── LineDraws：2 顶点小 mesh，同样按内容（start, end）去重 ─────
        for line in lines {
            let mut h = DefaultHasher::new();
            line.start.x.to_bits().hash(&mut h);
            line.start.y.to_bits().hash(&mut h);
            line.start.z.to_bits().hash(&mut h);
            line.end.x.to_bits().hash(&mut h);
            line.end.y.to_bits().hash(&mut h);
            line.end.z.to_bits().hash(&mut h);
            (PrimitiveTopology::Lines as u8).hash(&mut h);
            let pseudo_hash = h.finish();

            let line_key = ResourceKey::content::<LineVertexTag>(pseudo_hash);

            let cached_copy = if let Some(handle_ref) = cache_guard.get(line_key) {
                extract_buffers(handle_ref)
            } else {
                let positions: [[f32; 3]; 2] =
                    [line.start.to_array(), line.end.to_array()];
                let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("xcal-3d-line-vertices"),
                    contents: bytes_of(&positions),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let cached = CachedMeshBuffers {
                    vertex_buffer: vb,
                    index_buffer: None,
                    vertex_count: 2,
                    index_count: 0,
                    topology: PrimitiveTopology::Lines,
                };
                let copy = CachedMeshBuffersCopy {
                    vertex_buffer: cached.vertex_buffer.clone(),
                    index_buffer: None,
                    vertex_count: 2,
                    index_count: 0,
                    topology: PrimitiveTopology::Lines,
                };
                let owned = GpuHandle::new(cached);
                let _ = cache_guard.insert_handle(line_key, (), owned);
                copy
            };

            let bytes = uniform_bytes(view_proj, line.color);
            let block_index = arena_guard.push(&bytes);
            let offset_bytes = block_index as u32 * arena_guard.stride() as u32;

            skeletons.push(Skeleton {
                vertex_buffer: cached_copy.vertex_buffer,
                index_buffer: cached_copy.index_buffer,
                vertex_count: cached_copy.vertex_count,
                index_count: cached_copy.index_count,
                topology: cached_copy.topology,
                uniform_offset: offset_bytes,
            });
        }

        // 明确释放 cache guard（MutexGuard），之后再进行 arena.flush。
        drop(cache_guard);

        // ── Flush arena：一次性写 GPU，得到本帧共享的 bind group ───
        let (arena_buf, bind_group, _stride_u32) =
            arena_guard.flush(device, queue, &self.bind_group_layout);

        // 把返回的借用转成 owned clone（wgpu handle clone 是 ref-count 浅拷贝）。
        let arena_buf_owned: wgpu::Buffer = arena_buf.clone();
        let bind_group_owned: wgpu::BindGroup = bind_group.clone();

        // 释放 arena guard，使其不再被借用（之后可以 lock arena）。
        drop(arena_guard);

        // ── 统一填充 bind_group（clone = 浅拷贝 wgpu handle） ───────
        let draws_final: Vec<PreparedDraw3D> = skeletons
            .into_iter()
            .map(|s| PreparedDraw3D {
                vertex_buffer: s.vertex_buffer,
                index_buffer: s.index_buffer,
                vertex_count: s.vertex_count,
                index_count: s.index_count,
                topology: s.topology,
                uniform_offset: s.uniform_offset,
                bind_group: bind_group_owned.clone(),
            })
            .collect();

        Frame3DData {
            draws: draws_final,
            _arena_buffer: arena_buf_owned,
        }
    }

    /// 把已准备的绘制命令录制进外部 render pass（egui 回调路径）。
    pub fn record_prepared<'a>(&self, draws: &[PreparedDraw3D], rpass: &mut wgpu::RenderPass<'a>) {
        for d in draws {
            match d.topology {
                PrimitiveTopology::Triangles => rpass.set_pipeline(&self.pipeline_triangles),
                PrimitiveTopology::Lines => rpass.set_pipeline(&self.pipeline_lines),
                PrimitiveTopology::Points => continue,
            }
            // G6: dynamic uniform offset 传入 set_bind_group 的 offsets。
            rpass.set_bind_group(0, &d.bind_group, &[d.uniform_offset]);
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

// ─── tests ──────────────────────────────────────────────────────────────────

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

    // ── G2：hash_mesh 单测（纯 CPU，headless，无需 GPU） ──────────────

    fn basic_mesh() -> Mesh3D {
        Mesh3D::new(
            vec![
                glam::Vec3::new(-1.0, -1.0, 0.0),
                glam::Vec3::new(1.0, -1.0, 0.0),
                glam::Vec3::new(0.0, 1.0, 0.0),
            ],
            Some(vec![0, 1, 2]),
            PrimitiveTopology::Triangles,
            Color32::new(255, 255, 255, 255),
        )
    }

    #[test]
    fn test_hash_mesh_equal_data_equal_hash() {
        // 场景 1：相同顶点/索引/拓扑，不同颜色 → 相同哈希
        let a = basic_mesh();
        let mut b = basic_mesh();
        b.color = Color32::new(255, 0, 0, 255);
        assert_eq!(hash_mesh(&a), hash_mesh(&b), "color must be excluded");

        // 场景 2：改动一个顶点 → 不同哈希
        let mut c = basic_mesh();
        c.vertices[0].x = 9.9;
        assert_ne!(hash_mesh(&a), hash_mesh(&c), "vertex change must differ");

        // 场景 3：追加一个索引 → 不同哈希
        let mut d = basic_mesh();
        if let Some(ref mut idx) = d.indices {
            idx.push(0);
        }
        assert_ne!(hash_mesh(&a), hash_mesh(&d), "index change must differ");

        // 场景 4：换拓扑 → 不同哈希
        let mut e = basic_mesh();
        e.topology = PrimitiveTopology::Lines;
        assert_ne!(hash_mesh(&a), hash_mesh(&e), "topology change must differ");

        // 另外：indices = None vs Some 必须不同（覆盖 None 分支）
        let mut f = basic_mesh();
        f.indices = None;
        assert_ne!(hash_mesh(&a), hash_mesh(&f), "indices Some/None must differ");
    }

    // ── G5：UniformArena 单测（纯 CPU + 需要真 GPU 的 headless 测试） ─

    #[test]
    fn test_uniform_arena_stride_is_256_aligned() {
        let arena = UniformArena::new(UNIFORM_SIZE, 256);
        assert_eq!(arena.stride() % 256, 0);
        assert_eq!(arena.stride(), DYNAMIC_OFFSET_ALIGNMENT);

        // 非 256 对齐必须 panic（通过 catch_unwind 在运行时断言）。
        let result = std::panic::catch_unwind(|| UniformArena::new(UNIFORM_SIZE, 128));
        assert!(result.is_err(), "stride=128 must panic");

        // uniform_size > stride 必须 panic。
        let result = std::panic::catch_unwind(|| UniformArena::new(UNIFORM_SIZE + 256, 256));
        assert!(result.is_err(), "uniform_size > stride must panic");
    }

    #[test]
    fn test_uniform_arena_push_and_flush_headless() {
        let Some((backend, ctx)) = headless_backend() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let mut arena = UniformArena::new(UNIFORM_SIZE, 256);
        assert_eq!(arena.used_blocks(), 0);
        assert_eq!(arena.capacity_blocks(), 0);

        arena.begin_frame();
        let dummy = [0u8; UNIFORM_SIZE as usize];
        for i in 0..100u32 {
            let slot = arena.push(&dummy);
            assert_eq!(slot, i);
        }
        assert_eq!(arena.used_blocks(), 100);

        // flush 返回借用 arena 的引用；先取 buf_size/stride_val 的 owned 值（内部块里），
        // 再对外层 arena 检查 capacity（避免 mutable borrow + immutable borrow 重叠）。
        let (buf_size, stride_val);
        {
            let (buf, _bg, stride_u32) =
                arena.flush(ctx.device(), ctx.queue(), &backend.bind_group_layout);
            stride_val = stride_u32 as u64;
            buf_size = buf.size();
        }
        let capacity = arena.capacity_blocks();
        assert!(capacity >= 100);
        // next_power_of_two(max(100, 16)) = 128
        assert_eq!(capacity, 128);
        assert_eq!(stride_val, DYNAMIC_OFFSET_ALIGNMENT);
        assert_eq!(buf_size, 128u64 * DYNAMIC_OFFSET_ALIGNMENT);

        // begin_frame 重置 used_blocks，但 capacity 不变。
        arena.begin_frame();
        assert_eq!(arena.used_blocks(), 0);
        assert_eq!(arena.capacity_blocks(), 128);
    }

    // ── 原有 3 个测试：适配新结构 ─────────────────────────────────────

    #[test]
    fn test_backend_creation_does_not_panic() {
        let Some((backend, _ctx)) = headless_backend() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
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

        // 附加断言：后端创建时 mesh_cache 空、arena stride 256、capacity 0。
        assert_eq!(backend.mesh_cache.lock().expect("poisoned").len(), 0);
        assert_eq!(
            backend.arena.lock().expect("poisoned").stride(),
            DYNAMIC_OFFSET_ALIGNMENT
        );
        assert_eq!(backend.arena.lock().expect("poisoned").capacity_blocks(), 0);
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
        backend.notify_submit();
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
        // wireframe_cube 是 Lines 拓扑（第一个 draw）。
        assert_eq!(frame.draws[0].topology, PrimitiveTopology::Lines);
        // draw 1 是 line（也是 Lines）。
        assert_eq!(frame.draws[1].topology, PrimitiveTopology::Lines);

        // arena：2 个 draw → used_blocks == 2。
        assert_eq!(backend.arena.lock().expect("poisoned").used_blocks(), 2);
        // mesh cache：2 个唯一条目（1 mesh + 1 line）。
        assert_eq!(backend.mesh_cache.lock().expect("poisoned").len(), 2);
    }

    // ── G6 决定性测试：300 帧 buffer 创建不随帧增长 ─────────────────

    #[test]
    fn test_300_frames_no_growth() {
        let Some((backend, ctx)) = headless_backend() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let mut queue = DrawQueue3D::new();
        // 10 个不同 mesh（varying size → 不同内容哈希）。
        for size in 0..10u32 {
            let s = 0.5 + size as f32 * 0.1;
            queue.draw_mesh(
                wireframe_cube(s, Color32::new(255, 255, 255, 255)),
                Mat4::IDENTITY,
                Color32::new(255, 255, 255, 255),
            );
        }
        // 2 个不同 line（端点不同 → 不同内容哈希）。
        queue.draw_line(Vec3::ZERO, Vec3::X, Color32::RED);
        queue.draw_line(Vec3::ZERO, Vec3::Y, Color32::GREEN);

        // frame 0: warm-up。
        let f0 = backend.prepare_draws(
            ctx.device(),
            ctx.queue(),
            Mat4::IDENTITY,
            queue.meshes(),
            queue.lines(),
        );
        // 内容应当是 10+2=12 个 draw。
        assert_eq!(f0.draws.len(), 12);

        let arena_blocks_f0 = backend.arena.lock().expect("poisoned").capacity_blocks();
        let cache_len_f0 = backend.mesh_cache.lock().expect("poisoned").len();
        // 10 unique mesh + 2 unique line = 12 cached VB entries。
        assert_eq!(cache_len_f0, 12);
        // 12 draws 小于 MIN_BLOCKS(16)，capacity 应从 MIN 起步（16）。
        assert!(arena_blocks_f0 >= 16);

        backend.notify_submit();

        let mut stable_arena = arena_blocks_f0;
        let mut stable_cache = cache_len_f0;
        for frame in 1..300 {
            let _frame = backend.prepare_draws(
                ctx.device(),
                ctx.queue(),
                Mat4::IDENTITY,
                queue.meshes(),
                queue.lines(),
            );
            backend.notify_submit();
            let cache_len = backend.mesh_cache.lock().expect("poisoned").len();
            let arena_blocks = backend.arena.lock().expect("poisoned").capacity_blocks();
            if frame > 10 {
                assert_eq!(
                    cache_len, stable_cache,
                    "frame {frame}: cache len grew beyond stable ({stable_cache} -> {cache_len})"
                );
                assert!(
                    arena_blocks <= stable_arena,
                    "frame {frame}: arena blocks should not grow after warmup ({stable_arena} >= {arena_blocks})"
                );
            } else {
                stable_arena = stable_arena.max(arena_blocks);
                stable_cache = stable_cache.max(cache_len);
            }
        }
        // 内容去重断言：10 meshes + 2 lines = 12 vertex buffers cached。
        assert_eq!(
            stable_cache, 12,
            "should cache 10 unique mesh buffers + 2 unique line buffers"
        );
    }
}
