// wgpu_3d.rs
//
// WGPU 3D 后端（G2/G5/G6 资源生命周期改造）：
// - WgpuBackend3D：持有 device/queue 与两条管线（triangle list / line list），
//   共享一套 dynamic-offset uniform 绑定布局；
// - G2：mesh 缓冲缓存用 `ResourceKey::versioned`（几何 id + version）失效，
//   line 缓冲缓存用 `ResourceKey::content`（端点内容哈希）；
// - G5：UniformArena — 256 字节对齐的 dynamic uniform staging，flush 一次性 write_buffer；
// - G6：prepare_draws — mesh/line 顶点缓冲走 `mesh_cache`，
//   uniform 走 arena，缓存命中时不再 per-mesh per-frame 调用 `create_buffer_init`；
// - PreparedDraw3D / Frame3DData：只持有句柄克隆（wgpu::Buffer 是 ref-counted）。
//
// # RC.1 材质系统
//
// - 绑定组扩到 3 个 entry：binding 0 = uniform（dynamic）、binding 1 = `texture_2d<f32>`、
//   binding 2 = sampler。未注册纹理时绑定 1×1 透明黑兜底纹理（预乘 over 下退化为
//   材质色，见 `cube.wgsl`）；
// - 顶点布局扩到 20 字节：`Float32x3` 位置 @0 + `Float32x2` UV @12（`loc1`）。
//   UV 优先取几何自带 [`MeshGeometry::uvs`]，缺省时按 [`Material3D::uv_projection`] 兜底；
// - bind group 依赖 uniform buffer 与纹理视图，因此不再由 [`UniformArena`] 持有，
//   改由后端按纹理 id 缓存（`bind_groups`），buffer 重建（`generation` 变化）或
//   纹理注册时整体失效；
// - 离屏渲染目标按 [`LayerId`] 分别持有（`offscreen: HashMap<LayerId, RenderTarget>`），
//   解除 S3-2 遗留的"每类最多一个图层"限制。
//
// # RC.3 深度测试
//
// - 两条 3D 管线均带 `Depth32Float` 深度状态（`Less`，写入开）；
// - 深度目标按 [`LayerId`] 持有（`depth: HashMap<LayerId, DepthTarget>`），
//   与颜色目标同尺寸、尺寸变化时一并重建；
// - 深度附件只挂到 3D 的两个 pass（`render_layer_offscreen` / `submit_layer`）——
//   合成 pass 与 2D 离屏 pass 保持 `None`。
//
// egui 集成的回调（SceneCompositorCallback）见 scene_callback.rs。
//
// # G7 回校（R6）：为什么这里**不**接 `StaticConsistencyChecker`
//
// 计划倾向于"只接 3D 侧"，但本仓库的 3D 路径缺少接线所需的两个前提：
//   1. **没有顶点布局契约声明** —— 3D pipeline 的顶点布局是硬编码 bespoke
//      （`Float32x3` / stride 12），`VertexLayout` 从未参与，checker 的
//      `(layout, identity)` 对没有输入可比；
//   2. **没有 entity identity 契约** —— `DrawQueue3D` 不携带 entity id。
//      （RB 起 `MeshDraw` 已携带稳定几何 identity：`MeshGeometry::id()/version()`，
//      但它是"几何资源身份"而非"ECS 实体身份"，仍不足以支撑 checker 的
//      `(layout, identity)` 输入——顶点布局契约仍是第 1 点未解的问题。）
// 2D 侧则相反：内容（形变 / 相机 / group）每帧都变，接上必然大量误报。
// 因此 R6 决策 = **两侧都不接**，把事实记在此处；将来 3D 引入 VertexLayout 契约
// 声明 + 稳定 entity identity 后，可在 `prepare_draws` /
// `record_prepared` 入口补齐。

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::mem;
use std::num::NonZeroU64;
use std::sync::Mutex;

use glam::{Mat4, Vec2, Vec3};
use wgpu::util::DeviceExt as _;

use xcal_scene::domain::LayerId;
use xcal_scene::scene::components::{PrimitiveTopology, ShaderVariant, UvProjection};
use xc_notion::Color32;
use xcal_render::draw_queue_3d::{LineDraw, MeshDraw};
use xcal_render::{
    GpuHandle, RenderBackend3D, RenderLayer, RenderTarget, ResourceCache, ResourceKey, Viewport,
};

/// 默认着色器（cube.wgsl）：uniform = mat4 mvp + vec4 color，纹理 = 颜色调制。
pub const DEFAULT_SHADER: &str = include_str!("../shaders/cube.wgsl");

/// uniform 块大小：mat4x4（64 字节）+ vec4（16 字节）。
pub const UNIFORM_SIZE: u64 = 80;

/// wgpu dynamic uniform offset 要求的最小对齐。
pub const DYNAMIC_OFFSET_ALIGNMENT: u64 = 256;

/// 顶点步长：vec3 position（12 字节）+ vec2 uv（8 字节）+ vec3 normal（12 字节）= 32 字节。
const VERTEX_STRIDE: u64 = 32;
/// UV 属性在顶点内的字节偏移（紧随 position 之后）。
const VERTEX_UV_OFFSET: u64 = 12;
/// 法线属性在顶点内的字节偏移（紧随 uv 之后）。
const VERTEX_NORMAL_OFFSET: u64 = 20;

/// 未指定纹理时使用的兜底纹理 id（1×1 透明黑）。
const DEFAULT_TEXTURE_ID: u64 = 0;

/// [`RenderBackend3D::submit_layer`] 没有 `LayerId` 参数，用保留 id 承载其
/// 颜色/深度目标的键（颜色目标来自 `layer.target.view`，仅深度目标需按 id 缓存）。
pub const SUBMIT_LAYER_ID: LayerId = LayerId(u32::MAX);

/// RC.3：3D 深度缓冲格式（投影矩阵为 D3D/wgpu 约定，深度范围 [0, 1]）。
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

// ─── D2：着色变体 → pipeline 表 ─────────────────────────────────────────────

/// 后端预建 pipeline 时遍历的全部着色变体。
///
/// ⚠️ 新增 [`ShaderVariant`] 变体时**必须**同步扩此数组（否则该变体查表落空，会
/// `debug_assert` 并回退到 `Lambert`）。与 `compositor.rs` 的
/// `(LayerBlend, Conversion)` 双层预建同一写法。
const SHADER_VARIANTS: [ShaderVariant; 2] = [ShaderVariant::Lambert, ShaderVariant::Flat];

/// 着色变体 → 片元入口名（`@fragment fn`）的映射。
///
/// wgpu 无"动态入口点"，故变体只能靠"同一 shader module 的多个 `fs_*` 入口 + 各自
/// 预建的 pipeline"实现。`ShaderVariant` 定义在数据层 xcal-scene（不得引用 wgpu），
/// 因此该映射是后端侧的自由函数而非 `ShaderVariant` 的固有方法。
fn variant_entry_point(variant: ShaderVariant) -> &'static str {
    match variant {
        ShaderVariant::Flat => "fs_flat",
        ShaderVariant::Lambert => "fs_lambert",
    }
}

/// RC.3：一张离屏颜色目标配套的深度目标（尺寸变化时整体重建）。
struct DepthTarget {
    /// 持有纹理以延长生命周期（视图自身也引用纹理，此处显式保留）。
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

// ─── G5：UniformArena ──────────────────────────────────────────────────────

/// Dynamic uniform arena。stride 强制 256 字节对齐（wgpu dynamic uniform offset 的规范）。
///
/// 典型使用：
/// 1. `begin_frame()` 重置 used_blocks（staging 底层容量不释放，避免抖动）；
/// 2. `push(data)` 追加一帧内每个 draw 的 uniform 字节，返回块 index（0..used_blocks）；
/// 3. `flush(device, queue)`：必要时扩容 buffer，一次性 `write_buffer` 上传 staging
///    的全部数据。返回 (&buffer, stride_bytes)。
///
/// RC.1 起 arena **不再持有 bind group**：bind group 还需绑定材质纹理视图，
/// 由后端按纹理 id 缓存（见 [`WgpuBackend3D::bind_groups`]）。`generation()` 在
/// 每次重建 buffer 时递增，供后端判断其缓存的 bind group 是否已失效。
pub struct UniformArena {
    buffer: Option<wgpu::Buffer>,
    capacity_blocks: u32,
    used_blocks: u32,
    stride: u64,
    uniform_size: u64,
    staging: Vec<u8>,
    generation: u64,
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
            capacity_blocks: 0,
            used_blocks: 0,
            stride,
            uniform_size,
            staging: Vec::new(),
            generation: 0,
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
    /// 写回 staging 数据。返回 (&buffer, stride_bytes)。
    ///
    /// buffer 扩容（身份变化）时 `generation` 递增 —— 后端据此判定其按纹理缓存的
    /// bind group 已失效并整体重建。
    pub fn flush<'a>(
        &'a mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> (&'a wgpu::Buffer, u32) {
        let needed_blocks = self.used_blocks.max(Self::MIN_BLOCKS);

        if needed_blocks > self.capacity_blocks {
            let new_blocks = needed_blocks.next_power_of_two();
            let new_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("xcal-3d-uniform-arena"),
                size: new_blocks as u64 * self.stride,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // 缓冲身份切换 -> 引用它的 bind group 必须重建。
            self.generation += 1;
            self.buffer = Some(new_buf);
            self.capacity_blocks = new_blocks;
        }

        let buffer = self.buffer.as_ref().expect("buffer must be created");
        let used_bytes = (self.used_blocks as u64 * self.stride) as usize;
        if used_bytes > 0 {
            queue.write_buffer(buffer, 0, &self.staging[..used_bytes]);
        }

        (buffer, self.stride as u32)
    }

    /// uniform buffer 的版本号：每次重建（扩容）递增。
    #[inline]
    pub fn generation(&self) -> u64 {
        self.generation
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
/// - `bind_group`：材质 bind group（按纹理 id 缓存，见 [`BindGroupCache`]）。
///   绑定本帧 arena buffer + 材质纹理视图 + sampler；
/// - `uniform_offset`：dynamic uniform offset（字节，256 对齐）。
pub struct PreparedDraw3D {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: Option<wgpu::Buffer>,
    pub vertex_count: u32,
    pub index_count: u32,
    pub topology: PrimitiveTopology,
    /// 着色变体：与 `topology` 一起构成 pipeline 表的查表键。
    pub variant: ShaderVariant,
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

// ─── S3-2/C3：3D 图层离屏帧 ─────────────────────────────────────────────────

/// 一个 3D 图层渲染到**自己的离屏纹理**后的结果（交给合成器采样）。
///
/// 与 [`Frame3DData`] 的区别：`Frame3DData` 是"已上传的绘制资源"，本结构是
/// "已画进一张纹理的像素"，由 `SceneCompositorCallback::prepare` 交给 [`crate::Compositor`]。
pub struct Layer3DFrame {
    /// 本层离屏纹理视图（合成器的采样源）。
    pub offscreen: wgpu::TextureView,
    /// 离屏纹理格式（= 后端构造时的 `format`）；合成器据此选域转换入口。
    pub format: wgpu::TextureFormat,
    /// 保活：本帧 uniform arena buffer（`draws` 的 bind group 引用它）。
    pub _arena_buffer: wgpu::Buffer,
}

// ─── WgpuBackend3D ──────────────────────────────────────────────────────────

/// 材质 bind group 缓存（按纹理 id 索引）。
///
/// bind group 绑定 uniform arena buffer（binding 0）+ 纹理视图（binding 1）+ sampler
/// （binding 2），因此其有效性取决于两者：`arena_generation` 记录创建时的
/// [`UniformArena::generation`]，不一致即整体丢弃重建；纹理注册时也整体清空
/// （见 [`WgpuBackend3D::register_texture`]）。
#[derive(Default)]
struct BindGroupCache {
    arena_generation: u64,
    entries: HashMap<u64, wgpu::BindGroup>,
}

/// WGPU 3D 渲染后端（设备句柄 + 管线 + 绑定布局 + mesh 缓存 + uniform arena）。
///
/// 内部状态（cache / arena）用 `Mutex` 保护：`RenderBackend3D` 要求 `Send + Sync`，
/// 而 prepare_draws / record_prepared 是 `&self`，但需要修改缓存和 arena。
pub struct WgpuBackend3D {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// 管线目标格式（3D 离屏纹理沿用该格式，合成器据此判域）。
    format: wgpu::TextureFormat,
    /// 预建的渲染管线表：`(topology, variant)` → pipeline。
    ///
    /// 所有变体共用同一条 `pipeline_layout` 与同一 bind group layout（uniform /
    /// texture / sampler 三槽），故只需一张表。**已知限制**：若将来某变体需要额外
    /// 资源槽（粗糙度贴图 / 环境图），需引入第二套 bind group layout 与对应的
    /// pipeline 族，本表不再够用。
    pipelines: HashMap<(PrimitiveTopology, ShaderVariant), wgpu::RenderPipeline>,
    bind_group_layout: wgpu::BindGroupLayout,
    /// 材质采样器（binding 2）。
    sampler: wgpu::Sampler,
    /// 兜底纹理视图（1×1 透明黑，binding 1）：材质未指定纹理时使用。
    default_texture_view: wgpu::TextureView,
    /// 已注册纹理视图（`TextureHandle::id` → view）。
    textures: Mutex<HashMap<u64, wgpu::TextureView>>,
    /// 按纹理 id 缓存的材质 bind group。
    bind_groups: Mutex<BindGroupCache>,
    /// mesh/line 顶点（+可选索引）缓冲缓存。
    /// mesh key = `ResourceKey::versioned`（几何 id + version/UV 来源）+ `MeshVertexTag`；
    /// line key = `ResourceKey::content`（端点内容哈希）+ `LineVertexTag`；
    /// meta = ()；实际 GPU 句柄以 `GpuHandle<CachedMeshBuffers>` 形式存于 ResourceCache。
    mesh_cache: Mutex<ResourceCache<()>>,
    /// 256 对齐 dynamic uniform arena。
    arena: Mutex<UniformArena>,
    /// 按 [`LayerId`] 分别持有的离屏渲染目标（尺寸变化时重建）。
    offscreen: Mutex<HashMap<LayerId, RenderTarget>>,
    /// RC.3：按 [`LayerId`] 分别持有的深度目标，尺寸随颜色目标一同重建。
    depth: Mutex<HashMap<LayerId, DepthTarget>>,
}

impl WgpuBackend3D {
    /// 用给定 WGSL 源码构造后端（按 `(topology, variant)` 预建管线表）。
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        shader_source: &str,
    ) -> Self {
        let device = device.clone();
        let queue = queue.clone();

        // RC.1: binding 0 = uniform with dynamic offset；binding 1 = 材质纹理；
        // binding 2 = 采样器。
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("xcal-3d-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: Some(NonZeroU64::new(UNIFORM_SIZE).unwrap()),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
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

        // RC.1: 顶点 = 位置（Float32x3 @0）+ UV（Float32x2 @12）。
        // RC 补充: 追加法线（Float32x3 @20）供 shader 做 Lambert 着色。
        let vertex_layout = [Some(wgpu::VertexBufferLayout {
            array_stride: VERTEX_STRIDE,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: 0,
                    shader_location: 0,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x2,
                    offset: VERTEX_UV_OFFSET,
                    shader_location: 1,
                },
                wgpu::VertexAttribute {
                    format: wgpu::VertexFormat::Float32x3,
                    offset: VERTEX_NORMAL_OFFSET,
                    shader_location: 2,
                },
            ],
        })];

        let make_pipeline = |topology: wgpu::PrimitiveTopology,
                             cull_mode: Option<wgpu::Face>,
                             fs_entry: &'static str,
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
                    entry_point: Some(fs_entry),
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
                // RC.3：深度测试（Less：近处像素深度更小者胜出），写入深度。
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        // 预建全部 `(topology, variant)` 组合（三角形 + 线段；点列表当前无 pipeline）。
        let mut pipelines: HashMap<(PrimitiveTopology, ShaderVariant), wgpu::RenderPipeline> =
            HashMap::new();
        for variant in SHADER_VARIANTS {
            let fs_entry = variant_entry_point(variant);
            // 三角形：背面剔除。
            pipelines.insert(
                (PrimitiveTopology::Triangles, variant),
                make_pipeline(
                    wgpu::PrimitiveTopology::TriangleList,
                    Some(wgpu::Face::Back),
                    fs_entry,
                    "xcal-3d-pipeline-triangles",
                ),
            );
            // 线段：无剔除。
            pipelines.insert(
                (PrimitiveTopology::Lines, variant),
                make_pipeline(
                    wgpu::PrimitiveTopology::LineList,
                    None,
                    fs_entry,
                    "xcal-3d-pipeline-lines",
                ),
            );
        }

        // RC.1: 1×1 **透明黑**兜底纹理 —— 材质无纹理时绑定它。着色式为预乘 over
        // （`tex + color * (1 - tex.a)`），`tex = (0,0,0,0)` 时退化为 `color`。
        let default_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xcal-3d-default-texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &default_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[0u8, 0, 0, 0],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let default_texture_view =
            default_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("xcal-3d-material-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        Self {
            device,
            queue,
            format,
            pipelines,
            bind_group_layout,
            sampler,
            default_texture_view,
            textures: Mutex::new(HashMap::new()),
            bind_groups: Mutex::new(BindGroupCache::default()),
            mesh_cache: Mutex::new(ResourceCache::new()),
            arena: Mutex::new(UniformArena::new(UNIFORM_SIZE, DYNAMIC_OFFSET_ALIGNMENT)),
            offscreen: Mutex::new(HashMap::new()),
            depth: Mutex::new(HashMap::new()),
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

    /// 管线/离屏目标格式。
    pub fn format(&self) -> wgpu::TextureFormat {
        self.format
    }

    /// 注册（或替换）一个纹理视图，供材质按 `TextureHandle::id` 引用。
    ///
    /// 注册会使已缓存的材质 bind group 整体失效（视图可能变化）。
    /// 加锁顺序与 [`Self::prepare_draws`] 一致（bind_groups → textures），避免锁序反转。
    pub fn register_texture(&self, id: u64, view: wgpu::TextureView) {
        let mut bind_groups = self.bind_groups.lock().expect("bind_groups poisoned");
        self.textures
            .lock()
            .expect("textures poisoned")
            .insert(id, view);
        bind_groups.entries.clear();
    }

    /// 解析材质纹理视图：未注册的 id 回退到 1×1 透明黑兜底纹理。
    fn resolve_texture_view(&self, texture_id: u64) -> wgpu::TextureView {
        let guard = self.textures.lock().expect("textures poisoned");
        match guard.get(&texture_id) {
            Some(view) => view.clone(),
            None => self.default_texture_view.clone(),
        }
    }

    /// 创建材质 bind group：uniform（dynamic）+ 纹理视图 + sampler。
    fn create_material_bind_group(
        &self,
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("xcal-3d-material-bind-group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(UNIFORM_SIZE),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    /// 取得（必要时按 `viewport` 尺寸重建）指定图层的离屏渲染目标视图。
    fn offscreen_view(
        &self,
        device: &wgpu::Device,
        layer_id: LayerId,
        viewport: Viewport,
    ) -> wgpu::TextureView {
        let width = viewport.w.max(1);
        let height = viewport.h.max(1);
        let mut guard = self.offscreen.lock().expect("offscreen poisoned");
        let recreate = match guard.get(&layer_id) {
            Some(t) => t.size.width != width || t.size.height != height || t.format != self.format,
            None => true,
        };
        if recreate {
            guard.insert(
                layer_id,
                RenderTarget::new_offscreen(
                    device,
                    wgpu::Extent3d {
                        width,
                        height,
                        depth_or_array_layers: 1,
                    },
                    self.format,
                ),
            );
        }
        guard
            .get(&layer_id)
            .expect("offscreen target")
            .view()
            .clone()
    }

    /// 指定图层的 3D 离屏纹理（若已渲染过）；用于回读/诊断。
    pub fn offscreen_texture(&self, layer_id: LayerId) -> Option<wgpu::Texture> {
        self.offscreen
            .lock()
            .expect("offscreen poisoned")
            .get(&layer_id)
            .and_then(|t| t.texture.clone())
    }

    /// RC.3：取得（必要时按给定尺寸重建）指定图层的深度目标视图。
    ///
    /// 深度目标与颜色目标同尺寸、同 [`LayerId`] 键，尺寸变化时一并重建。
    fn depth_view(&self, device: &wgpu::Device, layer_id: LayerId, width: u32, height: u32) -> wgpu::TextureView {
        let width = width.max(1);
        let height = height.max(1);
        let mut guard = self.depth.lock().expect("depth poisoned");
        let recreate = match guard.get(&layer_id) {
            Some(t) => t.size != (width, height),
            None => true,
        };
        if recreate {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("xcal-3d-depth-texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            guard.insert(
                layer_id,
                DepthTarget {
                    _texture: texture,
                    view,
                    size: (width, height),
                },
            );
        }
        guard.get(&layer_id).expect("depth target").view.clone()
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
        // skeleton：暂存 draw 信息，bind_group 在 flush 之后再统一解析。
        struct Skeleton {
            vertex_buffer: wgpu::Buffer,
            index_buffer: Option<wgpu::Buffer>,
            vertex_count: u32,
            index_count: u32,
            topology: PrimitiveTopology,
            /// 着色变体（pipeline 表的查表键的一半）。
            variant: ShaderVariant,
            uniform_offset: u32,
            /// 材质纹理 id（bind group 的缓存键）。
            texture_id: u64,
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

        // ── MeshDraws：按几何 id + version 寻址（几何内容变化时 bump_version 失效）──
        for md in meshes {
            let geometry = &md.geometry;
            // UV 来源（几何自带 vs 兜底策略）参与缓存键：同一几何在不同投影策略下
            // 顶点数据不同，必须区分（见 `uv_source_code`）。
            let uv_code = uv_source_code(geometry.uvs.is_some(), md.material.uv_projection);
            let key_version = geometry.version().wrapping_mul(4).wrapping_add(uv_code);
            let key = ResourceKey::versioned::<MeshVertexTag>(geometry.id(), key_version);

            // 先查命中；命中直接取缓存里的字段副本；未命中创建 VB/IB 并 insert_handle。
            let cached_copy = if let Some(handle_ref) = cache_guard.get(key) {
                extract_buffers(handle_ref)
            } else {
                let uvs = match &geometry.uvs {
                    Some(uvs) => uvs.clone(),
                    None => fallback_uvs(&geometry.positions, md.material.uv_projection),
                };
                let vertices =
                    interleaved_pos_uv_normal(&geometry.positions, &uvs, geometry.normals.as_deref());
                let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("xcal-3d-mesh-vertices"),
                    contents: bytes_of(&vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let ib = geometry.indices.as_ref().map(|indices| {
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("xcal-3d-mesh-indices"),
                        contents: bytes_of(indices),
                        usage: wgpu::BufferUsages::INDEX,
                    })
                });
                let cached = CachedMeshBuffers {
                    vertex_buffer: vb,
                    index_buffer: ib,
                    vertex_count: geometry.positions.len() as u32,
                    index_count: geometry
                        .indices
                        .as_ref()
                        .map(|v| v.len() as u32)
                        .unwrap_or(0),
                    topology: geometry.topology,
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
            let bytes = uniform_bytes(mvp, md.material.color);
            let block_index = arena_guard.push(&bytes);
            let offset_bytes = block_index as u32 * arena_guard.stride() as u32;

            skeletons.push(Skeleton {
                vertex_buffer: cached_copy.vertex_buffer,
                index_buffer: cached_copy.index_buffer,
                vertex_count: cached_copy.vertex_count,
                index_count: cached_copy.index_count,
                topology: cached_copy.topology,
                variant: md.material.variant,
                uniform_offset: offset_bytes,
                texture_id: md
                    .material
                    .texture
                    .map(|t| t.id())
                    .unwrap_or(DEFAULT_TEXTURE_ID),
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
                // line 无材质纹理 → UV 恒为 0；法线填 (0, 0, 1)，
                // 配合默认头顶光（L = (0, 0, 1)）得 lit = 1，颜色不受光照影响。
                let vertices: [[f32; 8]; 2] = [
                    [line.start.x, line.start.y, line.start.z, 0.0, 0.0, 0.0, 0.0, 1.0],
                    [line.end.x, line.end.y, line.end.z, 0.0, 0.0, 0.0, 0.0, 1.0],
                ];
                let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("xcal-3d-line-vertices"),
                    contents: bytes_of(&vertices),
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
                // line 默认走 Lambert（法线填 (0,0,1) → lit = 1，外观不受光照影响）。
                variant: ShaderVariant::Lambert,
                uniform_offset: offset_bytes,
                // line 无材质纹理 → 绑定 1×1 **透明黑**兜底纹理（`DEFAULT_TEXTURE_ID = 0`
                // 未注册，`resolve_texture_view` 回退），着色式退化为 `color * lit`。
                texture_id: DEFAULT_TEXTURE_ID,
            });
        }

        // 明确释放 cache guard（MutexGuard），之后再进行 arena.flush。
        drop(cache_guard);

        // ── Flush arena：一次性把本帧全部 uniform 写进 GPU buffer ───
        let (arena_buf, _stride_u32) = arena_guard.flush(device, queue);
        // 把返回的借用转成 owned clone（wgpu handle clone 是 ref-count 浅拷贝）。
        let arena_buf_owned: wgpu::Buffer = arena_buf.clone();
        let arena_generation = arena_guard.generation();
        // 释放 arena guard，使其不再被借用（之后可以 lock bind_groups）。
        drop(arena_guard);

        // ── 按纹理 id 解析 / 缓存材质 bind group ────────────────────
        // bind group 绑定 arena buffer + 纹理视图 + sampler。buffer 重建
        // （generation 变化）时全部失效；纹理注册时由 register_texture 清空。
        let mut bg_guard = self.bind_groups.lock().expect("bind_groups poisoned");
        if bg_guard.arena_generation != arena_generation {
            bg_guard.entries.clear();
            bg_guard.arena_generation = arena_generation;
        }
        let draws_final: Vec<PreparedDraw3D> = skeletons
            .into_iter()
            .map(|s| {
                let bind_group = match bg_guard.entries.get(&s.texture_id) {
                    Some(bg) => bg.clone(),
                    None => {
                        let view = self.resolve_texture_view(s.texture_id);
                        let bg = self.create_material_bind_group(device, &arena_buf_owned, &view);
                        bg_guard.entries.insert(s.texture_id, bg.clone());
                        bg
                    }
                };
                PreparedDraw3D {
                    vertex_buffer: s.vertex_buffer,
                    index_buffer: s.index_buffer,
                    vertex_count: s.vertex_count,
                    index_count: s.index_count,
                    topology: s.topology,
                    variant: s.variant,
                    uniform_offset: s.uniform_offset,
                    bind_group,
                }
            })
            .collect();
        drop(bg_guard);

        Frame3DData {
            draws: draws_final,
            _arena_buffer: arena_buf_owned,
        }
    }

    /// 把已准备的绘制命令录制进外部 render pass（egui 回调路径）。
    pub fn record_prepared<'a>(&self, draws: &[PreparedDraw3D], rpass: &mut wgpu::RenderPass<'a>) {
        for d in draws {
            // 点列表当前无 pipeline —— 维持既有行为：跳过。
            if d.topology == PrimitiveTopology::Points {
                continue;
            }
            // 查表选 pipeline（`(topology, variant)`）。未命中（变体未预建）时回退到
            // 同拓扑的 Lambert，避免未知变体导致画面消失；debug 下断言以暴露漏建。
            let key = (d.topology, d.variant);
            let Some(pipeline) = self.pipelines.get(&key).or_else(|| {
                debug_assert!(
                    self.pipelines.contains_key(&key),
                    "3D pipeline 未预建：{key:?}；已回退到 Lambert（请补齐变体 / fs 入口 / pipeline 表）",
                );
                self.pipelines.get(&(d.topology, ShaderVariant::Lambert))
            }) else {
                continue;
            };
            rpass.set_pipeline(pipeline);
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

    /// C3：把一个 3D 图层渲染到**自己的离屏纹理**（透明清屏）。
    ///
    /// 产出 [`Layer3DFrame`]，由 `SceneCompositorCallback::prepare` 交给合成器按声明顺序混合。
    /// 离屏纹理按 `viewport` 尺寸缓存，尺寸不变时不重建。清屏 alpha = 0：未绘制处
    /// 保持透明，合成时不会盖住背景与下层（这是"图层可任意叠放"的前提）。
    pub fn render_layer_offscreen(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        layer_id: LayerId,
        view_proj: Mat4,
        viewport: Viewport,
        meshes: &[MeshDraw],
        lines: &[LineDraw],
    ) -> Layer3DFrame {
        let frame = self.prepare_draws(&self.device, &self.queue, view_proj, meshes, lines);
        let offscreen = self.offscreen_view(&self.device, layer_id, viewport);
        // RC.3：本层深度目标（与离屏颜色目标同尺寸），清为最远（1.0）。
        let depth_view = self.depth_view(&self.device, layer_id, viewport.w, viewport.h);

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xcal-3d-layer-offscreen-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &offscreen,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            self.record_prepared(&frame.draws, &mut pass);
        }

        Layer3DFrame {
            offscreen,
            format: self.format,
            _arena_buffer: frame._arena_buffer,
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

        // RC.3：本层深度目标（尺寸取自颜色目标），清为最远（1.0）。
        let depth_view = self.depth_view(
            &self.device,
            SUBMIT_LAYER_ID,
            layer.target.size.width,
            layer.target.size.height,
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
                        // S3-2/C3：图层离屏渲染必须透明清屏，否则会盖住背景与下层。
                        a: 0.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
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

/// UV 来源编码，参与 mesh 顶点缓冲的缓存键。
///
/// 同一几何在不同 UV 来源下顶点数据不同，必须区分：几何自带 UV → 0；
/// 否则按 [`UvProjection`] 兜底策略编码（Planar → 1 / Spherical → 2 / Triplanar → 3）。
fn uv_source_code(has_uvs: bool, projection: UvProjection) -> u64 {
    if has_uvs {
        0
    } else {
        match projection {
            UvProjection::Planar => 1,
            UvProjection::Spherical => 2,
            UvProjection::Triplanar => 3,
        }
    }
}

/// 交错顶点数据：每顶点 `[pos.xyz, uv.xy, normal.xyz]`（8 个 f32 = 32 字节）。
///
/// `uvs` 必须与 `positions` 等长（几何自带或 [`fallback_uvs`] 兜底）——不等长时
/// 对缺失 UV 以 (0, 0) 补齐，保证布局稳定。`normals` 为 `None`（如线框立方体）
/// 或长度不足时，对应顶点以 `(0, 0, 1)` 补齐：配合默认头顶
/// 光照（`L = (0, 0, 1)`）得到 `lit = 1`，故未携带法线的几何外观与开启光照前一致。
fn interleaved_pos_uv_normal(
    positions: &[Vec3],
    uvs: &[Vec2],
    normals: Option<&[Vec3]>,
) -> Vec<f32> {
    let mut out = Vec::with_capacity(positions.len() * 8);
    for (i, p) in positions.iter().enumerate() {
        let uv = uvs.get(i).copied().unwrap_or(Vec2::ZERO);
        let n = normals
            .and_then(|ns| ns.get(i).copied())
            .unwrap_or(Vec3::Z);
        out.extend_from_slice(&[p.x, p.y, p.z, uv.x, uv.y, n.x, n.y, n.z]);
    }
    out
}

/// 几何未携带 UV 时的兜底 UV（包围盒归一化到 [0, 1]）。
///
/// - [`UvProjection::Planar`]：以 XY 平面投影（忽略 Z）；
/// - [`UvProjection::Spherical`]：围绕包围盒中心的方向向量球面映射；
/// - [`UvProjection::Triplanar`]：逐顶点按相对包围盒中心偏移最大的分量选主轴投影。
///
/// 退化包围盒（任一轴跨度为 0）时按最小值 1e-6 处理，避免除零。
fn fallback_uvs(positions: &[Vec3], projection: UvProjection) -> Vec<Vec2> {
    if positions.is_empty() {
        return Vec::new();
    }
    let mut min = positions[0];
    let mut max = positions[0];
    for p in positions {
        min = min.min(*p);
        max = max.max(*p);
    }
    let size = (max - min).max(Vec3::splat(1e-6));
    let center = (min + max) * 0.5;

    positions
        .iter()
        .map(|p| {
            let local = *p - min;
            match projection {
                UvProjection::Planar => Vec2::new(local.x / size.x, local.y / size.y),
                UvProjection::Spherical => {
                    let d = (*p - center).normalize_or_zero();
                    // 方向向量 [-1, 1] → [0, 1]。
                    Vec2::new(d.x * 0.5 + 0.5, d.y * 0.5 + 0.5)
                }
                UvProjection::Triplanar => {
                    // 取相对中心偏移最大的分量作为主轴，对该主轴的正交两轴归一化。
                    let rel = *p - center;
                    let (ax, ay) = if rel.x.abs() >= rel.y.abs() && rel.x.abs() >= rel.z.abs() {
                        (local.y / size.y, local.z / size.z)
                    } else if rel.y.abs() >= rel.z.abs() {
                        (local.x / size.x, local.z / size.z)
                    } else {
                        (local.x / size.x, local.y / size.y)
                    };
                    Vec2::new(ax, ay)
                }
            }
        })
        .collect()
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_context::RenderContext;
    use xcal_scene::scene::components::{Material3D, MeshGeometry, TextureHandle};
    use xc_object::{Camera3D, Projection};
    use xcal_scene::scene::primitives::{parametric_surface, wireframe_cube};
    use xc_notion::{SurfaceFn, Vec3};
    use xcal_render::{DrawQueue3D, Viewport};

    /// 3D 后端的 headless 夹具：**守卫必须活到测试结束**（调用点绑定 `_gpu`）。
    #[track_caller]
    fn headless_backend() -> Option<(crate::gpu_test::GpuGuard, WgpuBackend3D, RenderContext)> {
        let (gpu, ctx) = crate::gpu_test::require_gpu()?;
        let backend =
            WgpuBackend3D::with_default_shader(ctx.device(), ctx.queue(), ctx.target_format());
        Some((gpu, backend, ctx))
    }

    // ── P0-1：SHADER_VARIANTS 穷举（纯 CPU，无需 GPU） ────────────────

    /// 编译期穷举：`ShaderVariant` 新增变体而此处未更新 → 编译失败。
    fn all_variants() -> [ShaderVariant; 2] {
        // 显式列出每个变体；漏一个即非穷尽 match → 编译失败。
        match ShaderVariant::Lambert {
            ShaderVariant::Lambert => {}
            ShaderVariant::Flat => {}
        }
        [ShaderVariant::Lambert, ShaderVariant::Flat]
    }

    /// `SHADER_VARIANTS` 必须覆盖 `ShaderVariant` 的全部变体。
    ///
    /// 证伪：从 `SHADER_VARIANTS` 删一项 → 本测试变红。
    #[test]
    fn shader_variants_array_is_exhaustive() {
        for v in all_variants() {
            assert!(
                SHADER_VARIANTS.contains(&v),
                "{v:?} 未在 SHADER_VARIANTS 中注册"
            );
        }
        assert_eq!(SHADER_VARIANTS.len(), all_variants().len());
    }

    // ── G2：几何缓存键单测（纯 CPU，headless，无需 GPU） ──────────────

    fn basic_mesh() -> MeshGeometry {
        MeshGeometry::new(
            vec![
                glam::Vec3::new(-1.0, -1.0, 0.0),
                glam::Vec3::new(1.0, -1.0, 0.0),
                glam::Vec3::new(0.0, 1.0, 0.0),
            ],
            Some(vec![0, 1, 2]),
            PrimitiveTopology::Triangles,
        )
    }

    /// `ResourceKey::Versioned`：版本不变则持续命中，`bump_version` 后缓存失效。
    #[test]
    fn test_versioned_key_invalidates_cache_on_bump() {
        let mut cache: ResourceCache<()> = ResourceCache::new();
        cache.begin_frame();

        let mut geometry = basic_mesh();
        let key_v0 = ResourceKey::versioned::<MeshVertexTag>(geometry.id(), geometry.version());
        assert!(cache.get(key_v0).is_none());
        let _ = cache.insert_handle(key_v0, (), GpuHandle::new(0u32));
        assert_eq!(cache.len(), 1);
        assert!(cache.get(key_v0).is_some());

        // 版本未变：同一 key，缓存继续命中，不产生新条目。
        let key_again = ResourceKey::versioned::<MeshVertexTag>(geometry.id(), geometry.version());
        assert_eq!(key_v0, key_again);
        assert_eq!(cache.len(), 1);

        // 版本递增：key 变化 → 缓存未命中（触发重建）。
        geometry.bump_version();
        let key_v1 = ResourceKey::versioned::<MeshVertexTag>(geometry.id(), geometry.version());
        assert_ne!(key_v0, key_v1);
        assert!(cache.get(key_v1).is_none());

        // 几何 id 跨 clone 稳定 → clone 与本体共享同一缓存条目。
        let clone = geometry.clone();
        assert_eq!(clone.id(), geometry.id());
        assert_eq!(
            ResourceKey::versioned::<MeshVertexTag>(clone.id(), clone.version()),
            key_v1
        );
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
        let Some((_gpu, _backend, ctx)) = headless_backend() else {
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
            let (buf, stride_u32) = arena.flush(ctx.device(), ctx.queue());
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
        let Some((_gpu, backend, _ctx)) = headless_backend() else {
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
        let Some((_gpu, backend, ctx)) = headless_backend() else {
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
            std::sync::Arc::new(wireframe_cube(1.0)),
            Mat4::IDENTITY,
            Material3D::color(Color32::new(255, 255, 255, 255)),
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

    /// C3 验收（真实设备）：3D 图层渲染到自己的离屏纹理，且**清屏为透明**。
    ///
    /// 空队列 → 离屏纹理全部应为 `[0,0,0,0]`。这是"图层可任意叠放"的前提：
    /// 若清屏不透明（旧值 alpha = 1.0），该层会像一张不透明卡片盖住背景与下层。
    #[test]
    fn test_render_layer_offscreen_clears_transparent() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        // 64 × 4 = 256 字节 = COPY_BYTES_PER_ROW_ALIGNMENT，回读无需补行距。
        let size = 64u32;
        let viewport = Viewport::full(size, size);
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test-3d-offscreen-encoder"),
            });
        let frame = backend.render_layer_offscreen(
            &mut encoder,
            LayerId(0),
            Mat4::IDENTITY,
            viewport,
            &[],
            &[],
        );
        assert_eq!(frame.format, ctx.target_format());
        ctx.submit(encoder.finish());
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll 失败");

        let texture = backend
            .offscreen_texture(LayerId(0))
            .expect("离屏纹理应已创建");
        assert_eq!((texture.width(), texture.height()), (size, size));
        let pixels = read_rgba8(ctx.device(), ctx.queue(), &texture, size, size);
        assert_eq!(pixels.len(), (size * size * 4) as usize);
        assert!(
            pixels.iter().all(|&b| b == 0),
            "C3：离屏清屏必须透明（首个非零字节 = {:?})",
            pixels.iter().position(|&b| b != 0)
        );
    }

    /// 回读 `Rgba8` 纹理（`width × 4` 恰为 256 对齐时无需补行距）。
    fn read_rgba8(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        let unpadded = width * 4;
        let padded = unpadded.div_ceil(256) * 256;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("test-3d-readback"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-3d-readback-encoder"),
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);

        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |r| r.expect("map_async 失败"));
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll 失败");
        let data = slice.get_mapped_range().expect("映射失败");
        let mut out = Vec::with_capacity((unpadded * height) as usize);
        for row in 0..height {
            let start = (row * padded) as usize;
            out.extend_from_slice(&data[start..start + unpadded as usize]);
        }
        drop(data);
        buffer.unmap();
        out
    }

    #[test]
    fn test_prepare_draws_counts() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let mut queue = DrawQueue3D::new();
        queue.draw_mesh(
            std::sync::Arc::new(wireframe_cube(1.0)),
            Mat4::IDENTITY,
            Material3D::color(Color32::new(255, 255, 255, 255)),
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
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let mut queue = DrawQueue3D::new();
        // 10 个不同 mesh（varying size → 不同几何 id，各自唯一缓存条目）。
        for size in 0..10u32 {
            let s = 0.5 + size as f32 * 0.1;
            queue.draw_mesh(
                std::sync::Arc::new(wireframe_cube(s)),
                Mat4::IDENTITY,
                Material3D::color(Color32::new(255, 255, 255, 255)),
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
        // 缓存条目断言：10 meshes + 2 lines = 12 vertex buffers cached。
        assert_eq!(
            stable_cache, 12,
            "should cache 10 unique mesh buffers + 2 unique line buffers"
        );
    }

    // ── RC.1 §4.2：材质纹理采样（像素级断言） ────────────────────────

    /// 注册 1×1 绿色纹理并让全屏四边形采样它；整屏像素应等于该绿色。
    ///
    /// 若材质纹理未接线（回退到 1×1 透明兜底纹理），预乘 over 下会退化为材质色
    /// 白色而非绿色 → 失败。
    #[test]
    fn test_material_samples_registered_texture() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 64u32;

        // 注册 1×1 纯绿纹理（id = 42）。
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("test-material-green-texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[0u8, 255, 0, 255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        backend.register_texture(
            42,
            texture.create_view(&wgpu::TextureViewDescriptor::default()),
        );

        // 全屏四边形：NDC x/y 覆盖 [-1, 1]，z = 0.5（在 [0, 1] 深度范围内）。
        // 顶点绕序从 +z 观察为 CCW，与默认背面剔除约定一致。
        let quad = MeshGeometry::new(
            vec![
                glam::Vec3::new(-1.0, -1.0, 0.5),
                glam::Vec3::new(1.0, -1.0, 0.5),
                glam::Vec3::new(1.0, 1.0, 0.5),
                glam::Vec3::new(-1.0, 1.0, 0.5),
            ],
            Some(vec![0, 1, 2, 0, 2, 3]),
            PrimitiveTopology::Triangles,
        )
        .with_uvs(vec![
            glam::Vec2::new(0.0, 1.0),
            glam::Vec2::new(1.0, 1.0),
            glam::Vec2::new(1.0, 0.0),
            glam::Vec2::new(0.0, 0.0),
        ]);

        let mut material = Material3D::color(Color32::WHITE);
        material.texture = Some(TextureHandle::new(42));
        let draws = [MeshDraw::new(
            std::sync::Arc::new(quad),
            Mat4::IDENTITY,
            material,
        )];

        let pixels = render_planes(&backend, &ctx, LayerId(7), size, &draws);
        // 目标格式 Bgra8UnormSrgb：内存字节序为 [B, G, R, A]，纯绿 = [0, 255, 0, 255]。
        let green = [0u8, 255, 0, 255];
        let first_bad = pixels.chunks_exact(4).position(|p| p != green);
        assert!(
            first_bad.is_none(),
            "材质应采样已注册的绿色纹理；首个非绿像素索引 = {first_bad:?}"
        );
    }

    // ── RC.3 §4.6：深度遮挡（像素级断言） ───────────────────────────

    /// 两个同向平面（中心方块 + 全屏平面）重叠，覆盖关系仅由深度决定。
    ///
    /// - Case 1：中心块 z = 0.25（近，红）+ 全屏 z = 0.75（远，蓝），**先画近的**；
    ///   中心像素应为红（近者胜出，远的虽后画仍被深度拒绝），角落像素为蓝。
    /// - Case 2：中心块 z = 0.75（远，红）+ 全屏 z = 0.25（近，蓝），**先画近的全屏**；
    ///   中心像素应保持蓝（远者后画仍被拒绝），角落像素为蓝。
    ///
    /// 两例的"远处几何后绘制"都被深度拒绝；去掉深度测试则中心像素会变成后画的颜色。
    #[test]
    fn test_depth_test_occludes_farther_geometry() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 64u32;
        // Bgra8UnormSrgb 字节序下的纯红 / 纯蓝。
        let red = [0u8, 0, 255, 255];
        let blue = [255u8, 0, 0, 255];

        // Case 1：先画近的红中心块，再画远的蓝全屏平面。
        let case1 = [
            flat_plane(0.5, 0.5, 0.25, Color32::RED),
            flat_plane(1.0, 1.0, 0.75, Color32::BLUE),
        ];
        let pixels = render_planes(&backend, &ctx, LayerId(11), size, &case1);
        assert_eq!(
            pixel_at(&pixels, size, 32, 32),
            red,
            "近的中心块应遮住后绘制的远处全屏平面"
        );
        assert_eq!(
            pixel_at(&pixels, size, 4, 4),
            blue,
            "角落仅有远的全屏平面覆盖"
        );

        // Case 2：先画近的蓝全屏平面，再画远的红中心块。
        let case2 = [
            flat_plane(1.0, 1.0, 0.25, Color32::BLUE),
            flat_plane(0.5, 0.5, 0.75, Color32::RED),
        ];
        let pixels = render_planes(&backend, &ctx, LayerId(12), size, &case2);
        assert_eq!(
            pixel_at(&pixels, size, 32, 32),
            blue,
            "近的全屏平面应遮住后绘制的远处中心块"
        );
        assert_eq!(pixel_at(&pixels, size, 4, 4), blue);
    }

    // ── RD §D2：着色变体可替换性（像素级断言，能证伪） ─────────────

    /// 同一几何切换 [`ShaderVariant`] 应产生不同像素 —— 证明"换风格 = 换数据"。
    ///
    /// 用法线恒为 `normalize(1, 0, 1)` 的四边形（**非** `flat_plane` 的恒定
    /// `(0, 0, 1)`）：此时 `dot(n, L) = 1/√2 < 1`，故 Lambert 的 `lit < 1`；而
    /// `Flat` 无光照（`lit = 1`）。两者像素必然不同 —— 若相同则变体机制失效。
    #[test]
    fn test_shader_variant_changes_pixels() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 64u32;

        // 法线倾斜的四边形：lit ≈ 0.2 + 0.8 / √2 ≈ 0.766 < 1。
        let n = glam::Vec3::new(1.0, 0.0, 1.0).normalize();
        let geometry = std::sync::Arc::new(
            MeshGeometry::new(
                vec![
                    glam::Vec3::new(-1.0, -1.0, 0.5),
                    glam::Vec3::new(1.0, -1.0, 0.5),
                    glam::Vec3::new(1.0, 1.0, 0.5),
                    glam::Vec3::new(-1.0, 1.0, 0.5),
                ],
                Some(vec![0, 1, 2, 0, 2, 3]),
                PrimitiveTopology::Triangles,
            )
            .with_uvs(vec![glam::Vec2::ZERO; 4])
            .with_normals(vec![n; 4]),
        );

        let mut flat_material = Material3D::color(Color32::WHITE);
        flat_material.variant = ShaderVariant::Flat;
        let mut lambert_material = Material3D::color(Color32::WHITE);
        lambert_material.variant = ShaderVariant::Lambert;

        let flat_pixels = render_planes(
            &backend,
            &ctx,
            LayerId(21),
            size,
            &[MeshDraw::new(
                std::sync::Arc::clone(&geometry),
                Mat4::IDENTITY,
                flat_material,
            )],
        );
        let lambert_pixels = render_planes(
            &backend,
            &ctx,
            LayerId(22),
            size,
            &[MeshDraw::new(
                std::sync::Arc::clone(&geometry),
                Mat4::IDENTITY,
                lambert_material,
            )],
        );

        // 目标格式 Bgra8UnormSrgb：纯白 = [255, 255, 255, 255]。
        let white = [255u8, 255, 255, 255];
        let center_flat = pixel_at(&flat_pixels, size, 32, 32);
        let center_lambert = pixel_at(&lambert_pixels, size, 32, 32);
        assert_eq!(center_flat, white, "Flat 变体应是无光照的纯白");
        assert_ne!(center_lambert, white, "Lambert 变体应因光照而变暗");
        assert_ne!(
            center_flat, center_lambert,
            "切换 ShaderVariant 必须改变输出像素"
        );
    }

    /// 生成一个朝向 +Z 的平面（[`parametric_surface`] 在 XY 平面，z 由变换平移）。
    ///
    /// `view_proj = IDENTITY` 时 `mvp = transform`，平移的 z 分量即 NDC 深度。
    fn flat_plane(half_w: f32, half_h: f32, z: f32, color: Color32) -> MeshDraw {
        let geometry = parametric_surface(
            SurfaceFn::new((-half_w, half_w), (-half_h, half_h), |_, _| 0.0),
            1,
            1,
        );
        MeshDraw::new(
            std::sync::Arc::new(geometry),
            Mat4::from_translation(glam::Vec3::new(0.0, 0.0, z)),
            Material3D::color(color),
        )
    }

    /// 以 `view_proj = IDENTITY` 把一组网格渲染到指定图层的离屏目标并回读像素。
    fn render_planes(
        backend: &WgpuBackend3D,
        ctx: &RenderContext,
        layer_id: LayerId,
        size: u32,
        draws: &[MeshDraw],
    ) -> Vec<u8> {
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test-3d-material-depth-encoder"),
            });
        let _frame = backend.render_layer_offscreen(
            &mut encoder,
            layer_id,
            Mat4::IDENTITY,
            Viewport::full(size, size),
            draws,
            &[],
        );
        ctx.submit(encoder.finish());
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll 失败");
        let texture = backend
            .offscreen_texture(layer_id)
            .expect("离屏纹理应已创建");
        read_rgba8(ctx.device(), ctx.queue(), &texture, size, size)
    }

    /// 取像素 `(x, y)` 的 4 字节（左上角为原点，Bgra8 字节序）。
    fn pixel_at(pixels: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        pixels[i..i + 4].try_into().expect("pixel bytes")
    }

    /// 以**给定相机**渲染一组网格到离屏目标并回读像素（K 线验收用）。
    fn render_with_camera(
        backend: &WgpuBackend3D,
        ctx: &RenderContext,
        layer_id: LayerId,
        size: u32,
        camera: &Camera3D,
        draws: &[MeshDraw],
    ) -> Vec<u8> {
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test-3d-camera-encoder"),
            });
        let _frame = backend.render_layer_offscreen(
            &mut encoder,
            layer_id,
            camera.view_projection(1.0),
            Viewport::full(size, size),
            draws,
            &[],
        );
        ctx.submit(encoder.finish());
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll 失败");
        let texture = backend
            .offscreen_texture(layer_id)
            .expect("离屏纹理应已创建");
        read_rgba8(ctx.device(), ctx.queue(), &texture, size, size)
    }

    /// 非透明像素数（画面覆盖面积的代理量）。
    fn coverage(pixels: &[u8]) -> usize {
        pixels.chunks_exact(4).filter(|p| p[3] > 0).count()
    }

    /// **K 线 GPU 像素级验收**（P 阶段教训：不得只断言组件字段变化）。
    ///
    /// 用**相机控制器产出的位姿**渲染同一场景，断言画面真的随之变化：
    /// 1. K1 `Fps` + `Look`：转 90° ⇒ 平面变侧视，覆盖塌缩；
    /// 2. K1 `Fps` + `Move`：靠近 ⇒ 平面变大，覆盖增加；
    /// 3. K3 `Orbit`：绕目标公转 90° ⇒ 画面改变（侧视：覆盖塌缩且像素不同）；
    /// 4. K2 轨迹：`Curve::Keyframes(Vec3)` 上的三个采样位姿 ⇒ 三帧互不相同。
    #[test]
    fn test_k_line_camera_controllers_change_pixels() {
        use xcal_scene::camera_input::{apply_fps, apply_orbit};
        use xcal_scene::domain::CameraCommand;
        use xcal_scene::{AnimValue, Curve, Interp};
        use xc_object::FpsView;

        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 64u32;
        // 场景：一张正对 +Z、半边长 0.5 的红色平面（位于 z = 0）。
        let draws = [flat_plane(0.5, 0.5, 0.0, Color32::RED)];
        let perspective = || Projection::perspective(60f32.to_radians(), 1.0, 0.1, 100.0);
        let base = Camera3D::new(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, Vec3::Y, perspective());

        let p_base = render_with_camera(&backend, &ctx, LayerId(20), size, &base, &draws);
        let c_base = coverage(&p_base);
        assert!(c_base > 0, "基线：相机应看到平面");

        // ── K1：Look 转 90° ⇒ 平面侧视 ──
        let mut cam = base;
        let mut fps = FpsView::from_camera(&cam);
        apply_fps(
            &mut fps,
            &mut cam,
            CameraCommand::Look {
                delta_yaw: std::f32::consts::FRAC_PI_2,
                delta_pitch: 0.0,
            },
        );
        let p_look = render_with_camera(&backend, &ctx, LayerId(21), size, &cam, &draws);
        assert!(
            coverage(&p_look) < c_base,
            "FPS 转头后平面应几乎不可见：base = {c_base}，look = {}",
            coverage(&p_look)
        );

        // ── K1：Move 前进 3 ⇒ 平面更大 ──
        let mut cam = base;
        let mut fps = FpsView::from_camera(&cam);
        apply_fps(
            &mut fps,
            &mut cam,
            CameraCommand::Move {
                delta: Vec3::new(0.0, 0.0, 3.0),
            },
        );
        let p_move = render_with_camera(&backend, &ctx, LayerId(22), size, &cam, &draws);
        assert!(
            coverage(&p_move) > c_base,
            "FPS 靠近后平面应更大：base = {c_base}，move = {}",
            coverage(&p_move)
        );

        // ── K3：Orbit 公转 90° ⇒ 画面改变 ──
        let mut cam = base;
        apply_orbit(
            &mut cam,
            CameraCommand::Orbit {
                delta_yaw: std::f32::consts::FRAC_PI_2,
                delta_pitch: 0.0,
            },
        );
        let p_orbit = render_with_camera(&backend, &ctx, LayerId(23), size, &cam, &draws);
        assert_ne!(p_orbit, p_base, "3D 轨道后画面必须不同");
        assert!(
            coverage(&p_orbit) < c_base,
            "公转 90° 后平面侧视，覆盖应塌缩：orbit = {}",
            coverage(&p_orbit)
        );

        // ── K2：轨迹（Keyframes(Vec3) 驱动 eye）⇒ 三帧互不相同 ──
        // 这正是 `CameraMode::Path` 的驱动方式（动画曲线给出 eye，交互被忽略）。
        let path = Curve::Keyframes {
            keys: vec![
                (0.0, AnimValue::Vec3(Vec3::new(0.0, 0.0, 5.0))),
                (1.0, AnimValue::Vec3(Vec3::new(4.0, 2.0, 3.0))),
            ],
            interpolation: Interp::Linear,
        };
        let mut frames: Vec<Vec<u8>> = Vec::new();
        for (i, t) in [0.0f64, 0.5, 1.0].into_iter().enumerate() {
            let cam = Camera3D::new(path.sample(t).vec3(), Vec3::ZERO, Vec3::Y, perspective());
            frames.push(render_with_camera(
                &backend,
                &ctx,
                LayerId(24 + i as u32),
                size,
                &cam,
                &draws,
            ));
        }
        assert_ne!(frames[0], frames[1], "轨迹 1/2 处画面应不同");
        assert_ne!(frames[1], frames[2], "轨迹终点画面应不同");
        assert_eq!(frames[0], p_base, "轨迹起点 = 初始位姿 ⇒ 画面应逐位一致");
    }
}
