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
// 计划倾向于"只接 3D 侧"，但接线需要两个前提。**B2-0a 已把两个前提补齐**：
//   1. **顶点布局契约声明** —— 3D pipeline 的顶点布局改为翻译 kernel 契约
//      [`ze_vertex::VertexLayout::standard_3d`]（`get_or_create_pipeline` 内），
//      不再硬编码；
//   2. **entity identity** —— `DrawQueue3D` 的 `MeshDraw` / `LineDraw` 携带
//      `entity: ObjectId`（逐对象录制用 `draw_mesh_for` / `draw_line_for` 填真实实体，
//      无来源绘制为 `ObjectId::DANGLING` 哨兵）。
// checker 的实际接线（`prepare_draws` / `record_prepared` 入口）仍留在后续可观测性
// 步骤（B2-6），本步只补契约与身份，不改变渲染行为。
// 2D 侧则相反：内容（形变 / 相机 / group）每帧都变，接上必然大量误报。

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::num::NonZeroU64;
use std::sync::Mutex;

use ze_notion::Mat4;
use wgpu::util::DeviceExt as _;

use crate::gpu_util::{UniformArena, bytes_of};
use ze_notion::Color32;
use zela_render::draw_queue_3d::{LineDraw, MeshDraw};
use zela_render::{DirectionalLightRecord, PointLightRecord, Viewport};
// B3-0.5e：GPU 侧契约模块已在本 crate（原 `zela_render::*`）。
use crate::render_backend::{GpuHandle, RenderBackend3D};
use crate::render_layer::RenderLayer;
use crate::render_target::RenderTarget;
use crate::resource_cache::{ResourceCache, ResourceKey};
use ze_render_ir::{LayerId, PostCaps, PrimitiveTopology, ShaderVariant, UvProjection};
use ze_render_ir::{
    Capabilities, CapabilityError, ColorFormat, CullMode, DepthFormat, LayerBlend, ZOrder,
};
use ze_vertex::AttribType;

/// 默认着色器（cube.wgsl）：uniform = mat4 mvp + mat4 model + vec4 color。
pub const DEFAULT_SHADER: &str = include_str!("../shaders/cube.wgsl");

/// uniform 块大小：mat4x4 mvp（64）+ mat4x4 model（64）+ vec4 color（16）
/// + vec4 params（16，PBR metallic/roughness）= 160 字节。
/// `model` 供片元取**世界位置**（B1-B 点光衰减用），顶点着色 `world_pos = model * pos`。
pub const UNIFORM_SIZE: u64 = 160;

/// `min_binding_size` 的常量形式：`UNIFORM_SIZE` 非零在编译期即可判定，
/// 避免调用点出现"不可能失败"的运行时 `unwrap()`。
const UNIFORM_MIN_BINDING_SIZE: NonZeroU64 = match NonZeroU64::new(UNIFORM_SIZE) {
    Some(v) => v,
    None => panic!("UNIFORM_SIZE must be non-zero"),
};

// ─── B1-A/B：场景级光源 uniform（binding 3，非 dynamic，所有 draw 共享一份）───

/// 场景级光源 uniform 块大小：`counts`(16) + 方向光 dirs(4×16=64)
/// + 方向光 colors(64) + 点光 pos/强度(4×16=64) + 点光 color/range(64)
/// + camera(16，B1-C PBR 视线向量用) = 288 字节。
/// 与 `cube.wgsl` 的 `Lights` 结构逐字段对应（全部 `vec4<f32>`，无对齐陷阱）。
pub const LIGHT_UNIFORM_SIZE: u64 = 288;

/// 方向光数量上限（**编译期常量**，超限显式报错 —— 铁律 2：不静默丢弃）。
pub const MAX_DIRECTIONAL_LIGHTS: usize = 4;

/// 点光数量上限（B1-B，**编译期常量**，超限显式报错 —— 铁律 2）。
pub const MAX_POINT_LIGHTS: usize = 4;

/// `LIGHT_UNIFORM_SIZE` 的常量形式（同 `UNIFORM_MIN_BINDING_SIZE`）。
const LIGHT_MIN_BINDING_SIZE: NonZeroU64 = match NonZeroU64::new(LIGHT_UNIFORM_SIZE) {
    Some(v) => v,
    None => panic!("LIGHT_UNIFORM_SIZE must be non-zero"),
};

/// 默认环境光占比（与 B1-A 之前的硬编码 `AMBIENT = 0.2` 逐位一致）。
const DEFAULT_AMBIENT: f32 = 0.2;

/// wgpu dynamic uniform offset 要求的最小对齐。
pub const DYNAMIC_OFFSET_ALIGNMENT: u64 = 256;

/// B2-0a：3D 顶点布局由 kernel 层契约声明
/// [`ze_vertex::VertexLayout::standard_3d`]（pos3@0 + uv2@12 + shading_normal3@20，stride 32），
/// 后端只做「契约 → wgpu 类型」翻译。原硬编码 `VERTEX_ATTRIBUTES` / `VERTEX_LAYOUT`
/// 已删除（铁律 4：产物字节格式逐位不变，只是声明来源换成契约）。
fn to_wgpu_format(ty: AttribType) -> wgpu::VertexFormat {
    match ty {
        AttribType::F32 => wgpu::VertexFormat::Float32,
        AttribType::F32x2 => wgpu::VertexFormat::Float32x2,
        AttribType::F32x3 => wgpu::VertexFormat::Float32x3,
        AttribType::F32x4 => wgpu::VertexFormat::Float32x4,
    }
}

/// 契约 → wgpu 顶点属性列表（shader location = 属性下标，单 buffer）。
///
/// 返回的 `Vec` 由调用点持有并在 `create_render_pipeline` 期间保持存活
/// （wgpu 在创建时拷贝属性数据）。
fn to_wgpu_vertex_attributes(layout: &ze_vertex::VertexLayout) -> Vec<wgpu::VertexAttribute> {
    layout
        .attribs
        .iter()
        .enumerate()
        .map(|(index, a)| wgpu::VertexAttribute {
            format: to_wgpu_format(a.ty),
            offset: u64::from(a.offset),
            shader_location: index as u32,
        })
        .collect()
}

/// 契约 → wgpu 步进模式。
fn to_wgpu_step_mode(step: ze_vertex::StepMode) -> wgpu::VertexStepMode {
    match step {
        ze_vertex::StepMode::Vertex => wgpu::VertexStepMode::Vertex,
        ze_vertex::StepMode::Instance => wgpu::VertexStepMode::Instance,
    }
}

/// 未指定纹理时使用的兜底纹理 id（1×1 透明黑）。
const DEFAULT_TEXTURE_ID: u64 = 0;

/// B2-4（D4）：`ZOrder` → 深度间距的换算系数（**唯一**的 epsilon 来源）。
///
/// 层序每 +1 ⇒ 沿平面法线向相机平移 `1e-4`（NDC 深度空间近似），足以消除
/// 共面 z-fighting，又小于单层几何厚度。**不引入常量 depth bias**。
pub const Z_ORDER_EPSILON: f32 = 1e-4;

/// B2-5：3D 默认 MSAA 采样数（与 2D 的 `MSAA_SAMPLES = 4` 对齐，Q5 决议）。
///
/// 这是**默认值**而非写死的命令：需要不同取值的调用方走
/// [`WgpuBackend3D::new_with_samples`]，并先用 [`WgpuBackend3D::capabilities`] 校验。
pub const DEFAULT_MSAA_SAMPLES: u32 = 4;

/// [`RenderBackend3D::submit_layer`] 没有 `LayerId` 参数，用保留 id 承载其
/// 颜色/深度目标的键（颜色目标来自 `layer.target.view`，仅深度目标需按 id 缓存）。
pub const SUBMIT_LAYER_ID: LayerId = LayerId(u32::MAX);

/// RC.3：3D 深度缓冲格式（投影矩阵为 D3D/wgpu 约定，深度范围 [0, 1]）。
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

// ─── D2：着色变体 → pipeline 表 ─────────────────────────────────────────────

/// 着色变体 → 片元入口名（`@fragment fn`）的映射。
///
/// wgpu 无"动态入口点"，故变体只能靠"同一 shader module 的多个 `fs_*` 入口 + 各自
/// 的 pipeline"实现。`ShaderVariant` 定义在数据层 zela-scene（不得引用 wgpu），
/// 因此该映射是后端侧的自由函数而非 `ShaderVariant` 的固有方法。
/// 该 match 是**穷尽**的 —— 新增变体漏写入口即编译失败（B1-E 起以此替代原
/// `SHADER_VARIANTS` 预建穷举测试：按需构建下不再有"预建表"可漏）。
fn variant_entry_point(variant: ShaderVariant) -> &'static str {
    match variant {
        ShaderVariant::Flat => "fs_flat",
        ShaderVariant::Lambert => "fs_lambert",
        ShaderVariant::Pbr => "fs_pbr",
    }
}

/// B2-0c（D22）：格式特性位 → 最大 MSAA 采样数（纯函数，可用合成 flags 单测）。
///
/// wgpu 30 的 `get_texture_format_features` 在 `Adapter` 上（不在 `Device`），
/// 故 `capabilities` 显式接收 adapter；本函数把位运算隔离出来，无需 GPU 即可验证。
fn max_sample_count_from_flags(flags: wgpu::TextureFormatFeatureFlags) -> u32 {
    let mut max = 1;
    if flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X2) {
        max = 2;
    }
    if flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X4) {
        max = 4;
    }
    if flags.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_X8) {
        max = 8;
    }
    max
}

/// B1-E/B2-2：pipeline **按需构建**的查表键。五个维度：
/// `(topology, variant, cull, blend, layout)`。
///
/// - `cull` / `blend` 用 **IR 类型**（`CullMode` / `LayerBlend`）—— `PipelineKey`
///   不得含任何 wgpu 类型（#1 的直接修正、D19）；
/// - `layout` 是第五维（D15）：队列统一后 2D / 3D 两套布局共存，布局不同的 draw
///   必须落到不同 pipeline（wgpu 的 `vertex.buffers` 在创建时固定）。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct PipelineKey {
    topology: PrimitiveTopology,
    variant: ShaderVariant,
    cull: CullMode,
    blend: LayerBlend,
    layout: ze_vertex::VertexLayout,
}

/// 意图层 `double_sided` → 后端剔除模式（B2-2）。
///
/// 三角形：封闭实体（`false`）剔除背面，双面（`true`）不剔除；
/// 线段/点没有面朝向，恒不剔除。
fn cull_mode_for(topology: PrimitiveTopology, double_sided: bool) -> CullMode {
    match topology {
        PrimitiveTopology::Triangles if !double_sided => CullMode::Back,
        PrimitiveTopology::Triangles => CullMode::None,
        PrimitiveTopology::Lines | PrimitiveTopology::Points => CullMode::None,
    }
}

/// IR `CullMode` → wgpu `Face`（后端翻译，B2-2）。
fn to_wgpu_cull_mode(cull: CullMode) -> Option<wgpu::Face> {
    match cull {
        CullMode::None => None,
        CullMode::Front => Some(wgpu::Face::Front),
        CullMode::Back => Some(wgpu::Face::Back),
    }
}

/// IR `LayerBlend` → wgpu `BlendState`（后端翻译，B2-2）。
///
/// - `Opaque` → `REPLACE`（与 B2-2 之前逐位一致）；
/// - `Alpha` → **预乘** alpha over（`cube.wgsl` 输出预乘色，见 RC.1 注释）；
/// - `Additive` → `src: One, dst: One`。
fn to_wgpu_blend(blend: LayerBlend) -> wgpu::BlendState {
    match blend {
        LayerBlend::Opaque => wgpu::BlendState::REPLACE,
        LayerBlend::Alpha => wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        LayerBlend::Additive => wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        },
    }
}

/// IR 图元拓扑 → wgpu 图元拓扑（IR 枚举在数据层，不引用 wgpu，转换在后端侧）。
fn to_wgpu_topology(topology: PrimitiveTopology) -> wgpu::PrimitiveTopology {
    match topology {
        PrimitiveTopology::Triangles => wgpu::PrimitiveTopology::TriangleList,
        PrimitiveTopology::Lines => wgpu::PrimitiveTopology::LineList,
        PrimitiveTopology::Points => wgpu::PrimitiveTopology::PointList,
    }
}

/// RC.3：一张离屏颜色目标配套的深度目标（尺寸变化时整体重建）。
struct DepthTarget {
    /// 持有纹理以延长生命周期（视图自身也引用纹理，此处显式保留）。
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

/// B2-5：一张图层的 **MSAA 颜色附件**（多采样），resolve 到单采样的离屏目标。
struct MsaaTarget {
    _texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
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
    /// B2-2：背面剔除（IR 类型，不再存 wgpu 类型）。
    pub cull: CullMode,
    /// B2-2：混合语义（IR `LayerBlend`，不再存 wgpu `BlendState`）。
    pub blend: LayerBlend,
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

/// 材质 bind group 缓存（key = (decal 纹理 id, 法线贴图 id)）。
///
/// bind group 绑定 uniform arena buffer（binding 0）+ decal 纹理视图（binding 1）
/// + sampler（binding 2）+ 法线贴图视图（binding 4），因此其有效性取决于两者：
/// `arena_generation` 记录创建时的 [`UniformArena::generation`]，不一致即整体
/// 丢弃重建；纹理注册时也整体清空（见 [`WgpuBackend3D::register_texture`]）。
#[derive(Default)]
struct BindGroupCache {
    arena_generation: u64,
    entries: HashMap<(u64, u64), wgpu::BindGroup>,
}

/// WGPU 3D 渲染后端（设备句柄 + 管线 + 绑定布局 + mesh 缓存 + uniform arena）。
///
/// 内部状态（cache / arena）用 `Mutex` 保护：prepare_draws / record_prepared
/// 是 `&self`，但需要修改缓存和 arena。
/// （`RenderBackend3D` 自 S1 起不再要求 `Send + Sync`；本类型的跨线程徽章见下方。）
pub struct WgpuBackend3D {
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// 管线目标格式（3D 离屏纹理沿用该格式，合成器据此判域）。
    format: wgpu::TextureFormat,
    /// B1-E：**按需构建**的渲染管线缓存：key = `(topology, variant, cull_mode, blending)`。
    ///
    /// 只有实际绘制过的 key 才建管线（不再预建笛卡尔积）。所有变体共用同一条
    /// `pipeline_layout` 与同一 bind group layout，故只需一张表。**已知限制**：
    /// 若将来某变体需要额外资源槽（粗糙度贴图 / 环境图），需引入第二套 bind
    /// group layout 与对应的 pipeline 族，本表不再够用。
    pipelines: Mutex<HashMap<PipelineKey, wgpu::RenderPipeline>>,
    /// 按需创建 pipeline 所需：同一 shader module（多个 `fs_*` 入口共用）。
    shader: wgpu::ShaderModule,
    /// 按需创建 pipeline 所需：共用 pipeline layout。
    pipeline_layout: wgpu::PipelineLayout,
    bind_group_layout: wgpu::BindGroupLayout,
    /// 材质采样器（binding 2）。
    sampler: wgpu::Sampler,
    /// 兜底纹理视图（1×1 透明黑，binding 1）：材质未指定纹理时使用。
    default_texture_view: wgpu::TextureView,
    /// B1-C：1×1 **平坦法线**兜底纹理视图（材质无法线贴图时绑定）。
    default_normal_view: wgpu::TextureView,
    /// B1-A：**场景级**光源 uniform buffer（binding 3，非 dynamic）。
    /// 所有 draw 共享一份；每帧 `prepare_draws` 按 [`LightQueue`] 覆写内容。
    light_buffer: wgpu::Buffer,
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
    /// B2-5：MSAA 采样数（**能力**，非写死命令，D22）。
    ///
    /// 构造时由调用方给出（有能力查询的一侧用 [`Self::capabilities`] 的
    /// `max_sample_count` 校验后传入）；`with_default_shader` 取 4 与 2D 对齐。
    sample_count: u32,
    /// B2-5：按 [`LayerId`] 持有的 MSAA 颜色附件（多采样 → resolve 到离屏单采样）。
    msaa_color: Mutex<HashMap<LayerId, MsaaTarget>>,
}

// S1 徽章：egui 的 `CallbackResources` 在 native 上是
// `type_map::concurrent::TypeMap`，其 `insert` 要求 `Send + Sync`。
// 本项目渲染全程单线程、主线程唯一 —— 这个边界不是真实的跨线程传递，
// 只是 egui 的类型要求。native 的 wgpu 资源本身是 `Send + Sync`，
// 本结构内的 `GpuHandle` 只承载 wgpu 资源（全部由本 crate 的 `GpuHandle::new` 创建），
// 因此这里显式声明徽章，而不把 `Send + Sync` 写回 `GpuHandle`：
// wasm 上 wgpu 资源含 `Rc<RefCell<…>>`，全局约束会让 wasm 直接编译失败。
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Send for WgpuBackend3D {}
#[cfg(not(target_arch = "wasm32"))]
unsafe impl Sync for WgpuBackend3D {}

impl WgpuBackend3D {
    /// 用给定 WGSL 源码构造后端。pipeline **不预建**（B1-E：按需构建 + 缓存，
    /// 见 [`Self::get_or_create_pipeline`] —— 构造后缓存为空，首次绘制才建）。
    // 体检第 9 项（观察，暂不改）：本函数约 210 行，可按
    // 「bind group layout → 管线表 → 默认纹理」拆分。
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        shader_source: &str,
    ) -> Self {
        Self::new_with_samples(device, queue, format, shader_source, DEFAULT_MSAA_SAMPLES)
    }

    /// B2-5：显式给定 MSAA 采样数的构造（**能力驱动**，D22）。
    ///
    /// 调用方应先用 [`Self::capabilities`] 查询 `max_sample_count`，再用
    /// `Capabilities::require_sample_count` 校验后传入；不支持的取值 ⇒ **显式 panic**
    /// （铁律 2，不静默降级）。支持 1（关闭）与 4（与 2D 对齐）。
    pub fn new_with_samples(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        format: wgpu::TextureFormat,
        shader_source: &str,
        sample_count: u32,
    ) -> Self {
        assert!(
            matches!(sample_count, 1 | 4),
            "不静默：3D 后端仅支持 MSAA 1x / 4x（请求 {sample_count}x）；\
             请先经 capabilities().require_sample_count 校验"
        );
        let device = device.clone();
        let queue = queue.clone();

        // RC.1: binding 0 = uniform with dynamic offset；binding 1 = 材质纹理；
        // binding 2 = 采样器。
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("zela-3d-bind-group-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: Some(UNIFORM_MIN_BINDING_SIZE),
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
                // B1-A：binding 3 = **场景级**光源 uniform（非 dynamic，所有 draw 共享）。
                // 光源数据每帧 write_buffer 更新，buffer 本身不重建 ⇒ 材质 bind group
                // 无需因灯光变化而失效。
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: Some(LIGHT_MIN_BINDING_SIZE),
                    },
                    count: None,
                },
                // B1-C：binding 4 = **法线贴图**纹理（PBR 变体用；其他变体不采样）。
                // 缺省绑定 1×1 平坦法线兜底纹理（`nts = (0,0,1)`）。
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("zela-3d-pipeline-layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("zela-3d-shader"),
            source: wgpu::ShaderSource::Wgsl(shader_source.into()),
        });

        // B1-E：pipeline **不在此预建** —— 顶点布局在 `get_or_create_pipeline`
        // 内由 kernel 契约 [`ze_vertex::VertexLayout::standard_3d`] 现场翻译（B2-0a），
        // pipeline 按需构建 + 缓存（key 四维定齐）。
        // 原 `(topology, variant)` 笛卡尔积预建已删除：pipeline 数不再随 variant
        // 数爆炸（结构判据）。

        // RC.1: 1×1 **透明黑**兜底纹理 —— 材质无纹理时绑定它。着色式为预乘 over
        // （`tex + color * (1 - tex.a)`），`tex = (0,0,0,0)` 时退化为 `color`。
        let default_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("zela-3d-default-texture"),
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

        // B1-C：1×1 **平坦法线**兜底纹理（128,128,255,255）—— 材质无法线贴图时
        // 绑定它：解码 `nts = (0,0,1)`，TBN 变换后 ≈ 几何法线，PBR 退化为
        // "无法线贴图的金属-粗糙度"（Lambert/Flat 变体不采样该绑定）。
        // **必须用 unorm（非 sRGB）**：sRGB 纹理采样时硬件会自动解码到线性域，
        // `(128,128,255)` 会变成 ≈ (0.216,0.216,1)，法线退化为斜向，破坏"缺省
        // 法线 = 恒等"的约定。用户注册的法线贴图同理应使用非 sRGB 格式。
        let default_normal_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("zela-3d-default-normal-texture"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &default_normal_texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &[128u8, 128, 255, 255],
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
        let default_normal_view =
            default_normal_texture.create_view(&wgpu::TextureViewDescriptor::default());

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("zela-3d-material-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        // B1-A/B/C：场景级光源 uniform buffer（binding 3）。初始写默认光照
        // （单方向光 (0,0,1) + ambient 0.2 + 相机原点占位）—— 与 B1-A 之前的
        // 硬编码逐位一致；此后每帧 `prepare_draws` 按 LightQueue 覆写（含相机位置）。
        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("zela-3d-light-uniform"),
            size: LIGHT_UNIFORM_SIZE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&light_buffer, 0, &default_light_bytes(ze_notion::Vec3::ZERO));

        Self {
            device,
            queue,
            format,
            pipelines: Mutex::new(HashMap::new()),
            shader,
            pipeline_layout,
            bind_group_layout,
            sampler,
            default_texture_view,
            default_normal_view,
            light_buffer,
            textures: Mutex::new(HashMap::new()),
            bind_groups: Mutex::new(BindGroupCache::default()),
            mesh_cache: Mutex::new(ResourceCache::new()),
            arena: Mutex::new(UniformArena::new(UNIFORM_SIZE, DYNAMIC_OFFSET_ALIGNMENT)),
            offscreen: Mutex::new(HashMap::new()),
            depth: Mutex::new(HashMap::new()),
            sample_count,
            msaa_color: Mutex::new(HashMap::new()),
        }
    }

    /// B2-5：当前 MSAA 采样数（1 = 关闭，4 = 与 2D 对齐）。
    pub fn sample_count(&self) -> u32 {
        self.sample_count
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

    /// B1-C：解析**法线贴图**纹理视图：未注册的 id 回退到 1×1 平坦法线兜底纹理。
    fn resolve_normal_view(&self, texture_id: u64) -> wgpu::TextureView {
        let guard = self.textures.lock().expect("textures poisoned");
        match guard.get(&texture_id) {
            Some(view) => view.clone(),
            None => self.default_normal_view.clone(),
        }
    }

    /// 创建材质 bind group：uniform（dynamic）+ decal 纹理视图 + sampler
    /// + 法线贴图视图（B1-C，binding 4）。
    fn create_material_bind_group(
        &self,
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
        view: &wgpu::TextureView,
        normal_view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("zela-3d-material-bind-group"),
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
                // B1-A：场景级光源 uniform（所有 draw 共享同一份；非 dynamic）。
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.light_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(LIGHT_UNIFORM_SIZE),
                    }),
                },
                // B1-C：法线贴图（PBR 变体采样；缺省 = 平坦法线兜底纹理）。
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(normal_view),
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

    /// B2-5：取得（必要时按尺寸重建）指定图层的 **MSAA 颜色附件**视图。
    ///
    /// 多采样纹理只作 render pass 的 color attachment，由 `resolve_target`
    /// 解析到单采样的离屏目标（[`Self::offscreen_view`]）。`sample_count == 1` 时
    /// 调用方不得使用本函数（走单采样直写路径）。
    fn msaa_color_view(
        &self,
        device: &wgpu::Device,
        layer_id: LayerId,
        width: u32,
        height: u32,
    ) -> wgpu::TextureView {
        let width = width.max(1);
        let height = height.max(1);
        let mut guard = self.msaa_color.lock().expect("msaa poisoned");
        let recreate = match guard.get(&layer_id) {
            Some(t) => t.size != (width, height),
            None => true,
        };
        if recreate {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("zela-3d-msaa-color"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: self.sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            // B12 Step 5：资源计数（**创建**次数；稳态逐帧增量应为 0）。
            crate::diagnostics::record_textures_created(1);
            guard.insert(
                layer_id,
                MsaaTarget {
                    _texture: texture,
                    view,
                    size: (width, height),
                },
            );
        }
        guard.get(&layer_id).expect("msaa target").view.clone()
    }

    /// RC.3：取得（必要时按给定尺寸重建）指定图层的深度目标视图。
    ///
    /// 深度目标与颜色目标同尺寸、同 [`LayerId`] 键，尺寸变化时一并重建。
    fn depth_view(
        &self,
        device: &wgpu::Device,
        layer_id: LayerId,
        width: u32,
        height: u32,
    ) -> wgpu::TextureView {
        let width = width.max(1);
        let height = height.max(1);
        let mut guard = self.depth.lock().expect("depth poisoned");
        let recreate = match guard.get(&layer_id) {
            Some(t) => t.size != (width, height),
            None => true,
        };
        if recreate {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("zela-3d-depth-texture"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                // B2-5：深度附件必须与颜色附件同采样数。
                sample_count: self.sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            // B12 Step 5：资源计数（**创建**次数；稳态逐帧增量应为 0）。
            crate::diagnostics::record_textures_created(1);
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

    /// 写入 uniform 块（mvp 64 + model 64 + color 16 + params 16 = 160 字节）。
    pub fn write_uniforms(
        queue: &wgpu::Queue,
        buffer: &wgpu::Buffer,
        mvp: Mat4,
        model: Mat4,
        color: Color32,
        params: [f32; 4],
    ) {
        queue.write_buffer(buffer, 0, &uniform_bytes(mvp, model, color, params));
    }

    /// 推进 mesh_cache 的「延迟销毁」提交帧计数器（destroyer.on_submit）。
    ///
    /// 每次把 command encoder 提交到 GPU queue 之后调用一次（每帧一次或多次）。
    pub fn notify_submit(&self) {
        self.mesh_cache
            .lock()
            .expect("mesh_cache poisoned")
            .notify_submit();
    }

    // ── prepare_draws：G6 核心 ────────────────────────────────────────

    /// 上传一帧的全部绘制资源。
    ///
    /// - mesh/line 顶点缓冲：按内容哈希，命中则复用，miss 才 create_buffer_init 一次；
    /// - uniform：**全部**走 `UniformArena.push`，flush 时一次性 write_buffer；
    /// - bind_group：每帧最多创建一次（arena 未扩容时复用 bind_group）。
    // 体检第 9 项（观察，暂不改）：本函数约 240 行，可按
    // 「mesh 缓存 → line 缓存 → uniform 装配 → bind group 缓存」拆分。
    pub fn prepare_draws<'a, M, L>(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        view_proj: Mat4,
        camera_pos: ze_notion::Vec3,
        meshes: M,
        lines: L,
        lights: &[DirectionalLightRecord],
        point_lights: &[PointLightRecord],
    ) -> Frame3DData
    where
        M: IntoIterator<Item = &'a MeshDraw>,
        L: IntoIterator<Item = &'a LineDraw>,
    {
        // B2-1：统一队列的消费方按载荷过滤后以迭代器传入；此处收敛为切片式视图。
        let meshes: Vec<&MeshDraw> = meshes.into_iter().collect();
        let lines: Vec<&LineDraw> = lines.into_iter().collect();
        // B1-A/B/C：每帧覆写场景级光源 uniform。**空灯光 = 默认光照**（单方向光
        // (0,0,1) + ambient 0.2，与硬编码时代逐位一致）；超限**显式报错**
        // （铁律 2：不静默丢弃）。`camera_pos` 一并写入（PBR 视线向量用）。
        let bytes = if lights.is_empty() && point_lights.is_empty() {
            default_light_bytes(camera_pos)
        } else {
            assert!(
                lights.len() <= MAX_DIRECTIONAL_LIGHTS,
                "方向光数量 {} 超过上限 {MAX_DIRECTIONAL_LIGHTS}（不静默丢弃，请调整场景光源数）",
                lights.len(),
            );
            assert!(
                point_lights.len() <= MAX_POINT_LIGHTS,
                "点光数量 {} 超过上限 {MAX_POINT_LIGHTS}（不静默丢弃，请调整场景光源数）",
                point_lights.len(),
            );
            pack_light_bytes(lights, point_lights, DEFAULT_AMBIENT, camera_pos)
        };
        queue.write_buffer(&self.light_buffer, 0, &bytes);

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
            /// 着色变体（pipeline 查表键之一）。
            variant: ShaderVariant,
            /// B2-2：剔除模式（pipeline 查表键）。
            cull: CullMode,
            /// B2-2：混合语义（pipeline 查表键）。
            blend: LayerBlend,
            /// B2-3：统一层序（透明桶排序键）。
            z: ZOrder,
            uniform_offset: u32,
            /// 材质纹理 id（bind group 的缓存键）。
            texture_id: u64,
            /// B1-C：法线贴图纹理 id（与 `texture_id` 组成缓存键）。
            normal_texture_id: u64,
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
                // B3-0b：顶点生产（UV 兜底 / 法线补齐 / 交错打包）在渲染无关层，
                // 后端只收字节并上传。失败 ⇒ **显式 panic**（不静默产空几何）。
                let data = zela_render::vertex_record::mesh_vertex_buffer(
                    geometry,
                    md.material.uv_projection,
                )
                .unwrap_or_else(|error| {
                    panic!("不静默：3D 顶点生产失败（几何 id={}）：{error}", geometry.id())
                });
                let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("zela-3d-mesh-vertices"),
                    contents: &data.vertices,
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let ib = (!data.indices.is_empty()).then(|| {
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("zela-3d-mesh-indices"),
                        contents: bytes_of(&data.indices),
                        usage: wgpu::BufferUsages::INDEX,
                    })
                });
                let cached = CachedMeshBuffers {
                    vertex_buffer: vb,
                    index_buffer: ib,
                    vertex_count: data.vertex_count,
                    index_count: data.indices.len() as u32,
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

            // B2-4（D4/R-C）：epsilon 由统一 `ZOrder` 派生，施加为 **normal offset**
            // （沿法线 +Z，即朝向相机侧），**不用常量 depth bias**。
            //
            // R-D 显式决策：没有 `shading_normal` 的几何在写出端以 (0,0,1) 补齐
            // ⇒ normal offset 沿 +Z，**非零**（不会静默退化为零偏移）；平面/斜面
            // 上都成立。`z == 0`（默认层）⇒ 偏移为 0，逐位等于 B2-4 之前的行为。
            let epsilon = md.z.0 as f32 * Z_ORDER_EPSILON;
            let transform = if epsilon == 0.0 {
                md.transform
            } else {
                Mat4::from_translation(ze_notion::Vec3::new(0.0, 0.0, epsilon)) * md.transform
            };

            let mvp = view_proj * transform;
            // B1-B：`model` 单独入 uniform，片元据此算世界位置（点光衰减用）。
            // B1-C：`params` = (metallic, roughness, 0, 0)（PBR 变体消费；其余变体忽略）。
            let params = [md.material.metallic, md.material.roughness, 0.0, 0.0];
            let bytes = uniform_bytes(mvp, transform, md.material.color, params);
            let block_index = arena_guard.push(&bytes);
            let offset_bytes = block_index as u32 * arena_guard.stride() as u32;

            skeletons.push(Skeleton {
                vertex_buffer: cached_copy.vertex_buffer,
                index_buffer: cached_copy.index_buffer,
                vertex_count: cached_copy.vertex_count,
                index_count: cached_copy.index_count,
                topology: cached_copy.topology,
                variant: md.material.variant,
                // B2-2：意图层 → 状态（材质未声明双面 ⇒ 剔除背面，与旧行为一致）。
                cull: cull_mode_for(cached_copy.topology, md.material.double_sided),
                blend: if md.material.transparent {
                    LayerBlend::Alpha
                } else {
                    LayerBlend::Opaque
                },
                z: md.z,
                uniform_offset: offset_bytes,
                texture_id: md
                    .material
                    .texture
                    .map(|t| t.id())
                    .unwrap_or(DEFAULT_TEXTURE_ID),
                normal_texture_id: md
                    .material
                    .normal_map
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
                // B3-0b：line 顶点也走生产端（B7：内联是第二份真源）。
                let data = zela_render::vertex_record::line_vertex_buffer(line.start, line.end)
                    .unwrap_or_else(|error| {
                        panic!("不静默：3D 线段顶点生产失败：{error}")
                    });
                let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("zela-3d-line-vertices"),
                    contents: &data.vertices,
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

            // line 顶点已是世界空间坐标 ⇒ `model = IDENTITY`，`world_pos = pos`。
            // B1-C：params 恒为 0（line 固定走 Lambert，不消费）。
            let bytes = uniform_bytes(view_proj, Mat4::IDENTITY, line.color, [0.0; 4]);
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
                // B2-2：线段没有面朝向 ⇒ 恒不剔除；不透明混合（与旧行为一致）。
                cull: CullMode::None,
                blend: LayerBlend::Opaque,
                z: line.z,
                uniform_offset: offset_bytes,
                // line 无材质纹理 → 绑定 1×1 **透明黑**兜底纹理（`DEFAULT_TEXTURE_ID = 0`
                // 未注册，`resolve_texture_view` 回退），着色式退化为 `color * lit`。
                texture_id: DEFAULT_TEXTURE_ID,
                // B1-C：line 无法线贴图 → 平坦法线兜底纹理。
                normal_texture_id: DEFAULT_TEXTURE_ID,
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
        // ── B2-3：透明桶 ─────────────────────────────────────────────
        // 不透明先画（深度测试定遮挡）；透明最后画，按统一 `ZOrder` **升序**
        // （低层在前、高层后叠加 —— 与 2D 画家算法同一顺序，稳定排序）。
        // 透明物体由 pipeline 的 Alpha blend 混合，深度写入仍开（B2-3 范围内不变）。
        {
            let mut opaque = Vec::new();
            let mut transparent = Vec::new();
            for s in skeletons {
                if s.blend == LayerBlend::Opaque {
                    opaque.push(s);
                } else {
                    transparent.push(s);
                }
            }
            transparent.sort_by_key(|s| s.z);
            opaque.extend(transparent);
            skeletons = opaque;
        }

        let mut bg_guard = self.bind_groups.lock().expect("bind_groups poisoned");
        if bg_guard.arena_generation != arena_generation {
            bg_guard.entries.clear();
            bg_guard.arena_generation = arena_generation;
        }
        let draws_final: Vec<PreparedDraw3D> = skeletons
            .into_iter()
            .map(|s| {
                let bg_key = (s.texture_id, s.normal_texture_id);
                let bind_group = match bg_guard.entries.get(&bg_key) {
                    Some(bg) => bg.clone(),
                    None => {
                        let view = self.resolve_texture_view(s.texture_id);
                        let nv = self.resolve_normal_view(s.normal_texture_id);
                        let bg = self.create_material_bind_group(device, &arena_buf_owned, &view, &nv);
                        bg_guard.entries.insert(bg_key, bg.clone());
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
                    cull: s.cull,
                    blend: s.blend,
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
            // B1-F（不静默）：点列表在 3D 路径不受支持 —— **显式报错**，不得静默丢弃。
            // 现状：全仓没有点列表生产者（`cube` / `parametric_surface` 只产三角形与
            // 线段），一旦触发即说明新增了后端未支持的图元类型，必须当面暴露。
            if d.topology == PrimitiveTopology::Points {
                panic!(
                    "不静默：3D 后端不支持点列表拓扑（PrimitiveTopology::Points），拒绝绘制而非静默丢弃"
                );
            }
            // B1-E/B2-2：按需构建 + 缓存（key 五维：topology / variant / cull / blend / layout）。
            // 每个 draw 必然命中（未命中即创建）—— 原「回退 Lambert」路径已结构性消失。
            let key = PipelineKey {
                topology: d.topology,
                variant: d.variant,
                cull: d.cull,
                blend: d.blend,
                // D15：3D 顶点布局是第五维（当前全部 3D draw 共用 standard_3d）。
                layout: ze_vertex::VertexLayout::standard_3d(),
            };
            let pipeline = self.get_or_create_pipeline(key);
            rpass.set_pipeline(&pipeline);
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

    /// B1-E/B2-2：按需构建 + 缓存 pipeline（key 五维：topology / variant / cull / blend / layout）。
    ///
    /// 命中直接返回缓存克隆；未命中则创建并缓存。**无回退路径** —— 与预建时代
    /// 「查表落空回退 Lambert」不同，这里不存在"漏建"状态（不静默，铁律 2）。
    /// wgpu 30 的 `create_render_pipeline` 不可失败（内部校验失败即 panic，且携带
    /// key 化的 label），故创建路径本身就是显式报错。
    fn get_or_create_pipeline(&self, key: PipelineKey) -> wgpu::RenderPipeline {
        let mut cache = self.pipelines.lock().expect("pipelines poisoned");
        if let Some(p) = cache.get(&key) {
            return p.clone();
        }
        let label = format!("zela-3d-pipeline-{key:?}");
        // B2-0a：顶点布局来自 kernel 契约；B2-2：取自 key 的第五维（D15）。
        let layout_desc = &key.layout;
        let attributes = to_wgpu_vertex_attributes(layout_desc);
        let vertex_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: u64::from(layout_desc.stride),
            step_mode: to_wgpu_step_mode(layout_desc.step_mode),
            attributes: &attributes,
        })];
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(&label),
                layout: Some(&self.pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &self.shader,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &vertex_buffers,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &self.shader,
                    entry_point: Some(variant_entry_point(key.variant)),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: self.format,
                        // B2-2：IR `LayerBlend` → wgpu `BlendState`（唯一的翻译点）。
                        blend: Some(to_wgpu_blend(key.blend)),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState {
                    topology: to_wgpu_topology(key.topology),
                    // B2-2：IR `CullMode` → wgpu `Face`（唯一的翻译点）。
                    cull_mode: to_wgpu_cull_mode(key.cull),
                    ..Default::default()
                },
                // RC.3：深度测试（Less：近处像素深度更小者胜出）。
                // B2-3（R-A）：**透明桶不写深度** —— 否则同深度的透明面会在
                // 后画者上被 `Less` 判掉，混合顺序无法由 ZOrder 控制。
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(key.blend == LayerBlend::Opaque),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    // B2-5（Q5）：3D MSAA 与 2D 对齐（默认 4x）；取值来自能力侧。
                    count: self.sample_count,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            });
        cache.insert(key, pipeline.clone());
        pipeline
    }

    /// B1-E 结构判据：当前已按需构建的 pipeline 数（= 实际绘制过的 key 数，
    /// 而非 `variant × topology` 笛卡尔积）。验证"pipeline 数不随 variant 数爆炸"。
    pub fn pipeline_count(&self) -> usize {
        self.pipelines.lock().expect("pipelines poisoned").len()
    }

    /// B2-0c（D22）：**能力查询** —— target 决定的最大 MSAA 采样数与格式。
    ///
    /// `sample_count` / 颜色格式 / 深度格式是"能力"不是"命令"：消费者（B2-5 的
    /// AA 统一）先查后请求；不支持 ⇒ [`CapabilityError`]（铁律 2，不静默降级）。
    ///
    /// 需要 `adapter`：wgpu 30 的 `get_texture_format_features` 是 `Adapter` 方法
    /// （`Device` 上没有）。调用方必须持有 adapter（B2-5 接线时补进装配层）。
    pub fn capabilities(
        &self,
        adapter: &wgpu::Adapter,
    ) -> Result<Capabilities, CapabilityError> {
        let color_format = match self.format {
            wgpu::TextureFormat::Rgba8UnormSrgb => ColorFormat::Rgba8UnormSrgb,
            wgpu::TextureFormat::Rgba8Unorm => ColorFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8UnormSrgb => ColorFormat::Bgra8UnormSrgb,
            wgpu::TextureFormat::Bgra8Unorm => ColorFormat::Bgra8Unorm,
            _ => return Err(CapabilityError::UnsupportedColorFormat),
        };
        let flags = adapter.get_texture_format_features(self.format).flags;
        Ok(Capabilities {
            max_sample_count: max_sample_count_from_flags(flags),
            color_format,
            depth_format: DepthFormat::Depth32Float,
            // wgpu 三角化后端没有后处理栈 / IBL：显式声明不支持（B4 Step 1）。
            post: PostCaps::none(),
        })
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
        camera_pos: ze_notion::Vec3,
        viewport: Viewport,
        meshes: &[MeshDraw],
        lines: &[LineDraw],
        lights: &[DirectionalLightRecord],
        point_lights: &[PointLightRecord],
    ) -> Layer3DFrame {
        let frame = self.prepare_draws(
            &self.device,
            &self.queue,
            view_proj,
            camera_pos,
            meshes,
            lines,
            lights,
            point_lights,
        );
        let offscreen = self.offscreen_view(&self.device, layer_id, viewport);
        // RC.3：本层深度目标（与离屏颜色目标同尺寸），清为最远（1.0）。
        let depth_view = self.depth_view(&self.device, layer_id, viewport.w, viewport.h);
        // B2-5：MSAA 附件（多采样）→ resolve 到单采样离屏目标；1x 时直写。
        let color_view = if self.sample_count > 1 {
            self.msaa_color_view(&self.device, layer_id, viewport.w, viewport.h)
        } else {
            offscreen.clone()
        };
        let resolve_target = if self.sample_count > 1 {
            Some(&offscreen)
        } else {
            None
        };

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("zela-3d-layer-offscreen-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    depth_slice: None,
                    resolve_target,
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
    fn submit_layer(
        &self,
        layer: &RenderLayer,
        encoder: &mut wgpu::CommandEncoder,
        lights: &[DirectionalLightRecord],
        point_lights: &[PointLightRecord],
    ) {
        let view_proj = layer.camera.view_projection(layer.viewport.aspect());
        let frame = self.prepare_draws(
            &self.device,
            &self.queue,
            view_proj,
            layer.camera.eye,
            layer.draw_queue.meshes(),
            layer.draw_queue.lines(),
            lights,
            point_lights,
        );

        // RC.3：本层深度目标（尺寸取自颜色目标），清为最远（1.0）。
        let depth_view = self.depth_view(
            &self.device,
            SUBMIT_LAYER_ID,
            layer.target.size.width,
            layer.target.size.height,
        );
        // B2-5：MSAA 附件 → resolve 到宿主的单采样目标；1x 时直写。
        let color_view = if self.sample_count > 1 {
            self.msaa_color_view(
                &self.device,
                SUBMIT_LAYER_ID,
                layer.target.size.width,
                layer.target.size.height,
            )
        } else {
            layer.target.view.clone()
        };
        let resolve_target = if self.sample_count > 1 {
            Some(&layer.target.view)
        } else {
            None
        };

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("zela-3d-layer-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                depth_slice: None,
                resolve_target,
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

/// 组装 uniform 字节：mat4 mvp（64，列主序）+ mat4 model（64）+ vec4 color（16）
/// + vec4 params（16，`x = metallic, y = roughness`，B1-C PBR）。
fn uniform_bytes(mvp: Mat4, model: Mat4, color: Color32, params: [f32; 4]) -> [u8; UNIFORM_SIZE as usize] {
    let mut data = [0u8; UNIFORM_SIZE as usize];
    let m = mvp.to_cols_array();
    data[..64].copy_from_slice(bytes_of(&m));
    let model_cols = model.to_cols_array();
    data[64..128].copy_from_slice(bytes_of(&model_cols));
    let c = [
        color.r() as f32 / 255.0,
        color.g() as f32 / 255.0,
        color.b() as f32 / 255.0,
        color.a() as f32 / 255.0,
    ];
    data[128..144].copy_from_slice(bytes_of(&c));
    data[144..160].copy_from_slice(bytes_of(&params));
    data
}

/// 组装**场景级**光源 uniform 字节，与 `cube.wgsl` 的 `Lights` 结构逐字段对应
/// （全部 `vec4<f32>`，无对齐陷阱）。
///
/// 布局（下标单位 = f32；共 `LIGHT_UNIFORM_SIZE / 4 = 72` 个 f32）：
/// - `[0..4]`   = counts（x = 方向光数，y = ambient，z = 点光数，w = 0）
/// - `[4..20]`  = dirs：每方向光 `vec4(xyz = 光方向[世界空间], w = 强度)`
/// - `[20..36]` = dir_colors：每方向光 `vec4(rgba)`
/// - `[36..52]` = points：每点光 `vec4(xyz = 位置[世界空间], w = 强度)`
/// - `[52..68]` = point_colors：每点光 `vec4(rgb, w = range)`
/// - `[68..72]` = camera：`vec4(xyz = 相机位置[世界空间], w = 0)`（B1-C，PBR 视线向量）
fn pack_light_bytes(
    lights: &[DirectionalLightRecord],
    point_lights: &[PointLightRecord],
    ambient: f32,
    camera: ze_notion::Vec3,
) -> [u8; LIGHT_UNIFORM_SIZE as usize] {
    let mut words = [0f32; (LIGHT_UNIFORM_SIZE / 4) as usize];
    words[0] = lights.len() as f32;
    words[1] = ambient;
    words[2] = point_lights.len() as f32;
    for (i, l) in lights.iter().take(MAX_DIRECTIONAL_LIGHTS).enumerate() {
        let d = &l.direction;
        words[4 + i * 4] = d.x;
        words[5 + i * 4] = d.y;
        words[6 + i * 4] = d.z;
        words[7 + i * 4] = l.intensity;
        words[20 + i * 4] = l.color[0];
        words[21 + i * 4] = l.color[1];
        words[22 + i * 4] = l.color[2];
        words[23 + i * 4] = l.color[3];
    }
    for (i, p) in point_lights.iter().take(MAX_POINT_LIGHTS).enumerate() {
        let pos = &p.position;
        words[36 + i * 4] = pos.x;
        words[37 + i * 4] = pos.y;
        words[38 + i * 4] = pos.z;
        words[39 + i * 4] = p.intensity;
        words[52 + i * 4] = p.color[0];
        words[53 + i * 4] = p.color[1];
        words[54 + i * 4] = p.color[2];
        words[55 + i * 4] = p.range;
    }
    words[68] = camera.x;
    words[69] = camera.y;
    words[70] = camera.z;
    let mut data = [0u8; LIGHT_UNIFORM_SIZE as usize];
    data.copy_from_slice(bytes_of(words.as_slice()));
    data
}

/// 默认光照字节：单方向光 `(0,0,1)` 纯白强度 1 + ambient 0.2、零点光 ——
/// 与 B1-A 之前的硬编码 `LIGHT_DIR=(0,0,1)` / `AMBIENT=0.2` **逐位一致**。
/// `camera` 只被 PBR 视线向量消费（默认场景无 PBR 材质时不影响任何像素）。
fn default_light_bytes(camera: ze_notion::Vec3) -> [u8; LIGHT_UNIFORM_SIZE as usize] {
    pack_light_bytes(
        &[DirectionalLightRecord::new(ze_notion::Vec3::Z, Color32::WHITE, 1.0)],
        &[],
        DEFAULT_AMBIENT,
        camera,
    )
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

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_context::RenderContext;
    use ze_notion::{SurfaceFn, Vec3};
    use ze_scene_core::{Camera3D, Projection};
    use zela_render::{DrawQueue3D, Viewport};
    use zela_scene_ecs::scene::components::{Material3D, MeshGeometry, TextureHandle};
    use zela_scene_ecs::scene::primitives::{parametric_surface, wireframe_cube};

    /// 3D 后端的 headless 夹具：**守卫必须活到测试结束**（调用点绑定 `_gpu`）。
    #[track_caller]
    fn headless_backend() -> Option<(crate::gpu_test::GpuGuard, WgpuBackend3D, RenderContext)> {
        let (gpu, ctx) = crate::gpu_test::require_gpu()?;
        let backend =
            WgpuBackend3D::with_default_shader(ctx.device(), ctx.queue(), ctx.target_format());
        Some((gpu, backend, ctx))
    }

    // ── B1-A：光源 uniform 打包（纯 CPU，无需 GPU） ─────────────────────

    /// B2-0a：3D 顶点布局契约 → wgpu 翻译产物与旧硬编码布局**逐位相同**
    /// （铁律 4：只换声明来源，不改顶点格式；因此像素不变）。
    #[test]
    fn test_vertex_layout_translation_matches_legacy_format() {
        let layout = ze_vertex::VertexLayout::standard_3d();
        assert_eq!(layout.stride, 32, "旧 VERTEX_STRIDE = 32");
        let attrs = to_wgpu_vertex_attributes(&layout);
        assert_eq!(attrs.len(), 3);

        // position: Float32x3 @0 → location 0
        assert_eq!(attrs[0].format, wgpu::VertexFormat::Float32x3);
        assert_eq!(attrs[0].offset, 0);
        assert_eq!(attrs[0].shader_location, 0);
        // uv: Float32x2 @12 → location 1（旧 VERTEX_UV_OFFSET）
        assert_eq!(attrs[1].format, wgpu::VertexFormat::Float32x2);
        assert_eq!(attrs[1].offset, 12);
        assert_eq!(attrs[1].shader_location, 1);
        // shading_normal: Float32x3 @20 → location 2（旧 VERTEX_NORMAL_OFFSET）
        assert_eq!(attrs[2].format, wgpu::VertexFormat::Float32x3);
        assert_eq!(attrs[2].offset, 20);
        assert_eq!(attrs[2].shader_location, 2);

        assert_eq!(
            to_wgpu_step_mode(layout.step_mode),
            wgpu::VertexStepMode::Vertex
        );
    }

    /// B2-0d（D18）：IR 的 `UniformLayout` 必须与 `cube.wgsl` 的两个结构逐位一致。
    ///
    /// 纯 CPU 测试（不需要 GPU）：wgsl 侧改动 ⇒ 这里红 ⇒ 强制同步 IR 声明，
    /// 消灭 `pack_uniform_bytes` / `pack_light_bytes` 的魔数偏移漂移。
    #[test]
    fn test_uniform_layout_matches_cube_wgsl() {
        let uniforms = ze_render_ir::cube_uniforms_layout();
        assert_eq!(uniforms.size, UNIFORM_SIZE as u32);
        assert_eq!(uniforms.offset_of("mvp"), Some(0));
        assert_eq!(uniforms.offset_of("model"), Some(64));
        assert_eq!(uniforms.offset_of("color"), Some(128));
        assert_eq!(uniforms.offset_of("params"), Some(144));

        let lights = ze_render_ir::cube_lights_layout();
        assert_eq!(lights.size, LIGHT_UNIFORM_SIZE as u32);
        assert_eq!(lights.offset_of("counts"), Some(0));
        assert_eq!(lights.offset_of("dirs"), Some(16));
        assert_eq!(lights.offset_of("colors"), Some(80));
        assert_eq!(lights.offset_of("points"), Some(144));
        assert_eq!(lights.offset_of("point_colors"), Some(208));
        assert_eq!(lights.offset_of("camera"), Some(272));
    }

    /// B2-0c（D22）：能力查询的采样数映射（纯 CPU，合成 flags，无需 GPU）。
    ///
    /// 覆盖 1/2/4/8 四档；调用方（B2-5）据 `require_sample_count` 决定请求值，
    /// 超限即显式错误（不静默降级）。
    #[test]
    fn test_max_sample_count_from_flags() {
        use wgpu::TextureFormatFeatureFlags as F;
        assert_eq!(max_sample_count_from_flags(F::empty()), 1);
        assert_eq!(max_sample_count_from_flags(F::MULTISAMPLE_X2), 2);
        assert_eq!(max_sample_count_from_flags(F::MULTISAMPLE_X2 | F::MULTISAMPLE_X4), 4);
        assert_eq!(
            max_sample_count_from_flags(
                F::MULTISAMPLE_X2 | F::MULTISAMPLE_X4 | F::MULTISAMPLE_X8
            ),
            8
        );
        // 高位独在也要取到（不要求低位连续）
        assert_eq!(max_sample_count_from_flags(F::MULTISAMPLE_X4), 4);

        // IR 侧的能力门槛：4x 在 max=4 时可行、max=2 时显式报错
        let caps = Capabilities {
            max_sample_count: 2,
            color_format: ColorFormat::Rgba8UnormSrgb,
            depth_format: DepthFormat::Depth32Float,
            post: PostCaps::none(),
        };
        assert!(caps.require_sample_count(4).is_err());
        let caps = Capabilities {
            max_sample_count: 4,
            color_format: ColorFormat::Rgba8UnormSrgb,
            depth_format: DepthFormat::Depth32Float,
            post: PostCaps::none(),
        };
        assert_eq!(caps.require_sample_count(4).unwrap(), 4);
    }

    /// 读取打包字节中第 `i` 个 f32。
    fn word_at(bytes: &[u8], i: usize) -> f32 {
        f32::from_ne_bytes(bytes[i * 4..i * 4 + 4].try_into().expect("word slice"))
    }

    /// 默认光照字节布局：单方向光 `(0,0,1)` 纯白强度 1 + ambient 0.2 ——
    /// 与 B1-A 之前 `cube.wgsl` 硬编码 `LIGHT_DIR=(0,0,1)` / `AMBIENT=0.2` 逐位一致。
    #[test]
    fn test_default_light_bytes_layout() {
        let data = default_light_bytes(ze_notion::Vec3::ZERO);
        assert_eq!(data.len(), LIGHT_UNIFORM_SIZE as usize);
        // counts = (1 方向光, ambient 0.2, 0, 0)
        assert_eq!(word_at(&data, 0), 1.0);
        assert_eq!(word_at(&data, 1), DEFAULT_AMBIENT);
        assert_eq!(word_at(&data, 2), 0.0);
        assert_eq!(word_at(&data, 3), 0.0);
        // dirs[0] = (0, 0, 1, 强度 1)
        assert_eq!(word_at(&data, 4), 0.0);
        assert_eq!(word_at(&data, 5), 0.0);
        assert_eq!(word_at(&data, 6), 1.0);
        assert_eq!(word_at(&data, 7), 1.0);
        // colors[0] = (1, 1, 1, 1)
        assert_eq!(word_at(&data, 20), 1.0);
        assert_eq!(word_at(&data, 21), 1.0);
        assert_eq!(word_at(&data, 22), 1.0);
        assert_eq!(word_at(&data, 23), 1.0);
        // camera（B1-C）= (0, 0, 0, 0)
        assert_eq!(word_at(&data, 68), 0.0);
        assert_eq!(word_at(&data, 69), 0.0);
        assert_eq!(word_at(&data, 70), 0.0);
        assert_eq!(word_at(&data, 71), 0.0);
    }

    /// 两方向光的打包布局：第二光写入 `dirs[1]` / `colors[1]`，方向/强度/颜色逐字段正确。
    #[test]
    fn test_pack_light_bytes_two_directional() {
        let lights = [
            DirectionalLightRecord::new(ze_notion::Vec3::Z, Color32::WHITE, 1.0),
            DirectionalLightRecord::new(
                ze_notion::Vec3::new(0.0, 1.0, 0.0),
                Color32::new(255, 0, 0, 255),
                0.5,
            ),
        ];
        let data = pack_light_bytes(&lights, &[], 0.2, ze_notion::Vec3::ZERO);
        assert_eq!(word_at(&data, 0), 2.0);
        // 第二光 dirs[1] = (0, 1, 0, 0.5)
        assert_eq!(word_at(&data, 8), 0.0);
        assert_eq!(word_at(&data, 9), 1.0);
        assert_eq!(word_at(&data, 10), 0.0);
        assert_eq!(word_at(&data, 11), 0.5);
        // 第二光 colors[1] = (1, 0, 0, 1)
        assert_eq!(word_at(&data, 24), 1.0);
        assert_eq!(word_at(&data, 25), 0.0);
        assert_eq!(word_at(&data, 26), 0.0);
        assert_eq!(word_at(&data, 27), 1.0);
    }

    /// 点光打包布局（B1-B）：`counts.z` = 点光数；`points[i] = (位置, 强度)`；
    /// `point_colors[i] = (rgb, range)` —— 与 `cube.wgsl` 的 `Lights` 逐字段对应。
    #[test]
    fn test_pack_light_bytes_point() {
        let point = PointLightRecord::new(
            ze_notion::Vec3::new(1.0, 2.0, 3.0),
            Color32::new(0, 255, 0, 255),
            2.0,
            4.0,
        );
        let data = pack_light_bytes(&[], &[point], 0.2, ze_notion::Vec3::ZERO);
        assert_eq!(word_at(&data, 2), 1.0); // counts.z = 点光数
        // points[0] = (1, 2, 3, 强度 2)
        assert_eq!(word_at(&data, 36), 1.0);
        assert_eq!(word_at(&data, 37), 2.0);
        assert_eq!(word_at(&data, 38), 3.0);
        assert_eq!(word_at(&data, 39), 2.0);
        // point_colors[0] = (0, 1, 0, range 4)
        assert_eq!(word_at(&data, 52), 0.0);
        assert_eq!(word_at(&data, 53), 1.0);
        assert_eq!(word_at(&data, 54), 0.0);
        assert_eq!(word_at(&data, 55), 4.0);
        // 方向光区域不受影响（counts.x = 0）。
        assert_eq!(word_at(&data, 0), 0.0);
        // camera（B1-C）可打包：传入 (1,2,3) → words[68..71]。
        let data2 = pack_light_bytes(&[], &[point], 0.2, ze_notion::Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(word_at(&data2, 68), 1.0);
        assert_eq!(word_at(&data2, 69), 2.0);
        assert_eq!(word_at(&data2, 70), 3.0);
    }

    // ── G2：几何缓存键单测（纯 CPU，headless，无需 GPU） ──────────────

    fn basic_mesh() -> MeshGeometry {
        MeshGeometry::new(
            vec![
                ze_notion::Vec3::new(-1.0, -1.0, 0.0),
                ze_notion::Vec3::new(1.0, -1.0, 0.0),
                ze_notion::Vec3::new(0.0, 1.0, 0.0),
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
            Mat4::IDENTITY,
            Color32::new(255, 255, 255, 255),
            [0.0, 0.5, 0.0, 0.0],
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
            ze_notion::Vec3::new(0.0, 0.0, 5.0),
            ze_notion::Vec3::ZERO,
            ze_notion::Vec3::Y,
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
        backend.submit_layer(&layer, &mut encoder, &[], &[]);
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
            ze_notion::Vec3::ZERO,
            viewport,
            &[],
            &[],
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
            ze_notion::Vec3::ZERO,
            queue.meshes(),
            queue.lines(),
            &[],
            &[],
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
            ze_notion::Vec3::ZERO,
            queue.meshes(),
            queue.lines(),
            &[],
            &[],
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
                ze_notion::Vec3::ZERO,
                queue.meshes(),
                queue.lines(),
                &[],
                &[],
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
                ze_notion::Vec3::new(-1.0, -1.0, 0.5),
                ze_notion::Vec3::new(1.0, -1.0, 0.5),
                ze_notion::Vec3::new(1.0, 1.0, 0.5),
                ze_notion::Vec3::new(-1.0, 1.0, 0.5),
            ],
            Some(vec![0, 1, 2, 0, 2, 3]),
            PrimitiveTopology::Triangles,
        )
        .with_uvs(vec![
            ze_notion::Vec2::new(0.0, 1.0),
            ze_notion::Vec2::new(1.0, 1.0),
            ze_notion::Vec2::new(1.0, 0.0),
            ze_notion::Vec2::new(0.0, 0.0),
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
        let n = ze_notion::Vec3::new(1.0, 0.0, 1.0).normalize();
        let geometry = std::sync::Arc::new(
            MeshGeometry::new(
                vec![
                    ze_notion::Vec3::new(-1.0, -1.0, 0.5),
                    ze_notion::Vec3::new(1.0, -1.0, 0.5),
                    ze_notion::Vec3::new(1.0, 1.0, 0.5),
                    ze_notion::Vec3::new(-1.0, 1.0, 0.5),
                ],
                Some(vec![0, 1, 2, 0, 2, 3]),
                PrimitiveTopology::Triangles,
            )
            .with_uvs(vec![ze_notion::Vec2::ZERO; 4])
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

    /// B1-E 结构判据：pipeline **按需构建**且缓存命中 —— 只建实际绘制过的 key。
    ///
    /// 证伪：若仍走 `(variant, topology)` 笛卡尔积预建，构造后即有 3×2=6 条；
    /// 本测试要求构造后 0 条，并只随实际绘制递增（0 → 1 → 1 → 2 → 3）。
    #[test]
    fn test_b1e_pipelines_built_on_demand() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 32u32;
        assert_eq!(
            backend.pipeline_count(),
            0,
            "B1-E：构造后不应预建任何 pipeline（非笛卡尔积）"
        );

        let geometry = std::sync::Arc::new(parametric_surface(
            SurfaceFn::new((-0.5, 0.5), (-0.5, 0.5), |_, _| 0.0),
            1,
            1,
        ));

        // ① Lambert 三角形 → 第 1 条。
        let mut lambert = Material3D::color(Color32::WHITE);
        lambert.variant = ShaderVariant::Lambert;
        render_planes(
            &backend,
            &ctx,
            LayerId(30),
            size,
            &[MeshDraw::new(
                std::sync::Arc::clone(&geometry),
                Mat4::IDENTITY,
                lambert.clone(),
            )],
        );
        assert_eq!(backend.pipeline_count(), 1, "只建 Lambert 三角形 1 条");

        // ② 同 key 再画一遍 → 命中缓存，不新建。
        render_planes(
            &backend,
            &ctx,
            LayerId(31),
            size,
            &[MeshDraw::new(
                std::sync::Arc::clone(&geometry),
                Mat4::IDENTITY,
                lambert,
            )],
        );
        assert_eq!(backend.pipeline_count(), 1, "同 key 命中缓存，不再建");

        // ③ Pbr 三角形（同拓扑、不同变体）→ 第 2 条。
        let mut pbr = Material3D::color(Color32::WHITE);
        pbr.variant = ShaderVariant::Pbr;
        render_planes(
            &backend,
            &ctx,
            LayerId(32),
            size,
            &[MeshDraw::new(std::sync::Arc::clone(&geometry), Mat4::IDENTITY, pbr)],
        );
        assert_eq!(backend.pipeline_count(), 2);

        // ④ 线段（不同拓扑、无剔除）→ 第 3 条。
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test-b1e-lines-encoder"),
            });
        let _frame = backend.render_layer_offscreen(
            &mut encoder,
            LayerId(33),
            Mat4::IDENTITY,
            Vec3::ZERO,
            Viewport::full(size, size),
            &[],
            &[LineDraw::new(
                Vec3::new(-0.5, 0.0, 0.5),
                Vec3::new(0.5, 0.0, 0.5),
                Color32::WHITE,
            )],
            &[],
            &[],
        );
        ctx.submit(encoder.finish());
        ctx.device()
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll 失败");
        assert_eq!(
            backend.pipeline_count(),
            3,
            "3 条 = 实际绘制过的 key 数，而非 3 variant × 2 topology = 6 条预建"
        );
    }

    /// **B2-2 结构判据**：`double_sided` / `transparent` 是 pipeline key 的新维度，
    /// 与顶层 `ZOrder`（B2-1）一起工作；pipeline 数只随实际用到的状态组合增长。
    #[test]
    fn test_b2_2_pipeline_state_dimensions() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 32u32;
        let geometry = std::sync::Arc::new(parametric_surface(
            SurfaceFn::new((-0.5, 0.5), (-0.5, 0.5), |_, _| 0.0),
            1,
            1,
        ));

        // ① 默认材质（不双面、不透明）→ 1 条
        let base = Material3D::color(Color32::WHITE);
        render_planes(
            &backend,
            &ctx,
            LayerId(34),
            size,
            &[MeshDraw::new(std::sync::Arc::clone(&geometry), Mat4::IDENTITY, base)],
        );
        assert_eq!(backend.pipeline_count(), 1);

        // ② 双面（double_sided）→ 新 key（cull: Back → None）→ 2 条
        let mut double_sided = base;
        double_sided.double_sided = true;
        render_planes(
            &backend,
            &ctx,
            LayerId(35),
            size,
            &[MeshDraw::new(
                std::sync::Arc::clone(&geometry),
                Mat4::IDENTITY,
                double_sided,
            )],
        );
        assert_eq!(
            backend.pipeline_count(),
            2,
            "double_sided 必须参与 key（否则双面状态会被旧 pipeline 吃掉）"
        );

        // ③ 透明（transparent）→ 新 key（blend: Opaque → Alpha）→ 3 条
        let mut transparent = base;
        transparent.transparent = true;
        render_planes(
            &backend,
            &ctx,
            LayerId(36),
            size,
            &[MeshDraw::new(
                std::sync::Arc::clone(&geometry),
                Mat4::IDENTITY,
                transparent,
            )],
        );
        assert_eq!(backend.pipeline_count(), 3, "transparent 必须参与 key");

        // ④ D15：layout 是第五维 —— 两个只有布局不同的 key 必须不相等。
        let k = |layout: ze_vertex::VertexLayout| PipelineKey {
            topology: PrimitiveTopology::Triangles,
            variant: ShaderVariant::Lambert,
            cull: CullMode::Back,
            blend: LayerBlend::Opaque,
            layout,
        };
        let mut other_layout = ze_vertex::VertexLayout::standard_3d();
        other_layout.cache = ze_vertex::CacheStrategy::PerFrame;
        assert_ne!(k(ze_vertex::VertexLayout::standard_3d()), k(other_layout));
    }

    /// **B2-2 验收**：双面 —— 2D 派生片（平面）从**背面**可见。
    ///
    /// 用镜像投影翻转绕序来制造「从背面看」：单面材质被剔除（中心全透明），
    /// 双面材质保留像素（中心不透明）。
    #[test]
    fn test_b2_2_double_sided_visible_from_back() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 32u32;
        // 镜像 X ⇒ 三角形绕序翻转 ⇒ 等价于从背面观察。
        let mirrored = Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));

        // ① 单面：背面被剔除 ⇒ 中心 alpha = 0（清屏透明）
        let single = flat_plane(0.5, 0.5, 0.0, Color32::WHITE);
        let px = render_planes_with_view_proj(&backend, &ctx, LayerId(37), size, mirrored, &[single]);
        assert_eq!(
            pixel_at(&px, size, 16, 16)[3],
            0,
            "单面材质从背面必须不可见（cull Back）"
        );

        // ② 双面：同一视角可见
        let mut both = flat_plane(0.5, 0.5, 0.0, Color32::WHITE);
        both.material.double_sided = true;
        let px = render_planes_with_view_proj(&backend, &ctx, LayerId(38), size, mirrored, &[both]);
        assert!(
            pixel_at(&px, size, 16, 16)[3] > 0,
            "双面材质从背面必须可见（cull None）"
        );
    }

    /// **B2-3 验收**：2D 图元派生网格**真的能进 3D 管线**（派生 → 上传 → 绘制）。
    #[test]
    fn test_b2_3_derived_2d_rect_renders_in_3d() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 32u32;
        let shape = ze_render_ir::Shape2D::Rect(ze_notion::Rect2D::new(
            ze_notion::Vec2::new(-0.5, -0.5),
            ze_notion::Vec2::new(0.5, 0.5),
        ));
        let style = ze_render_ir::Style2D {
            fill: Some(Color32::WHITE),
            ..Default::default()
        };
        let derived = zela_render::derive_mesh(&shape, &style).expect("Rect 必须可派生");
        assert!(!derived.is_empty());
        let part = &derived[0];
        assert!(part.material.double_sided);
        assert_eq!(part.material.variant, ShaderVariant::Flat);

        let px = render_planes(
            &backend,
            &ctx,
            LayerId(41),
            size,
            &[MeshDraw::new(
                part.geometry.clone(),
                Mat4::IDENTITY,
                part.material,
            )],
        );
        assert_eq!(
            pixel_at(&px, size, 16, 16),
            [255, 255, 255, 255],
            "派生矩形应以 Flat 纯白覆盖中心（不参与光照）"
        );
    }

    /// **B2-4 验收**：共面两片靠 `ZOrder` 派生 epsilon 消除 z-fighting。
    ///
    /// 同一几何、同一深度：不带偏移时 `Less` 会淘汰后画的同深度片（红色胜出）；
    /// 带 epsilon 后高 `ZOrder` 片向相机侧偏移 ⇒ 蓝色胜出。**未使用 depth bias**。
    #[test]
    fn test_b2_4_epsilon_from_zorder_resolves_coplanar() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 32u32;
        let mut low = flat_plane(0.5, 0.5, 0.0, Color32::RED);
        low.z = ZOrder::new(0);
        let mut high = flat_plane(0.5, 0.5, 0.0, Color32::BLUE);
        high.z = ZOrder::new(1);

        // 真实透视相机（相机在 +Z ⇒ 法线 +Z 即朝向相机）：
        // 先画高层再画低层（顺序刻意与 ZOrder 相反）；epsilon 让高层在深度上更近。
        let camera = Camera3D::new(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            Projection::perspective(60f32.to_radians(), 1.0, 0.1, 100.0),
        );
        let px = render_with_camera(&backend, &ctx, LayerId(42), size, &camera, &[high, low]);
        let center = pixel_at(&px, size, 16, 16);
        assert!(
            center[0] > center[2],
            "ZOrder=1 的蓝色应因 epsilon 更近而胜出（BGRA：B=通道0），实际 {center:?}"
        );
        // 结构约束：深度偏差仍为空（D4：不用常量 depth bias）。
        // （pipeline 里 `DepthBiasState::default()`，见 get_or_create_pipeline。）
    }

    /// **B2-3 验收**：透明桶 —— 透明物体按统一 `ZOrder` 顺序混合。
    ///
    /// 两片**同深度**的半透明面（不写深度 ⇒ 都能通过深度测试）：ZOrder 大者后画，
    /// 其颜色应主导中心像素；把 ZOrder 对调 ⇒ 主导色随之对调（可证伪）。
    #[test]
    fn test_b2_3_transparent_bucket_orders_by_zorder() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 32u32;
        let red = Color32::new(255, 0, 0, 128);
        let blue = Color32::new(0, 0, 255, 128);

        let mut red_q = flat_plane(0.5, 0.5, 0.0, red);
        red_q.material.transparent = true;
        red_q.z = ZOrder::new(0);
        let mut blue_q = flat_plane(0.5, 0.5, 0.0, blue);
        blue_q.material.transparent = true;
        blue_q.z = ZOrder::new(1);

        // RED(z=0) 先画、BLUE(z=1) 后画 ⇒ 中心偏蓝。
        // 目标格式是 Bgra8（headless 默认）⇒ 字节 0 = B、字节 2 = R。
        let a = render_planes(&backend, &ctx, LayerId(39), size, &[red_q.clone(), blue_q.clone()]);
        let center = pixel_at(&a, size, 16, 16);
        assert!(
            center[0] > center[2],
            "ZOrder 较大者后画，蓝色应主导中心像素（BGRA：B=通道0），实际 {center:?}"
        );

        // 对调 ZOrder ⇒ 红色主导。
        let mut red_q2 = red_q.clone();
        red_q2.z = ZOrder::new(1);
        let mut blue_q2 = blue_q.clone();
        blue_q2.z = ZOrder::new(0);
        let b = render_planes(
            &backend,
            &ctx,
            LayerId(40),
            size,
            &[blue_q2, red_q2],
        );
        let center = pixel_at(&b, size, 16, 16);
        assert!(
            center[2] > center[0],
            "对调 ZOrder 后红色应主导中心像素（BGRA：R=通道2），实际 {center:?}"
        );
    }

    /// **B2-5 验收**：3D 上 MSAA 4x（与 2D 对齐），且采样数由构造侧给出（能力驱动）。
    ///
    /// 证伪：1x 后端对斜边只有全/全无两种覆盖；4x 后端应出现**部分覆盖**像素。
    #[test]
    fn test_b2_5_msaa_4x_produces_partial_coverage() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        assert_eq!(
            backend.sample_count(),
            DEFAULT_MSAA_SAMPLES,
            "默认与 2D 对齐（4x）"
        );

        let one_x = WgpuBackend3D::new_with_samples(
            ctx.device(),
            ctx.queue(),
            ctx.target_format(),
            DEFAULT_SHADER,
            1,
        );
        assert_eq!(one_x.sample_count(), 1);

        let size = 64u32;
        let mut draw = flat_plane(0.9, 0.9, 0.0, Color32::WHITE);
        // 绕 Z 旋转 ⇒ 斜边（硬边在 1x 下只有 0 / 255 两种 alpha）。
        draw.transform = Mat4::from_rotation_z(0.5) * draw.transform;

        let p4 = render_planes(&backend, &ctx, LayerId(43), size, &[draw.clone()]);
        let p1 = render_planes(&one_x, &ctx, LayerId(44), size, &[draw]);

        let partial = |px: &[u8]| {
            px.chunks_exact(4)
                .filter(|c| c[3] > 0 && c[3] < 255)
                .count()
        };
        assert!(
            partial(&p4) > partial(&p1),
            "4x 应产生更多部分覆盖像素（4x={}，1x={}）",
            partial(&p4),
            partial(&p1)
        );
    }

    /// **B1-F 验收（真实设备）**：点列表拓扑 ⇒ **必须显式报错**，不得静默跳过
    /// （铁律 2「不静默」：丢弃一个 draw 而不报错，会让"画面少东西"无从查起）。
    ///
    /// 证伪：若恢复「静默 continue」，本 `should_panic` 测试变红。
    #[test]
    #[should_panic(expected = "不静默")]
    fn test_b1f_points_topology_panics_explicitly() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 32u32;
        let geometry = std::sync::Arc::new(MeshGeometry::new(
            vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            None,
            PrimitiveTopology::Points,
        ));
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test-b1f-points-encoder"),
            });
        // render_layer_offscreen 内部走 record_prepared，遇到 Points 必须 panic。
        let _frame = backend.render_layer_offscreen(
            &mut encoder,
            LayerId(40),
            Mat4::IDENTITY,
            Vec3::ZERO,
            Viewport::full(size, size),
            &[MeshDraw::new(
                geometry,
                Mat4::IDENTITY,
                Material3D::color(Color32::WHITE),
            )],
            &[],
            &[],
            &[],
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
            Mat4::from_translation(ze_notion::Vec3::new(0.0, 0.0, z)),
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
        render_planes_with_view_proj(backend, ctx, layer_id, size, Mat4::IDENTITY, draws)
    }

    /// 以给定 `view_proj` 渲染一组网格并回读像素（B2-2 双面测试用）。
    fn render_planes_with_view_proj(
        backend: &WgpuBackend3D,
        ctx: &RenderContext,
        layer_id: LayerId,
        size: u32,
        view_proj: Mat4,
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
            view_proj,
            ze_notion::Vec3::ZERO,
            Viewport::full(size, size),
            draws,
            &[],
            &[],
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

    /// 以**给定相机与光源**渲染一组网格到离屏目标并回读像素（B1-A/B 用）。
    fn render_with_camera_lights(
        backend: &WgpuBackend3D,
        ctx: &RenderContext,
        layer_id: LayerId,
        size: u32,
        camera: &Camera3D,
        draws: &[MeshDraw],
        lights: &[DirectionalLightRecord],
        point_lights: &[PointLightRecord],
    ) -> Vec<u8> {
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test-3d-camera-lights-encoder"),
            });
        let _frame = backend.render_layer_offscreen(
            &mut encoder,
            layer_id,
            camera.view_projection(1.0),
            camera.eye,
            Viewport::full(size, size),
            draws,
            &[],
            lights,
            point_lights,
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

    /// 以**给定相机**渲染一组网格到离屏目标并回读像素（K 线验收用；
    /// 光源走默认光照）。
    fn render_with_camera(
        backend: &WgpuBackend3D,
        ctx: &RenderContext,
        layer_id: LayerId,
        size: u32,
        camera: &Camera3D,
        draws: &[MeshDraw],
    ) -> Vec<u8> {
        render_with_camera_lights(backend, ctx, layer_id, size, camera, draws, &[], &[])
    }

    /// **B1-A 验收（真实设备）**：同一曲面在**两个不同方向光**下像素不同；
    /// 且默认光照（无显式光源）与"单方向光 `(0,0,1)` 纯白强度 1"**逐位相同**
    /// —— 演进态的像素一致性锚点（旧硬编码 `LIGHT_DIR=(0,0,1)` / `AMBIENT=0.2`）。
    #[test]
    fn test_directional_light_affects_pixels_and_default_matches_legacy() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let camera = Camera3D::new(
            ze_notion::Vec3::new(0.0, 0.0, 3.0),
            ze_notion::Vec3::ZERO,
            ze_notion::Vec3::Y,
            Projection::perspective(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 100.0),
        );
        // 真实曲面（法线随位置变化）才能体现方向光差异。
        let draws = vec![flat_plane(1.0, 1.0, 0.0, Color32::new(255, 255, 255, 255))];
        let up_light = [DirectionalLightRecord::new(ze_notion::Vec3::Z, Color32::WHITE, 1.0)];
        let side_light = [DirectionalLightRecord::new(
            ze_notion::Vec3::new(1.0, 0.0, 0.0),
            Color32::WHITE,
            1.0,
        )];

        let px_default = render_with_camera(&backend, &ctx, LayerId(1), 64, &camera, &draws);
        let px_up = render_with_camera_lights(
            &backend,
            &ctx,
            LayerId(2),
            64,
            &camera,
            &draws,
            &up_light,
            &[],
        );
        let px_side = render_with_camera_lights(
            &backend,
            &ctx,
            LayerId(3),
            64,
            &camera,
            &draws,
            &side_light,
            &[],
        );

        assert_eq!(
            px_default, px_up,
            "默认光照必须与显式单光 (0,0,1) 逐位一致（B1-A 像素锚点）"
        );
        assert_ne!(
            px_up, px_side,
            "两个不同方向光下同一场景的像素必须不同"
        );
    }

    /// **B1-A 验收（真实设备）**：方向光数量超上限 ⇒ **显式报错**（铁律 2：
    /// 不静默丢弃）。空灯光为默认光照；`MAX_DIRECTIONAL_LIGHTS + 1` 个光必须 panic。
    #[test]
    #[should_panic(expected = "超过上限")]
    fn test_directional_light_over_limit_panics() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let mut lights = Vec::new();
        for _ in 0..=MAX_DIRECTIONAL_LIGHTS {
            lights.push(DirectionalLightRecord::new(
                ze_notion::Vec3::Z,
                Color32::WHITE,
                1.0,
            ));
        }
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test-3d-over-limit-encoder"),
            });
        let _ = backend.render_layer_offscreen(
            &mut encoder,
            LayerId(0),
            Mat4::IDENTITY,
            ze_notion::Vec3::ZERO,
            Viewport::full(64, 64),
            &[],
            &[],
            &lights,
            &[],
        );
    }

    /// 非透明像素数（画面覆盖面积的代理量）。
    fn coverage(pixels: &[u8]) -> usize {
        pixels.chunks_exact(4).filter(|p| p[3] > 0).count()
    }

    /// **B1-B 验收（真实设备）**：点光数量超上限 ⇒ **显式报错**（铁律 2：
    /// 不静默丢弃）。`MAX_POINT_LIGHTS + 1` 个点光必须 panic。
    #[test]
    #[should_panic(expected = "超过上限")]
    fn test_point_light_over_limit_panics() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let mut points = Vec::new();
        for _ in 0..=MAX_POINT_LIGHTS {
            points.push(PointLightRecord::new(
                ze_notion::Vec3::ZERO,
                Color32::WHITE,
                1.0,
                2.0,
            ));
        }
        let mut encoder = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("test-3d-point-over-limit-encoder"),
            });
        let _ = backend.render_layer_offscreen(
            &mut encoder,
            LayerId(0),
            Mat4::IDENTITY,
            ze_notion::Vec3::ZERO,
            Viewport::full(64, 64),
            &[],
            &[],
            &[],
            &points,
        );
    }

    /// **B1-B 验收（真实设备）**：点光距离衰减 —— 同一平面上，靠近光源的像素
    /// 显著亮于远离光源（超出 range 的像素只剩环境光）。这是"点光真的随距离
    /// 衰减"的像素级证据，而非只断言 uniform 打包。
    #[test]
    fn test_point_light_distance_falloff() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 64u32;
        let camera = Camera3D::new(
            ze_notion::Vec3::new(0.0, 0.0, 3.0),
            ze_notion::Vec3::ZERO,
            ze_notion::Vec3::Y,
            Projection::perspective(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 100.0),
        );
        // 大平面（半边长 2.0，屏幕 64px 内全被覆盖）：点光悬在平面正上方 (0,0,1)，
        // range 2.0 ⇒ 中心（距离 1）有衰减项，屏幕角落（距离 > 2）只剩 ambient。
        let draws = vec![flat_plane(2.0, 2.0, 0.0, Color32::new(255, 255, 255, 255))];
        let point = [PointLightRecord::new(
            ze_notion::Vec3::new(0.0, 0.0, 1.0),
            Color32::WHITE,
            1.0,
            2.0,
        )];
        let px = render_with_camera_lights(&backend, &ctx, LayerId(1), size, &camera, &draws, &[], &point);
        // 屏幕中心 = 平面中心（光源正下方，距离 1）。
        let center = pixel_at(&px, size, size / 2, size / 2);
        // 屏幕左上角 = 平面左上角附近（距离 > range ⇒ 衰减到 0）。
        let corner = pixel_at(&px, size, 0, 0);
        // 白色光下三通道相等，取通道 0 比较即可（Bgra8 字节序）。
        assert!(
            center[0] > corner[0],
            "点光近处应显著亮于远处：center={:?} corner={:?}",
            center,
            corner
        );
        // 近处至少是"ambient + 明显衰减项"（远大于纯 ambient 0.2 的 sRGB ≈ 124）。
        assert!(center[0] > 150, "光源正下方应明显亮于环境光：{:?}", center);
    }

    // ── B1-C：PBR（真实设备） ─────────────────────────────────────────

    /// 注册一张 1×1 法线贴图（`Rgba8Unorm`，**非 sRGB** —— 见 `new()` 注释：
    /// sRGB 采样会自动解码到线性域，会破坏法线值）。
    fn register_normal_map(
        backend: &WgpuBackend3D,
        ctx: &RenderContext,
        id: u64,
        texel: [u8; 4],
    ) {
        let texture = ctx.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("test-3d-normal-map"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
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
            &texel,
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
            id,
            texture.create_view(&wgpu::TextureViewDescriptor::default()),
        );
    }

    /// **B1-C 验收（真实设备）**：PBR 变体接线成功的像素级证据 ——
    /// 1. 同一曲面，`Pbr` 与 `Lambert` 像素不同（GGX 高光 + 线性域 albedo）；
    /// 2. `metallic` 切换（0 ↔ 1）改变像素（金属无漫反射项）；
    /// 3. 法线贴图（倾斜法线）改变像素（屏幕空间 TBN 生效）；
    /// 4. 同一带法线贴图场景，**相机位置**改变像素（PBR 是视点相关的，
    ///    证明 `Lights.camera` 真的进入 uniform 并参与计算）。
    #[test]
    fn test_pbr_metal_roughness_normal_map_change_pixels() {
        let Some((_gpu, backend, ctx)) = headless_backend() else {
            return;
        };
        let size = 64u32;
        let camera = Camera3D::new(
            ze_notion::Vec3::new(0.0, 0.0, 3.0),
            ze_notion::Vec3::ZERO,
            ze_notion::Vec3::Y,
            Projection::perspective(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 100.0),
        );
        // 正对 +Z 的平面（UV 随位置变化 ⇒ 屏幕空间 TBN 非退化）。
        let plane = std::sync::Arc::new(parametric_surface(
            SurfaceFn::new((-1.0, 1.0), (-1.0, 1.0), |_, _| 0.0),
            1,
            1,
        ));

        let lambert = Material3D::color(Color32::WHITE); // 默认 Lambert
        let mut pbr_dielectric = Material3D::color(Color32::WHITE);
        pbr_dielectric.variant = ShaderVariant::Pbr; // metallic 0 / roughness 0.5
        let mut pbr_metal = Material3D::color(Color32::WHITE);
        pbr_metal.variant = ShaderVariant::Pbr;
        pbr_metal.metallic = 1.0;

        let draw = |material: Material3D| {
            MeshDraw::new(std::sync::Arc::clone(&plane), Mat4::IDENTITY, material)
        };

        // 1) Pbr ≠ Lambert（默认光照、同一相机）。
        let px_lambert =
            render_with_camera(&backend, &ctx, LayerId(31), size, &camera, &[draw(lambert)]);
        let px_pbr =
            render_with_camera(&backend, &ctx, LayerId(32), size, &camera, &[draw(pbr_dielectric)]);
        assert_ne!(px_lambert, px_pbr, "Pbr 与 Lambert 必须产生不同像素");

        // 2) metallic 0 ↔ 1（同为 Pbr）。
        let px_metal =
            render_with_camera(&backend, &ctx, LayerId(33), size, &camera, &[draw(pbr_metal)]);
        assert_ne!(px_pbr, px_metal, "metallic 0 与 1 必须产生不同像素");

        // 3) 法线贴图：1×1 倾斜法线（(255,128,255) → nts ≈ (1, 0, 1)，法线朝 +X 斜 45°）。
        register_normal_map(&backend, &ctx, 7, [255, 128, 255, 255]);
        let mut pbr_bumped = Material3D::color(Color32::WHITE);
        pbr_bumped.variant = ShaderVariant::Pbr;
        pbr_bumped.normal_map = Some(TextureHandle::new(7));
        let px_bump =
            render_with_camera(&backend, &ctx, LayerId(34), size, &camera, &[draw(pbr_bumped)]);
        assert_ne!(px_pbr, px_bump, "法线贴图必须改变 PBR 像素（屏幕空间 TBN 生效）");

        // 4) 视点相关：同一带法线贴图场景，换相机位置 ⇒ 像素改变。
        let camera2 = Camera3D::new(
            ze_notion::Vec3::new(2.0, 0.0, 3.0),
            ze_notion::Vec3::ZERO,
            ze_notion::Vec3::Y,
            Projection::perspective(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 100.0),
        );
        let px_bump2 =
            render_with_camera(&backend, &ctx, LayerId(35), size, &camera2, &[draw(pbr_bumped)]);
        assert_ne!(
            px_bump, px_bump2,
            "相机位置必须改变 PBR 像素（视线向量进入 uniform）"
        );
    }

    /// B1-C：uniform `params`（metallic/roughness）字节打包位置 ——
    /// mvp(0..16) + model(16..32) + color(32..36) + params(36..40)。
    #[test]
    fn test_uniform_bytes_params_packing() {
        let data = uniform_bytes(
            Mat4::IDENTITY,
            Mat4::IDENTITY,
            Color32::WHITE,
            [0.25, 0.75, 0.0, 0.0],
        );
        assert_eq!(data.len(), UNIFORM_SIZE as usize);
        assert_eq!(word_at(&data, 36), 0.25);
        assert_eq!(word_at(&data, 37), 0.75);
        assert_eq!(word_at(&data, 38), 0.0);
        assert_eq!(word_at(&data, 39), 0.0);
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
        use ze_scene_core::FpsView;
        use zela_scene_ecs::camera_input::{apply_fps, apply_orbit};
        use zela_scene_ecs::domain::CameraCommand;
        use zela_scene_ecs::{AnimValue, Curve, Interp};

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

