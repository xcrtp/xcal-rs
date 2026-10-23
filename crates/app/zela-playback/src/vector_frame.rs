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

use ze_scene_core::domain::LayerDescriptor;
use ze_scene_core::LayerKind;
use zela_render::draw::DrawQueue;
use zela_render_backends::routing::FrameContent;
use zela_render_backends::Recorded2D;
use zela_render_vector::Light;

use crate::Runtime;

/// 一帧的内容快照：路由读它的特征，矢量渲染复用同一份数据。
pub struct VectorFrame {
    layers: Vec<LayerDescriptor>,
    draws: Vec<Recorded2D>,
    lights: Vec<Light>,
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
                let lights =
                    crate::vector3d::lights(scheduler.resources().get::<zela_render::LightQueue>());
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
                    lights,
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
            has_2d_primitives: self.draws.len(),
            has_meshes: self.mesh_count,
            has_line_meshes: self.has_line_meshes,
            has_content_texture,
        }
    }

    /// 用矢量后端渲染当前帧，返回 RGBA8。
    ///
    /// 调用前必须已经 `step_frame`（或等价地推进过一帧）；
    /// 是否该用矢量由调用方按 [`Self::content`] + 路由决定（本函数不判断）。
    pub fn render(
        &self,
        runtime: &Runtime,
        width: u32,
        height: u32,
        params: &zela_scene_bundle::RenderParams,
        background: [f32; 4],
    ) -> Result<Vec<u8>, String> {
        let camera3d = self.layers.iter().find_map(|layer| match &layer.kind {
            LayerKind::Viewport3D { camera } => Some(*camera),
            _ => None,
        });
        match camera3d {
            Some(camera) => {
                let records = runtime
                    .driver
                    .with_scheduler(|scheduler| -> Result<_, String> {
                        let queue = scheduler.resources().get::<DrawQueue>();
                        match queue {
                            Some(queue) => crate::vector3d::mesh_records(queue),
                            None => Ok(Vec::new()),
                        }
                    })?;
                let mut view =
                    crate::vector3d::view3(&camera, (width, height), background, params)?;
                view.lights = self.lights.clone();
                zela_render_vector::render_3d_with(width, height, 4, &view, &records)
            }
            None => {
                let items: Vec<zela_render_vector::SceneItem<'_>> = self
                    .draws
                    .iter()
                    .map(|recorded| zela_render_vector::SceneItem {
                        shape: &recorded.shape,
                        transform: recorded.transform,
                        style: recorded.style,
                    })
                    .collect();
                zela_render_vector::render_scene_2d(width, height, 4, background, &items)
            }
        }
    }
}
