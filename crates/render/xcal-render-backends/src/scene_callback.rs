// scene_callback.rs
//
// S3-2 修复：**一个 callback 合成所有图层**。
//
// # 为什么必须合并成一个回调
//
// `egui_wgpu::CallbackResources` 是**全局共享**的 `TypeMap`（`HashMap<TypeId, Box<dyn Any>>`），
// **不是 per-callback 的**。旧实现里 `Layer2DCallback` 与 `Layer3DCallback` 各自
// `insert::<CompositeFrame>(...)`（2D 帧 / 3D 帧）—— 后插入覆盖先插入，于是两个
// `paint()` 都 `get::<CompositeFrame>()` 拿到**同一帧**，把同一张离屏纹理画两遍：
//
// - 声明序 2D→3D：prepare 最后插入的是 3D 帧 → 两处都画 3D（2D 全无）
// - 声明序 3D→2D：prepare 最后插入的是 2D 帧 → 两处都画 2D（3D 全无）
//
// 合成本来就是"一次性消费全部图层"的操作，结构上就不该"每层一个 callback"。
// 本模块用**单个** `SceneCompositorCallback` 承载本帧全部图层：
// `prepare` 逐层渲染到各自的离屏纹理 → 收集全部 `CompositeOp`（保持声明序）
// → **一次** `Compositor::prepare` → 单个 `CompositeFrame` → `paint` 一次
// `composite_layers`。于是 `TypeMap` 里的合成条目**只有一处**，寻址冲突从结构上消失，
// 同时顺带消灭了"同帧双画 → 边缘双重描边加粗"。
//
// # 离屏目标的容量限制（本阶段口径）
//
// `WgpuBackend2D` / `WgpuBackend3D` 各自只有**一份**离屏目标
// （`targets: Option<..>` / `offscreen: Mutex<Option<..>>`），所以本帧
// **每类最多一个图层**（`prepare` 里有 `debug_assert`）。按 `LayerId` 分别持有
// 离屏目标属 S3-3（2D/3D 互为纹理）的事，本阶段不扩范围。

use xcal_scene::domain::{LayerBlend, LayerId, LayerKind};
use xcal_render::{DrawQueue, DrawQueue3D, Viewport};
use xcal_render::draw_queue_3d::{LineDraw, MeshDraw};
use xc_geometry::Rect2D;

use crate::compositor::{CompositeFrame, CompositeOp, Compositor};
use crate::wgpu_2d::{
    Frame2DData, Frame2DGeometry, OFFSCREEN_FORMAT, Recorded2D, WgpuBackend2D, snapshot_queue,
};
use crate::wgpu_3d::WgpuBackend3D;

/// 本帧一个图层的声明（有效序，由 `EguiSceneHost::present` 排好）。
///
/// 只带合成所需的最小信息：`LayerKind`（含相机，用于 3D 的 view-projection）
/// + 混合方式 + 不透明度。
#[derive(Debug, Clone)]
pub struct SceneLayer {
    /// 层标识（每层唯一，见 `build_scene_view`）。
    pub id: LayerId,
    /// 层类型（2D 视口 / 3D 视口）。
    pub kind: LayerKind,
    /// 与下层的混合方式。
    pub blend: LayerBlend,
    /// 不透明度（[0,1]）。
    pub opacity: f32,
}

/// 一帧合成结果 + 保活句柄。
///
/// **保活纪律**：旧的 `Frame2DData` / `Layer3DFrame` 靠自己进 `TypeMap` 来保证
/// 各自 uniform arena buffer 活到 `paint`；合并后两个帧结构都不再入表，
/// 所以它们的 `_arena_buffer` 必须挂在这里 —— 否则 `prepare` 一结束
/// buffer 就被析构，而 `CompositeFrame` 的 bind group 还引用着写在它上面的 uniform。
pub struct SceneCompositorFrame {
    /// 单个合成帧（层序 = 声明序）。
    pub frame: CompositeFrame,
    /// 保活：本帧全部图层 uniform arena buffer（2D / 3D 各一）。
    pub _keep_alive: Vec<wgpu::Buffer>,
}

/// 唯一回调：`prepare` 渲染全部图层并一次性合成，`paint` 只做一次混合。
#[cfg(feature = "egui-wgpu")]
pub struct SceneCompositorCallback {
    /// 视口矩形（egui points，屏幕坐标，原点左上）。
    viewport_points: Rect2D,
    /// 本帧图层（**有效序**，即翻转开关已生效后的顺序）。
    layers: Vec<SceneLayer>,
    /// 2D 队列快照（仅当有效序中存在 2D 图层时才采集）。
    draws_2d: Vec<Recorded2D>,
    /// 3D mesh / line 快照（仅当有效序中存在 3D 图层时才采集）。
    meshes_3d: Vec<MeshDraw>,
    lines_3d: Vec<LineDraw>,
}

#[cfg(feature = "egui-wgpu")]
impl SceneCompositorCallback {
    /// 从视口矩形、图层声明（有效序）与两个绘制队列构造。
    ///
    /// 只快照**有效序里真的存在**的那一类队列，避免 `gpu_2d = false`（2D 走 CPU
    /// 回放）时白拷一份 2D 几何。
    pub fn new(
        viewport_points: Rect2D,
        layers: Vec<SceneLayer>,
        queue_2d: &DrawQueue,
        queue_3d: &DrawQueue3D,
    ) -> Self {
        let needs_2d = layers.iter().any(|l| matches!(l.kind, LayerKind::Viewport2D { .. }));
        let needs_3d = layers.iter().any(|l| matches!(l.kind, LayerKind::Viewport3D { .. }));
        Self {
            viewport_points,
            draws_2d: if needs_2d { snapshot_queue(queue_2d) } else { Vec::new() },
            meshes_3d: if needs_3d { queue_3d.meshes().to_vec() } else { Vec::new() },
            lines_3d: if needs_3d { queue_3d.lines().to_vec() } else { Vec::new() },
            layers,
        }
    }

    /// 本帧图层数（有效序）。
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }
}

#[cfg(feature = "egui-wgpu")]
impl egui_wgpu::CallbackTrait for SceneCompositorCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        debug_assert!(
            self.layers
                .iter()
                .filter(|l| matches!(l.kind, LayerKind::Viewport2D { .. }))
                .count()
                <= 1
                && self
                    .layers
                    .iter()
                    .filter(|l| matches!(l.kind, LayerKind::Viewport3D { .. }))
                    .count()
                    <= 1,
            "每类最多一个图层：2D/3D 后端各只有一份离屏目标（per-LayerId 目标属 S3-3）"
        );

        let ppp = screen_descriptor.pixels_per_point;
        let screen_size_px = screen_descriptor.size_in_pixels;
        let (left, top, w_pt, h_pt) = (
            self.viewport_points.min.x,
            self.viewport_points.min.y,
            self.viewport_points.max.x - self.viewport_points.min.x,
            self.viewport_points.max.y - self.viewport_points.min.y,
        );

        // 逐层渲染到各自离屏纹理，按**声明序**收集 CompositeOp。
        let mut ops: Vec<CompositeOp> = Vec::with_capacity(self.layers.len());
        let mut keep_alive: Vec<wgpu::Buffer> = Vec::with_capacity(self.layers.len());

        for layer in &self.layers {
            match &layer.kind {
                LayerKind::Viewport2D { .. } => {
                    let geometry = Frame2DGeometry {
                        viewport_points: self.viewport_points,
                        pixels_per_point: ppp,
                        screen_size_px,
                    };
                    // 可变借用在此块内结束，后面才能不可变借用 3D 后端。
                    let Some(frame) = callback_resources
                        .get_mut::<WgpuBackend2D>()
                        .map(|backend| {
                            backend.begin_frame_at(geometry);
                            for d in &self.draws_2d {
                                backend.push(&d.shape, &d.transform, &d.style, d.z);
                            }
                            backend.finish_frame(egui_encoder, &geometry)
                        })
                    else {
                        continue;
                    };
                    let Frame2DData {
                        offscreen,
                        _arena_buffer,
                        ..
                    } = frame;
                    keep_alive.push(_arena_buffer);
                    ops.push(CompositeOp {
                        texture: offscreen,
                        src_format: OFFSCREEN_FORMAT,
                        blend: layer.blend,
                        opacity: layer.opacity,
                        rect_px: (left, top, w_pt, h_pt),
                    });
                }
                LayerKind::Viewport3D { camera } => {
                    let Some(backend) = callback_resources.get::<WgpuBackend3D>() else {
                        continue;
                    };
                    // 离屏纹理按**物理像素**建（逻辑点 × ppp）。
                    let viewport = Viewport::new(
                        0,
                        0,
                        ((w_pt * ppp).round() as u32).max(1),
                        ((h_pt * ppp).round() as u32).max(1),
                    );
                    let view_proj = camera.view_projection(viewport.aspect());
                    let frame = backend.render_layer_offscreen(
                        egui_encoder,
                        view_proj,
                        viewport,
                        &self.meshes_3d,
                        &self.lines_3d,
                    );
                    keep_alive.push(frame._arena_buffer);
                    ops.push(CompositeOp {
                        texture: frame.offscreen,
                        src_format: frame.format,
                        blend: layer.blend,
                        opacity: layer.opacity,
                        rect_px: (left, top, w_pt, h_pt),
                    });
                }
            }
        }

        // **一次**合成：全部图层进同一个 CompositeFrame（层序 = 声明序，不重排）。
        let Some(compositor) = callback_resources.get::<Compositor>() else {
            return Vec::new();
        };
        let frame = compositor.prepare(device, &ops);
        callback_resources.insert(SceneCompositorFrame {
            frame,
            _keep_alive: keep_alive,
        });
        Vec::new()
    }

    fn paint(
        &self,
        info: egui::epaint::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        let (Some(compositor), Some(scene)) = (
            callback_resources.get::<Compositor>(),
            callback_resources.get::<SceneCompositorFrame>(),
        ) else {
            return;
        };
        // 裁剪到回调区域，避免图层内容覆盖 egui 面板。
        let clip = info.clip_rect_in_pixels();
        render_pass.set_scissor_rect(
            clip.left_px.max(0) as u32,
            clip.top_px.max(0) as u32,
            clip.width_px.max(0) as u32,
            clip.height_px.max(0) as u32,
        );
        compositor.composite_layers(render_pass, &scene.frame);
    }
}
