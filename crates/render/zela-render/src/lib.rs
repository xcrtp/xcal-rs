// zela-render: 渲染管线层（**渲染无关**；B3-0.5e 起不依赖 wgpu）
// 依赖方向：ze-notion / zela-scene ← zela-render ← zela-render-backends
//
// 本 crate 禁止依赖 egui；GPU 侧契约（RenderTarget / RenderLayer / RenderBackend /
// ResourceCache）已迁入 `zela-render-backends`（B3-0.5e）。
//
// 包含：
//   - 2D 绘制管线（draw.rs：Painter2D / DrawQueue / Shape2D / Style2D /
//     Drawable / record_drawables / record_scene）
//   - 3D 队列与绘制器 trait（draw_queue_3d.rs / painter_3d.rs）
//   - 视口（viewport.rs）与顶点生产（vertex_record.rs）
//   - 路径稳定的转发模块（`shape` / `vertex`）与预留后端目标
//     （`painter_2d.rs::Painter2DTarget`，已有生产实现 `WgpuBackend2D`）
//
// 渲染契约层（顶点格式 / 资源缓存纯数据）与图元载荷（Shape2D / Style2D）已下沉 kernel
// （ze-vertex / ze-resource / ze-render-ir）；这里通过 `pub use ze_vertex as vertex;`、
// `pub mod shape { .. }` 等**转发模块**保留原有公开路径，真实定义在各自 kernel crate。
//
// 具体后端（wgpu_2d / wgpu_3d / eframe）位于 zela-render-backends。
// S4.1：旧 CPU 2D 回放后端（`egui_2d` 模块）已删除。

pub mod components;
pub mod content_ir;
pub mod draw;
pub mod draw_queue_3d;
pub mod filters;
pub mod frame;
pub mod light_queue;
pub mod painter_2d;
pub mod painter_3d;
pub mod record_systems;
pub mod shape_to_mesh;
pub mod tessellate;
pub mod vertex_record;
pub mod view_rect;
pub mod viewport;

// 顶点格式契约已下沉 kernel/ze-vertex：保留 `zela_render::vertex::*` 路径。
pub use ze_vertex as vertex;

// 2D 图元体系（Shape2D / Style2D / CustomShapeData）已下沉 kernel/ze-render-ir：
// 保留 `zela_render::shape::*` 路径（与本 crate 的 `vertex` 同一口径的过渡 shim）。
pub mod shape {
    pub use ze_render_ir::shape::*;
}

// render_backend 模块始终编译（GpuHandle 无 wgpu 依赖）；
// B3-0.5e：GPU 侧模块（render_backend / render_layer / render_target / resource_cache）
// 已迁入 `zela-render-backends` —— 本 crate **不再依赖 wgpu**（渲染无关层）。
// 依赖方向：ze-notion / zela-scene ← zela-render ← zela-render-backends。

// ─── 2D 管线 re-export ────────────────────────────────────────────────────
// M0 起载荷统一为 Shape2D / Style2D（旧 Shape / Style 枚举已删除）。
// M2 起 Shape2D / Style2D / 样式组件 / 降级 trait 的定义在 kernel/ze-render-ir，
// 此处转发以保持 `zela_render::*` 路径不变（下游 / 后端无需改动）。
pub use draw::{
    ContentTextureQueue, DrawItem, DrawQueue, Drawable, MorphItem, MorphQueue, Painter2D,
    RevealItem, RevealQueue, SceneRender, apply_opacity, opacity_of, push_morph_item,
    push_reveal_item, record_drawables, record_path_stroke, record_scene, record_scene_tagged,
    visible_for,
};
// B2-0b：`DrawCommand` / `ZIndex` 已迁入渲染 IR（单一真源），此处转发保持
// `zela_render::{DrawCommand, ZIndex}` 路径不变（下游与后端无需改动）。
pub use ze_render_ir::{
    CustomShapeData, CustomSpec, CustomSpecs, DrawCommand, FillSolid, FillStyle, Opacity,
    ResolveStyle, Shape2D, SpecId, StrokeSolid, StrokeStyle, Style2D, ToShape2D, ZIndex, ZOrder,
};
// B2-3：2D 图元 → 3D 平面网格（派生桥）。
pub use shape_to_mesh::{DerivedMesh, ShapeMeshError, derive_mesh};

// ─── 呈现层度量空间（宽度所属空间）───────────────────────────────────────
// 由 `record_scene` 统一解析（唯一换算点）；下游（UI/App）用它声明描边宽度
// 属于世界空间还是屏幕空间。
pub use ze_notion::MeasureSpace;

// ─── 3D 队列 / 绘制器 ─────────────────────────────────────────────────────
pub use draw_queue_3d::{DrawQueue3D, LineDraw, MeshDraw};
pub use painter_3d::Painter3D;
pub use light_queue::{DirectionalLightRecord, LightQueue, PointLightRecord};

// ─── 视口（纯数据） ───────────────────────────────────────────────────────
pub use viewport::Viewport;

// ─── 渲染目标矩形（Resource；S0 定义下沉 zela-scene::domain）─────────────
pub use view_rect::ViewRect;

// ─── 帧体构造（S0 从 zela-egui 归位；无 egui / 无后端类型）────────────────
pub use frame::{
    FrameInputs, FrameOptions, SceneLayer, build_frame_inputs, build_scene_view, draw_call_count,
    end_frame,
};

// ─── 合成后滤镜链（C 线：纯数据；管线/着色器在 zela-render-backends） ─────
pub use filters::{FilterKind, FilterSlot, MAX_FILTER_SLOTS, POST_UNIFORM_SIZE, PostFilterChain};

// ─── 录制 System（P4 从 zela-egui 归位到渲染层） ──────────────────────────
pub use record_systems::{
    ContentTextureRecordSystem, MorphRecordSystem, RevealRecordSystem, SceneRecord3DSystem,
    SceneRecordSystem, register_record_systems,
};

// ─── 内容 IR 与预留后端目标 ───────────────────────────────────────────────
pub use content_ir::{ContentDrawCmd, ir_to_draw_cmds};
pub use painter_2d::Painter2DTarget;

// ─── S3-0：lyon 三角化适配层（纯 CPU，无 wgpu/egui 依赖） ────────────────
pub use tessellate::{
    REVEAL_STEP_PX, TessKind, TessellateError, Tessellation2D, Vertex2D, tessellate,
    tessellate_reveal, tessellate_with, tessellation_layout, tolerance_for_scale,
};

// ─── 顶点格式契约（纯数据，无 wgpu 依赖；翻译层在 zela-render-backends） ──
pub use vertex::{
    AaModel, AaSlotSemantics, AttribSemantic, AttribType, CacheStrategy, StaticConsistencyChecker,
    StepMode, VertexAttrib, VertexInputDesc, VertexLayout, VertexLayoutError, hash_bytes,
    hash_bytes_sampled,
};

// ─── 渲染契约的纯数据类型（无 wgpu 依赖） ─────────────────────────────────
// GpuHandle / GpuResourceId / AccessError 来自 kernel/ze-resource（始终可用）。
pub use ze_resource::render_backend::{AccessError, GpuHandle, GpuResourceId};

// 为下游用户提供与原 zela::scene::comp::{StrokeSolid, FillSolid} 一致的访问
// 通过在 scene 根重新导出 components + render 组件。
pub mod scene {
    pub use crate::components;
    pub use crate::components as comp;
    pub use zela_scene_ecs::scene::Scene;
}
