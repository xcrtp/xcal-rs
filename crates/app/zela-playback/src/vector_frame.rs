//! 一帧内容的**矢量渲染**与路由特征（B11 Step 4/5）。
//!
//! # 为什么在 playback
//!
//! native（`zela-app::frame_loop`）与浏览器（`zela-web`）都要做同一件事：
//! 「读调度器 → 判 3D/2D → 喂矢量后端」。`zela-app` 带 Lua / Typst，不能进浏览器；
//! 本 crate 是两边唯一的公共层 ⇒ 快照与渲染都放这里，避免两份实现漂移。
//!
//! # 口径
//!
//! - 快照只读**当前帧**（调用方已 `step_frame` 推进）；
//! - 3D 网格记录在渲染时再取（`mesh_records` 对线框会显式报错，必须在路由**之后**）。

use ze_notion::{Path2D, Vec2};
use ze_scene_core::domain::LayerDescriptor;
use ze_scene_core::LayerKind;
use zela_render::draw::DrawQueue;
use zela_render_backends::routing::FrameContent;
use zela_render_backends::Recorded2D;

use crate::Runtime;

/// 书写动画项（B14）：`path` 已由录制层换算到**屏幕空间**，`style` 的宽度也已是像素。
///
/// 矢量后端没有 reveal 管线 ⇒ 这里按**归一化弧长**把 `[start, end]` 裁成开放折线再描边。
/// 与 wgpu 的差别只有"按弧长预细分 + 着色器丢弃" vs "CPU 裁剪"，可见部分是同一段几何。
struct RevealSnapshot {
    path: Path2D,
    start: f32,
    end: f32,
    style: ze_render_ir::Style2D,
}

/// 一帧的内容快照：路由读它的特征，矢量渲染复用同一份数据。
pub struct VectorFrame {
    layers: Vec<LayerDescriptor>,
    draws: Vec<Recorded2D>,
    reveals: Vec<RevealSnapshot>,
    morph_count: usize,
    mesh_count: usize,
    has_line_meshes: bool,
}

impl VectorFrame {
    /// 在调度器借用期内取完（`DrawQueue` 不是 `Clone`）。
    pub fn take(runtime: &Runtime) -> Result<Self, String> {
        runtime
            .driver
            .with_scheduler(|scheduler| -> Result<_, String> {
                let view_options = zela_render::FrameOptions::default();
                let view = zela_render::frame::build_scene_view(
                    &zela_scene_ecs::contract::WorldView::new(scheduler.world()),
                    scheduler.resources(),
                    view_options,
                );
                let layers = view.layers.clone();
                let queue = scheduler.resources().get::<DrawQueue>();
                let draws = queue
                    .map(zela_render_backends::snapshot_queue)
                    .unwrap_or_default();
                // B14：reveal 与 morph 是**独立队列**（不在 `DrawQueue` 里）。
                // 只取 DrawQueue ⇒ 网格线 / 摆杆 / 轨迹弧整片消失（X4-01 的真实宽度）。
                let reveals: Vec<RevealSnapshot> = scheduler
                    .resources()
                    .get::<zela_render::RevealQueue>()
                    .map(|queue| {
                        queue
                            .0
                            .iter()
                            .map(|item| RevealSnapshot {
                                path: item.path.clone(),
                                start: item.start,
                                end: item.end,
                                style: item.style,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let morph_count = scheduler
                    .resources()
                    .get::<zela_render::MorphQueue>()
                    .map_or(0, |queue| queue.0.len());
                // ⚠️ B14：**不要在这里取灯**。`take()` 早于 3D 录制阶段运行，
                // 此时 `LightQueue` 资源还不存在（实测 `None`）⇒ 会静默退回默认灯。
                // 灯与网格一样，必须在 `render()`（渲染时）再取。
                let (mesh_count, has_line_meshes) = match queue {
                    Some(queue) => {
                        let mut count = 0usize;
                        let mut lines = false;
                        for mesh in queue.meshes() {
                            count += 1;
                            if mesh.geometry.topology
                                != ze_scene_core::components::PrimitiveTopology::Triangles
                            {
                                lines = true;
                            }
                        }
                        (count, lines)
                    }
                    None => (0, false),
                };
                Ok(Self {
                    layers,
                    draws,
                    reveals,
                    morph_count,
                    mesh_count,
                    has_line_meshes,
                })
            })
    }

    /// 路由输入：**只描述帧内容**（C1：不含负载 / 墙钟 / 随机数）。
    pub fn content(&self) -> FrameContent {
        let has_3d_camera = self
            .layers
            .iter()
            .any(|layer| matches!(layer.kind, LayerKind::Viewport3D { .. }));
        let has_content_texture = self.layers.iter().any(|layer| {
            !matches!(
                layer.kind,
                LayerKind::Viewport2D { .. } | LayerKind::Viewport3D { .. }
            )
        });
        FrameContent {
            has_3d_camera,
            // B14：reveal 也是 2D 图元（独立队列，但同样占画面）。
            has_2d_primitives: self.draws.len() + self.reveals.len(),
            has_meshes: self.mesh_count,
            has_line_meshes: self.has_line_meshes,
            has_content_texture,
            has_morphs: self.morph_count > 0,
        }
    }
}

/// 一帧的渲染输入：3D（view + 网格记录）或 2D（背景 + 记录），都已转成矢量后端的类型。
///
/// **B15**：同步 / 异步 / GPU 目标三条路径共用这一份准备，避免几何映射漂移。
enum Prepared {
    ThreeD {
        view: zela_render_vector::View3,
        records: Vec<zela_render_vector::PathRecord>,
    },
    TwoD {
        background: [f32; 4],
        records: Vec<zela_render_vector::PathRecord>,
    },
}

impl VectorFrame {
    fn prepare(
        &self,
        runtime: &Runtime,
        width: u32,
        height: u32,
        params: &zela_scene_bundle::RenderParams,
        background: [f32; 4],
    ) -> Result<Prepared, String> {
        let camera3d = self.layers.iter().find_map(|layer| match &layer.kind {
            LayerKind::Viewport3D { camera } => Some(*camera),
            _ => None,
        });
        match camera3d {
            Some(camera) => {
                // B14 / X4-02：网格与灯**都必须在渲染时取**（`take()` 太早，`LightQueue`
                // 那时还不存在 ⇒ 以前一直静默用默认灯，画面系统性偏暗）。
                let (records, lights) = runtime
                    .driver
                    .with_scheduler(|scheduler| -> Result<_, String> {
                        let lights = crate::vector3d::lights(
                            scheduler.resources().get::<zela_render::LightQueue>(),
                        );
                        let records = match scheduler.resources().get::<DrawQueue>() {
                            Some(queue) => crate::vector3d::mesh_records(queue)?,
                            None => Vec::new(),
                        };
                        Ok((records, lights))
                    })?;
                let mut view =
                    crate::vector3d::view3(&camera, (width, height), background, params)?;
                view.lights = lights;
                Ok(Prepared::ThreeD { view, records })
            }
            None => {
                let mut items: Vec<zela_render_vector::SceneItem<'_>> = self
                    .draws
                    .iter()
                    .map(|recorded| zela_render_vector::SceneItem {
                        shape: &recorded.shape,
                        transform: recorded.transform,
                        style: recorded.style,
                    })
                    .collect();
                // B14：wgpu 把 reveal 排在 DrawQueue 全部几何**之后**（同一 z 的稳定序）
                // ⇒ 这里也追加在末尾。样式宽度已是像素、路径已是屏幕空间（录制层换算过）。
                let reveal_items: Vec<(ze_render_ir::Shape2D, ze_render_ir::Style2D)> =
                    self.reveals
                        .iter()
                        .filter_map(|reveal| {
                            clip_reveal(&reveal.path, reveal.start, reveal.end)
                                .map(|shape| (shape, reveal.style))
                        })
                        .collect();
                items.extend(reveal_items.iter().map(|(shape, style)| {
                    zela_render_vector::SceneItem {
                        shape,
                        transform: ze_notion::Transform2D::identity(),
                        style: *style,
                    }
                }));
                // 与 `render_scene_2d` 同一条映射（只是把记录留下来给三条路径复用）。
                let records = zela_render_vector::scene_records(&items, (width, height))
                    .map_err(|error| error.to_string())?;
                Ok(Prepared::TwoD {
                    background,
                    records,
                })
            }
        }
    }

    /// 用矢量后端渲染当前帧，返回 RGBA8（native 同步读回）。
    ///
    /// 调用前必须已经 `step_frame`（或等价地推进过一帧）；
    /// 是否该用矢量由调用方按 [`Self::content`] + 路由决定（本函数不判断）。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render(
        &self,
        runtime: &Runtime,
        width: u32,
        height: u32,
        params: &zela_scene_bundle::RenderParams,
        background: [f32; 4],
    ) -> Result<Vec<u8>, String> {
        match self.prepare(runtime, width, height, params, background)? {
            Prepared::ThreeD { view, records } => {
                zela_render_vector::render_3d_with(width, height, 4, &view, &records)
            }
            Prepared::TwoD {
                background,
                records,
            } => zela_render_vector::render_2d(width, height, 4, background, &records),
        }
    }

    /// **B15（甲）**：浏览器上的同一帧（异步回读）。
    #[cfg(target_arch = "wasm32")]
    pub async fn render_async(
        &self,
        runtime: &Runtime,
        width: u32,
        height: u32,
        params: &zela_scene_bundle::RenderParams,
        background: [f32; 4],
    ) -> Result<Vec<u8>, String> {
        match self.prepare(runtime, width, height, params, background)? {
            Prepared::ThreeD { view, records } => {
                zela_render_vector::render_3d_with_async(width, height, 4, &view, &records).await
            }
            Prepared::TwoD {
                background,
                records,
            } => {
                zela_render_vector::render_2d_async(width, height, 4, background, &records).await
            }
        }
    }

    /// **B15（乙）**：把当前帧直接合成进 `target`（native 同步版），像素不经过 CPU。
    #[cfg(not(target_arch = "wasm32"))]
    pub fn render_to_texture(
        &self,
        runtime: &Runtime,
        target: &wgpu::Texture,
        width: u32,
        height: u32,
        params: &zela_scene_bundle::RenderParams,
        background: [f32; 4],
    ) -> Result<(), String> {
        match self.prepare(runtime, width, height, params, background)? {
            Prepared::ThreeD { view, records } => zela_render_vector::render_3d_to_texture(
                target, width, height, 4, &view, &records,
            ),
            Prepared::TwoD {
                background,
                records,
            } => zela_render_vector::render_2d_to_texture(
                target, width, height, 4, background, &records,
            ),
        }
    }

    /// **B15（乙）**：同 [`Self::render_to_texture`]，浏览器上的异步版。
    #[cfg(target_arch = "wasm32")]
    pub async fn render_to_texture_async(
        &self,
        runtime: &Runtime,
        target: &wgpu::Texture,
        width: u32,
        height: u32,
        params: &zela_scene_bundle::RenderParams,
        background: [f32; 4],
    ) -> Result<(), String> {
        match self.prepare(runtime, width, height, params, background)? {
            Prepared::ThreeD { view, records } => {
                zela_render_vector::render_3d_to_texture_async(
                    target, width, height, 4, &view, &records,
                )
                .await
            }
            Prepared::TwoD {
                background,
                records,
            } => {
                zela_render_vector::render_2d_to_texture_async(
                    target, width, height, 4, background, &records,
                )
                .await
            }
        }
    }
}

/// 把 reveal 的 `[start, end]`（归一化弧长）裁成一条**开放折线**（B14 / X4-01）。
///
/// 与 wgpu 的 reveal 管线同语义：几何按弧长参数化、区间外不可见；区别只是
/// wgpu 在着色器里丢弃，这里在 CPU 裁剪。`None` = 本帧没有可见段。
fn clip_reveal(path: &Path2D, start: f32, end: f32) -> Option<ze_render_ir::Shape2D> {
    let (start, end) = (start.clamp(0.0, 1.0), end.clamp(0.0, 1.0));
    if end <= start {
        return None;
    }
    let flat = path.flatten(0.25);
    if flat.len() < 2 {
        return None;
    }
    let mut cumulative = Vec::with_capacity(flat.len());
    let mut total = 0.0f32;
    cumulative.push(0.0);
    for pair in flat.windows(2) {
        total += (pair[1] - pair[0]).length();
        cumulative.push(total);
    }
    if total <= f32::EPSILON {
        return None;
    }
    let mut out: Vec<Vec2> = Vec::new();
    for i in 0..flat.len() - 1 {
        let (s0, s1) = (cumulative[i] / total, cumulative[i + 1] / total);
        if s1 <= start || s0 >= end {
            continue;
        }
        let span = (s1 - s0).max(f32::EPSILON);
        let t0 = ((start - s0) / span).clamp(0.0, 1.0);
        let t1 = ((end - s0) / span).clamp(0.0, 1.0);
        if out.is_empty() {
            out.push(flat[i] + (flat[i + 1] - flat[i]) * t0);
        }
        out.push(flat[i] + (flat[i + 1] - flat[i]) * t1);
    }
    if out.len() < 2 {
        return None;
    }
    Some(ze_render_ir::Shape2D::Path(Path2D::from_polyline(&out, false)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last_x(shape: &ze_render_ir::Shape2D) -> f32 {
        let ze_render_ir::Shape2D::Path(path) = shape else {
            panic!("reveal 裁剪结果应是 Path");
        };
        path.flatten(0.25).last().expect("至少一个点").x
    }

    /// 半程 reveal 的终点应落在路径中点（按弧长参数化，不是按顶点序号）。
    #[test]
    fn reveal_clip_keeps_the_parameterized_span() {
        let path = Path2D::from_polyline(&[Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0)], false);
        let full = clip_reveal(&path, 0.0, 1.0).expect("整段可见");
        assert!((last_x(&full) - 10.0).abs() < 1e-3, "整段终点应为 10");
        let half = clip_reveal(&path, 0.0, 0.5).expect("半段可见");
        assert!((last_x(&half) - 5.0).abs() < 1e-3, "半段终点应为 5，实测 {half:?}");
        // 区间倒置 / 空区间 ⇒ 不可见（不是"整段"）。
        assert!(clip_reveal(&path, 0.5, 0.5).is_none());
        assert!(clip_reveal(&path, 0.8, 0.2).is_none());
    }
}
