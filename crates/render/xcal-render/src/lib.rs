// xcal-render: 渲染 trait + 调度 + 资源管理层（三层架构的中间层）
// 依赖方向：xcal-geometry / xcal-core ← xcal-render ← xcal-render-backends
//
// 本 crate 禁止依赖 egui。wgpu 仅通过 feature = "wgpu" 引入
// （RenderTarget / RenderLayer / RenderBackend / ResourceCache 等 GPU 抽象）。
//
// 包含：
//   - 冻结的 2D 管线（draw.rs：Painter2D / DrawQueue / Shape / Style / record_scene）
//   - 3D 队列与绘制器 trait（draw_queue_3d.rs / painter_3d.rs）
//   - 视口/渲染目标/渲染层（viewport.rs / render_target.rs / render_layer.rs）
//   - 后端 trait 与资源缓存（render_backend.rs / resource_cache.rs，wgpu gate）
//   - 新图元体系（shape.rs / painter_2d.rs / draw_queue_2d.rs / vector_content.rs）
//   - 文本渲染抽象（text_engine.rs）
//
// 具体后端（wgpu_3d / egui_2d / eframe / 文本引擎实现）位于 xcal-render-backends。

pub mod components;
pub mod content_ir;
pub mod draw;
pub mod draw_queue_2d;
pub mod draw_queue_3d;
pub mod painter_2d;
pub mod painter_3d;
pub mod shape;
pub mod tessellate;
pub mod text_engine;
pub mod vector_content;
pub mod vertex;
pub mod viewport;

// render_backend 模块始终编译（GpuHandle 无 wgpu 依赖）；
// RenderBackend / RenderBackend3D trait 在模块内 cfg(feature = "wgpu")。
pub mod render_backend;
#[cfg(feature = "wgpu")]
pub mod render_layer;
#[cfg(feature = "wgpu")]
pub mod render_target;
#[cfg(feature = "wgpu")]
pub mod resource_cache;

// ─── 2D 管线 re-export ────────────────────────────────────────────────────
// M0 起载荷统一为 Shape2D / Style2D（旧 Shape / Style 枚举已删除）。
pub use draw::{DrawCommand, DrawQueue, Painter2D, SceneRender, ZIndex, record_scene};

// ─── 呈现层度量空间（宽度所属空间）───────────────────────────────────────
// 由 `record_scene` 统一解析（唯一换算点）；下游（UI/App）用它声明描边宽度
// 属于世界空间还是屏幕空间。
pub use xc_geometry::MeasureSpace;

// ─── 3D 队列 / 绘制器 ─────────────────────────────────────────────────────
pub use draw_queue_3d::{DrawQueue3D, LineDraw, MeshDraw};
pub use painter_3d::Painter3D;

// ─── 视口（纯数据） ───────────────────────────────────────────────────────
pub use viewport::Viewport;

// ─── 新图元体系 ───────────────────────────────────────────────────────────
pub use content_ir::{ContentDrawCmd, ir_to_draw_cmds};
pub use draw_queue_2d::{DrawCommand2D, DrawQueue2D};
pub use painter_2d::{Painter2DTarget, Style2D};
pub use shape::{CustomShapeData, CustomTypeId, Shape2D};

// ─── S3-0：lyon 三角化适配层（纯 CPU，无 wgpu/egui 依赖） ────────────────
pub use tessellate::{
    TessKind, Tessellation2D, Vertex2D, tessellate, tessellation_layout, tolerance_for_scale,
};
// text_engine 已由新的 ContentIR 两阶段管线取代（见 text_engine.rs 各条目的 #[deprecated]）。
// 这里保留 re-export 以免下游直接从 xcal-render:: 取类型时 break；
// 导入侧显式 allow，避免 crate 自己 re-export 就触发 deprecated warning。
#[allow(deprecated)]
pub use text_engine::{
    BitmapFormat, BitmapOverlay, TextConfig, TextInput, TextRenderEngine, TextRenderError,
    TextRenderOutput,
};
pub use vector_content::VectorContent;

// ─── 顶点格式契约（纯数据，无 wgpu 依赖；翻译层在 xcal-render-backends） ──
pub use vertex::{
    hash_bytes, hash_bytes_sampled, AaModel, AaSlotSemantics, AttribSemantic, AttribType,
    CacheStrategy, StaticConsistencyChecker, StepMode, VertexAttrib, VertexInputDesc,
    VertexLayout, VertexLayoutError,
};

// ─── wgpu 抽象（feature gate） ────────────────────────────────────────────
#[cfg(feature = "wgpu")]
pub use render_backend::{RenderBackend, RenderBackend3D};
#[cfg(feature = "wgpu")]
pub use render_layer::RenderLayer;
#[cfg(feature = "wgpu")]
pub use render_target::RenderTarget;
#[cfg(feature = "wgpu")]
pub use resource_cache::{
    CacheKey, DeferredDestroyer, GpuResourceCache, ResourceCache, ResourceKey,
};

// GpuHandle / GpuResourceId / AccessError 是纯 Any 包装 + 类型，无 wgpu 依赖，始终可用。
pub use render_backend::{AccessError, GpuHandle, GpuResourceId};

// 为下游用户提供与原 xcal::scene::comp::{StrokeSolid, FillSolid} 一致的访问
// 通过在 scene 根重新导出 components + render 组件。
pub mod scene {
    pub use crate::components;
    pub use crate::components as comp;
    pub use xcal_core::scene::Scene;
}
