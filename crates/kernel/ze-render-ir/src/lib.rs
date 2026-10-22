// ze-render-ir — 渲染 IR 层
//
// 2D 图元的**中间表示**：Shape2D（逻辑图元）+ Style2D（绘制样式）+ 样式组件 +
// 「对象 → IR」的降级 trait（ToShape2D / ResolveStyle）。
//
// 分层位置（M2 建立）：
//
// ```text
// ze-notion          理念层      数学值 + 代数运算
// ze-render-ir       渲染 IR     Shape2D / Style2D / ToShape2D / ResolveStyle   ← 本 crate
// ze-scene-contract  契约层      Scene / ObjectId / Describable / *Desc
// ze-object          场景对象层  Circle / Camera2D / Transform / …（M3）
// zela-render        渲染管线    三角化 / 合批 / 队列（lyon 等后端侧依赖）
// ```
//
// 硬约束（与理念层同级，验收口径见 M2 计划 §6.2）：
//   - 只依赖理念层 `ze-notion` + kernel 层的 `ze-vertex`（B2-0b / D23：IR 携带顶点
//     布局引用；两者同在 kernel 层，不构成反向依赖）
//   - 不得出现 ECS / hecs / GPU / wgpu / egui / UI
//   - 不得依赖 `zela-*` / `ze-object`
//     （`cargo tree -p ze-render-ir | grep -E "zela-|ze-object"` = 0）
//
// 为什么 `ToShape2D` 吃的是仿射矩阵而不是相机：见 `degrade.rs` 顶部注释
// （相机在场景对象层，IR 层认识它会成环）。

pub mod caps;
pub mod degrade;
pub mod draw;
pub mod layer;
pub mod mesh;
pub mod params;
pub mod pipeline;
pub mod shape;
pub mod style;
pub mod target;
pub mod uniform;

pub use degrade::ToShape2D;
pub use caps::{
    CapabilityNegotiationError, NAMESPACES, PINNED_FINGERPRINTS, RenderCapabilitySet,
    fingerprints, negotiate, signatures_of,
};
pub use draw::{DrawCommand, ZIndex, ZOrder};
pub use layer::{LayerBlend, LayerId};
pub use mesh::{PrimitiveTopology, ShaderVariant, TextureHandle, UvProjection};
pub use params::{
    AmbientOcclusion, Bloom, MAX_SUPERSAMPLE, Msaa, RenderParamError, RenderParams, TextureFilter,
    ToneCurve, ToneMapping,
};
pub use pipeline::{
    Capabilities, CapabilityError, ColorFormat, CullMode, DepthFormat, PassDepth, PassDesc,
    PipelineDesc, PostCaps,
};
pub use shape::{CustomShapeData, CustomSpec, CustomSpecs, Shape2D, SpecId, Style2D};
pub use style::{
    FillSolid, FillStyle, Opacity, ResolveStyle, StrokeCap, StrokeJoin, StrokeShape, StrokeSolid,
    StrokeStyle, fill_style, stroke_style_raw,
};
pub use uniform::{
    UniformField, UniformLayout, UniformLayoutError, UniformType, cube_lights_layout,
    cube_uniforms_layout,
};
pub use target::{RenderTargetDesc, TargetDescError};

/// 呈现层度量空间：宽度属**世界**（随缩放）还是**屏幕**（不随缩放）。
///
/// 定义在理念层 `ze-notion`；此处转发，使 IR 的消费者（渲染层 / UI / App）
/// 不必单独声明 `ze-notion` 依赖即可 `zela_render::MeasureSpace` 引用。
pub use ze_notion::MeasureSpace;
