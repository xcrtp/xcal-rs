// xcal-render-backends: 具体渲染后端实现（feature-gated）
//
// 依赖方向：xc-notion / xcal-scene ← xcal-render ← xcal-render-backends
//
// 模块：
//   - render_context：RenderContext（device/queue/format 句柄，wgpu/pollster）
//   - wgpu_3d：WgpuBackend3D / PreparedDraw3D / Frame3DData（3D MVE 后端）
//   - scene_callback：SceneCompositorCallback（唯一 egui_wgpu 回调，一次合成全部图层，feature = "egui-wgpu"）
//   - eframe_backend：EframeWgpuBackend（RenderBackend 的 egui-wgpu 实现）
//   - text：文本引擎（Typst/LaTeX，预留空壳，feature = "text"）
//
// S4.1：旧 CPU 2D 管线（`egui_2d` 模块 + 其编译期 feature）已删除——
// 2D 统一走 `wgpu_2d` 离屏渲染（lyon 三角化），不再有逐变体手写的 egui 回放路径。

#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod gpu_test;
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod gpu_util;
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod render_context;
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod wgpu_3d;
// 顶点格式契约 → wgpu 翻译层（V6）：纯函数，不碰 wgpu_3d.rs
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod vertex_layout;
// S3-1：wGPU 2D 后端（离屏纹理 + MSAA 4x + 合成）。egui 回调部分单独门控
// （`egui-wgpu`），纯后端与几何/裁剪数学在 `wgpu` 下即可编译与测试。
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod wgpu_2d;
// S3-2/C4：图层合成器（N 张图层纹理按声明顺序混合进主 pass）。
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod compositor;

#[cfg(feature = "egui-wgpu")]
pub mod eframe_backend;
// W1：**不依赖 egui 的渲染环境装配**（`StandardBackends`）+ headless 渲染入口。
// 与 `wgpu_2d` 等同门控（只吃 `RenderContext`），故在 `wgpu` 下即可编译与测试。
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod standard_backends;
// S3-2 修复：合并 2D/3D 为**单个**回调，一次性合成全部图层。
// （`CallbackResources` 是全局 TypeMap，每层一个 callback 会互相覆盖合成资源。）
//
// W1：本模块**不再整体**门控在 `egui-wgpu` 下 —— `SceneCompositorCallback` 的字段
// 全是纯数据，它同时是 headless 路径的"一帧渲染请求"；只有
// `impl egui_wgpu::CallbackTrait` 留在 `egui-wgpu` 门控内。
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod scene_callback;

// 旧 svg_recorder / text 空壳已删除（见上注）。
// 下游若有使用，改为：xc-prerender::engine → xc-content::ContentIR → xcal-render::ir_to_draw_cmds。

// ─── re-export ────────────────────────────────────────────────────────────

#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use render_context::{DEFAULT_TARGET_FORMAT, RenderContext};
// C1：GPU 用例的统一入口（跳过可见 + 严格模式）。
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use compositor::{CompositeFrame, CompositeOp, Compositor};
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use gpu_test::{GpuGuard, require_gpu, require_gpu_named};
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use vertex_layout::to_wgpu;
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use wgpu_2d::{
    Frame2DData, Frame2DGeometry, MSAA_SAMPLES, OFFSCREEN_FORMAT, Recorded2D, WgpuBackend2D,
    snapshot_queue,
};
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use wgpu_3d::{Frame3DData, Layer3DFrame, PreparedDraw3D, WgpuBackend3D};

#[cfg(feature = "egui-wgpu")]
pub use eframe_backend::{EframeWgpuBackend, install_standard_backends};
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use scene_callback::{SceneCompositorCallback, SceneCompositorFrame, SceneLayer};
// W1：不依赖 egui 的装配 + headless 渲染入口。
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use standard_backends::{
    BackendStore, ScreenParams, StandardBackends, render_frame_headless, render_single_frame,
};
