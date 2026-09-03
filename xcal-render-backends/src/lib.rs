// xcal-render-backends: 具体渲染后端实现（feature-gated）
//
// 依赖方向：xcal-geometry / xcal-core ← xcal-render ← xcal-render-backends
//
// 模块：
//   - render_context：RenderContext（device/queue/format 句柄，wgpu/pollster）
//   - wgpu_3d：WgpuBackend3D / PreparedDraw3D / Frame3DData（3D MVE 后端）
//   - layer_callback：Layer3DCallback（egui_wgpu 回调，feature = "egui-wgpu"）
//   - egui_2d：EguiPainter + color_to_egui（Painter2D 的 egui 实现，feature = "egui"）
//   - eframe_backend：EframeWgpuBackend（RenderBackend 的 egui-wgpu 实现）
//   - svg_recorder：SVG 录制后端（预留空壳）
//   - text：文本引擎（Typst/LaTeX，预留空壳，feature = "text"）

#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod render_context;
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub mod wgpu_3d;

#[cfg(feature = "egui-wgpu")]
pub mod eframe_backend;
#[cfg(feature = "egui-wgpu")]
pub mod layer_callback;

#[cfg(feature = "egui")]
pub mod egui_2d;

pub mod svg_recorder;

pub mod text;

// ─── re-export ────────────────────────────────────────────────────────────

#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use render_context::{DEFAULT_TARGET_FORMAT, RenderContext};
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use wgpu_3d::{Frame3DData, PreparedDraw3D, WgpuBackend3D};

#[cfg(feature = "egui-wgpu")]
pub use eframe_backend::EframeWgpuBackend;
#[cfg(feature = "egui-wgpu")]
pub use layer_callback::Layer3DCallback;

#[cfg(feature = "egui")]
pub use egui_2d::{EguiPainter, color_to_egui};
