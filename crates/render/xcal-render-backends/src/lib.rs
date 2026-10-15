// xcal-render-backends: 具体渲染后端实现（feature-gated）
//
// 依赖方向：xcal-geometry / xcal-scene ← xcal-render ← xcal-render-backends
//
// 模块：
//   - render_context：RenderContext（device/queue/format 句柄，wgpu/pollster）
//   - wgpu_3d：WgpuBackend3D / PreparedDraw3D / Frame3DData（3D MVE 后端）
//   - scene_callback：SceneCompositorCallback（唯一 egui_wgpu 回调，一次合成全部图层，feature = "egui-wgpu"）
//   - egui_2d：EguiPainter + color_to_egui（Painter2D 的 egui 实现，feature = "cpu-2d"）
//   - eframe_backend：EframeWgpuBackend（RenderBackend 的 egui-wgpu 实现）
//   - svg_recorder：SVG 录制后端（预留空壳）
//   - text：文本引擎（Typst/LaTeX，预留空壳，feature = "text"）

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
// S3-2 修复：合并 2D/3D 为**单个**回调，一次性合成全部图层。
// （`CallbackResources` 是全局 TypeMap，每层一个 callback 会互相覆盖合成资源。）
#[cfg(feature = "egui-wgpu")]
pub mod scene_callback;

// CPU 2D 管线（编译期 feature `cpu-2d`）：`Painter2D` 的 egui 就地回放实现。
// 是否真的走该路径由运行期开关 `xcal_egui::UiConfig::gpu_2d` 决定（见 host::present）。
#[cfg(feature = "cpu-2d")]
pub mod egui_2d;

// 旧 svg_recorder / text 空壳已删除（见上注）。
// 下游若有使用，改为：xc-prerender::engine → xc-content::ContentIR → xcal-render::ir_to_draw_cmds。

// ─── re-export ────────────────────────────────────────────────────────────

#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use render_context::{DEFAULT_TARGET_FORMAT, RenderContext};
#[cfg(any(feature = "wgpu", feature = "egui-wgpu"))]
pub use compositor::{CompositeFrame, CompositeOp, Compositor};
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
pub use eframe_backend::EframeWgpuBackend;
#[cfg(feature = "egui-wgpu")]
pub use scene_callback::{SceneCompositorCallback, SceneCompositorFrame, SceneLayer};

#[cfg(feature = "cpu-2d")]
pub use egui_2d::{EguiPainter, color_to_egui};
