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
// # 离屏目标（RC.1 起）
//
// `WgpuBackend2D` / `WgpuBackend3D` 现在都按 [`LayerId`] 分别持有离屏目标
// （`HashMap<LayerId, ..>`），本帧**同类图层不再有数量上限**——这是 S3-3
// （2D/3D 互为纹理）的前提，也是同一类多个图层各自成像的基础。

use xcal_scene::domain::{LayerBlend, LayerId, LayerKind};
use xcal_render::{DrawQueue, DrawQueue3D, MorphItem, Viewport};
use xcal_render::draw_queue_3d::{LineDraw, MeshDraw};
use xc_geometry::{Rect2D, Vec2 as GeoVec2};

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

/// 内容纹理层**专用**的 2D 后端资源。
///
/// 与 [`WgpuBackend2D`] 是同一类型的**独立实例**。原因：`prepare` 同一帧会对 2D
/// 后端跑两次 `finish_frame`（趟 1 内容层、趟 2 视口层），而 `WgpuBackend2D` 的
/// `UniformArena` 在 `finish_frame` 里 `begin_frame()` 重置、`flush()` 从 offset 0
/// 重写——共用实例时后一次的 uniform 会覆盖前一次；又因两次 pass 都录进同一个
/// `egui_encoder`、稍后才 `submit`，内容层的 pass 在 GPU 上实际读到的是视口层的
/// uniform（clip 矩阵 / 颜色错乱），表现为"关掉网格（视口层无 draw → 不写 uniform）
/// 才显示正确内容"。独立实例从结构上隔离两类的 uniform arena。
///
/// 惰性创建：首次遇到内容层时才 `insert`，纯 2D / 纯 3D 场景不产生额外开销。
#[cfg(feature = "egui-wgpu")]
struct ContentBackend2D(WgpuBackend2D);

/// 唯一回调：`prepare` 渲染全部图层并一次性合成，`paint` 只做一次混合。
#[cfg(feature = "egui-wgpu")]
pub struct SceneCompositorCallback {
    /// 视口矩形（egui points，屏幕坐标，原点左上）。
    viewport_points: Rect2D,
    /// 本帧图层（**有效序**，即翻转开关已生效后的顺序）。
    layers: Vec<SceneLayer>,
    /// 2D 队列快照（仅当有效序中存在 2D 图层时才采集）。
    draws_2d: Vec<Recorded2D>,
    /// 内容纹理层（S3-3）的录制快照，按 `LayerId` 索引。
    ///
    /// 内容层是**纹理源**：`prepare` 先把它渲染到 `size_px` 大小的独立纹理，
    /// 再 `register_texture` 给 3D 材质采样（见 D1–D4）。
    content: Vec<(LayerId, Vec<Recorded2D>)>,
    /// 3D mesh / line 快照（仅当有效序中存在 3D 图层时才采集）。
    meshes_3d: Vec<MeshDraw>,
    lines_3d: Vec<LineDraw>,
    /// 形变项快照（S4.3）：`from`/`to` 常量 + 逐帧 `t`，由 2D 图层用形变管线渲染。
    morphs: Vec<MorphItem>,
}

#[cfg(feature = "egui-wgpu")]
impl SceneCompositorCallback {
    /// 从视口矩形、图层声明（有效序）与两个绘制队列构造。
    ///
    /// 只快照**有效序里真的存在**的那一类队列（S4.1 后仅内容纹理层那个例外），
    /// 避免白拷一份用不到的几何。
    pub fn new(
        viewport_points: Rect2D,
        layers: Vec<SceneLayer>,
        queue_2d: &DrawQueue,
        queue_3d: &DrawQueue3D,
        content: Vec<(LayerId, Vec<Recorded2D>)>,
        morphs: Vec<MorphItem>,
    ) -> Self {
        let needs_2d = layers.iter().any(|l| matches!(l.kind, LayerKind::Viewport2D { .. }));
        let needs_3d = layers.iter().any(|l| matches!(l.kind, LayerKind::Viewport3D { .. }));
        Self {
            viewport_points,
            draws_2d: if needs_2d { snapshot_queue(queue_2d) } else { Vec::new() },
            content,
            meshes_3d: if needs_3d { queue_3d.meshes().to_vec() } else { Vec::new() },
            lines_3d: if needs_3d { queue_3d.lines().to_vec() } else { Vec::new() },
            // 形变项属于 2D 图层；没有 2D 图层时无处渲染，直接丢弃。
            morphs: if needs_2d { morphs } else { Vec::new() },
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
        queue: &wgpu::Queue,
        screen_descriptor: &egui_wgpu::ScreenDescriptor,
        egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
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

        // ── 趟 1：先把全部内容纹理层渲染到各自纹理，并注册给 3D 后端 ──
        //
        // 必须早于视口层：3D 视口层在 prepare_draws 里按 `TextureHandle::id` 解析
        // 纹理视图，此刻该视图必须已注册（见计划 D2 / S4）。因为两趟都录进同一个
        // `egui_encoder`，内容 pass 在 GPU 上先于 3D pass 执行。
        for layer in &self.layers {
            let LayerKind::ContentTexture {
                size_px,
                texture,
                composite,
                ..
            } = &layer.kind
            else {
                continue;
            };
            let Some(draws) = self
                .content
                .iter()
                .find(|(id, _)| *id == layer.id)
                .map(|(_, d)| d)
            else {
                continue;
            };

            // 内容纹理按 **1:1 物理像素**渲染：几何视口 == 纹理尺寸、ppp = 1，
            // 于是内容相机输出的"屏幕像素"直接就是纹理像素（见 D1）。
            let geometry = Frame2DGeometry {
                viewport_points: Rect2D::from_min_size(
                    GeoVec2::ZERO,
                    GeoVec2::new(size_px[0] as f32, size_px[1] as f32),
                ),
                pixels_per_point: 1.0,
                screen_size_px: *size_px,
            };
            // 内容层走**独立**的 2D 后端实例：与趟 2 的视口层隔离 uniform arena，
            // 避免同帧两次 `finish_frame` 互相覆盖（见 [`ContentBackend2D`]）。
            if callback_resources.get::<ContentBackend2D>().is_none() {
                callback_resources.insert(ContentBackend2D(WgpuBackend2D::new(device, queue)));
            }
            let Some(frame) = callback_resources
                .get_mut::<ContentBackend2D>()
                .map(|cb| {
                    let backend = &mut cb.0;
                    backend.begin_frame_at(geometry);
                    for d in draws {
                        backend.push(&d.shape, &d.transform, &d.style, d.z);
                    }
                    backend.finish_frame(egui_encoder, layer.id, &geometry)
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

            // 注册给 3D 后端：会清空其材质 bind group 缓存，本帧视口层重建时
            // 即拿到新视图。无 3D 后端（纯 2D 场景）时静默跳过。
            if let Some(backend_3d) = callback_resources.get::<WgpuBackend3D>() {
                backend_3d.register_texture(texture.id(), offscreen.clone());
            }

            // 默认 `composite = false`：内容层只作纹理源，不参与屏幕合成。
            if *composite {
                ops.push(CompositeOp {
                    texture: offscreen,
                    src_format: OFFSCREEN_FORMAT,
                    blend: layer.blend,
                    opacity: layer.opacity,
                    rect_px: (left, top, w_pt, h_pt),
                });
            }
        }

        // ── 趟 2：按声明序渲染视口层 ──
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
                            // S4.3：形变项随 2D 图层走形变管线（`t` 走 uniform）。
                            for m in &self.morphs {
                                backend.push_morph(m.clone());
                            }
                            backend.finish_frame(egui_encoder, layer.id, &geometry)
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
                        layer.id,
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
                // 内容纹理层已在趟 1 处理（除非 composite=true 已进 ops）——
                // 它不是视口层，趟 2 跳过。
                LayerKind::ContentTexture { .. } => continue,
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

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "egui-wgpu"))]
mod tests {
    use super::*;
    use crate::render_context::RenderContext;
    use xcal_render::ZIndex;
    use xcal_render::components::FillSolid;
    use xcal_scene::scene::components::{
        Camera3D, Material3D, MeshGeometry, PrimitiveTopology, Projection, TextureHandle,
    };
    use xcal_scene::types::{Camera2D, Rect};
    use xc_geometry::Color32;

    /// 内容纹理尺寸（与 3D 视口同尺寸，便于逐像素对应）。
    const TEX: u32 = 32;
    /// 内容纹理句柄 id（3D 材质与内容层共用）。
    const CONTENT_ID: u64 = 9;
    const CONTENT_LAYER: LayerId = LayerId(1);
    const VIEW_3D_LAYER: LayerId = LayerId(2);
    /// 目标格式 `Bgra8UnormSrgb` 字节序下的纯红。
    const RED_BGRA: [u8; 4] = [0, 0, 255, 255];
    /// 源格式 `Rgba8Unorm` 字节序下的纯红。
    const RED_RGBA: [u8; 4] = [255, 0, 0, 255];

    /// 回读纹理像素（`width * 4` 按 256 对齐补行距）。
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
            label: Some("test-content-readback"),
            size: (padded * height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-content-readback-encoder"),
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

    /// 取像素 `(x, y)` 的 4 字节（左上角为原点）。
    fn pixel_at(pixels: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        pixels[i..i + 4].try_into().expect("pixel bytes")
    }

    /// **S6 · 能证伪**：内容纹理层必须在 3D 视口层**之前**完成注册与渲染。
    ///
    /// 构造：一张填满纯红的内容纹理（`ContentTexture` 层）+ 一个全屏四边形，
    /// 其材质按 `TextureHandle::new(CONTENT_ID)` 采样该纹理（Lambert，法线
    /// `+Z` 与固定光 `LIGHT_DIR=(0,0,1)` 同向 ⇒ `lit = 1`，输出即纹理本身）。
    ///
    /// 断言：3D 离屏中心像素等于内容纹理的红色。
    /// **证伪方式**：把 `prepare` 的两趟顺序对调（先渲染视口层再渲染内容层）——
    /// 3D 层解析材质纹理时该 id 尚未注册，回退到 1×1 透明兜底纹理 ⇒ 预乘 over
    /// 下退化为材质色（白），中心像素为白而非红，本断言失败。
    ///
    /// 注意 `layers` **刻意**把内容层声明在 3D 视口之后：正确性来自两趟结构，
    /// 不依赖声明序（R2）。
    #[test]
    fn content_layer_before_viewport_3d() {
        let Ok(ctx) = RenderContext::headless() else {
            eprintln!("no GPU adapter, skipping");
            return;
        };
        let device = ctx.device();
        let queue = ctx.queue();

        let mut resources = egui_wgpu::CallbackResources::new();
        resources.insert(WgpuBackend2D::new(device, queue));
        resources.insert(WgpuBackend3D::with_default_shader(
            device,
            queue,
            ctx.target_format(),
        ));
        resources.insert(Compositor::new(device, ctx.target_format()));

        // ── 内容层：整张红底 ───────────────────────────────────────────
        let mut content_queue = DrawQueue::new();
        content_queue.rect_fill(
            ZIndex::new(0),
            Rect::new((0.0, 0.0).into(), (TEX as f32, TEX as f32).into()),
            FillSolid::new(Color32::RED),
        );
        let content = vec![(CONTENT_LAYER, snapshot_queue(&content_queue))];

        // ── 3D 层：全屏四边形，材质引用内容纹理 ────────────────────────
        let quad = MeshGeometry::new(
            vec![
                glam::Vec3::new(-1.0, -1.0, 0.0),
                glam::Vec3::new(1.0, -1.0, 0.0),
                glam::Vec3::new(1.0, 1.0, 0.0),
                glam::Vec3::new(-1.0, 1.0, 0.0),
            ],
            Some(vec![0, 1, 2, 0, 2, 3]),
            PrimitiveTopology::Triangles,
        )
        .with_uvs(vec![
            glam::Vec2::new(0.0, 1.0),
            glam::Vec2::new(1.0, 1.0),
            glam::Vec2::new(1.0, 0.0),
            glam::Vec2::new(0.0, 0.0),
        ])
        .with_normals(vec![glam::Vec3::Z; 4]);
        let mut material = Material3D::color(Color32::WHITE);
        material.texture = Some(TextureHandle::new(CONTENT_ID));
        let mut queue_3d = DrawQueue3D::new();
        queue_3d.draw_mesh(std::sync::Arc::new(quad), glam::Mat4::IDENTITY, material);

        // 正交相机：垂直半高 1.0、aspect 1.0 ⇒ 覆盖世界 x/y ∈ [-1,1]，
        // 与四边形的世界范围完全对齐（视口 32×32，aspect = 1.0）。
        let camera = Camera3D::new(
            glam::Vec3::new(0.0, 0.0, 5.0),
            glam::Vec3::ZERO,
            glam::Vec3::Y,
            Projection::orthographic(1.0, 1.0, 0.1, 100.0),
        );

        let layers = vec![
            SceneLayer {
                id: VIEW_3D_LAYER,
                kind: LayerKind::Viewport3D { camera },
                blend: LayerBlend::Alpha,
                opacity: 1.0,
            },
            SceneLayer {
                id: CONTENT_LAYER,
                kind: LayerKind::ContentTexture {
                    camera: Camera2D::new(
                        (TEX as f32 / 2.0, TEX as f32 / 2.0).into(),
                        1.0,
                        Rect::new((0.0, 0.0).into(), (TEX as f32, TEX as f32).into()),
                    ),
                    size_px: [TEX, TEX],
                    texture: TextureHandle::new(CONTENT_ID),
                    composite: false,
                },
                blend: LayerBlend::Alpha,
                opacity: 1.0,
            },
        ];

        let callback = SceneCompositorCallback::new(
            Rect2D::from_min_size(GeoVec2::ZERO, GeoVec2::new(TEX as f32, TEX as f32)),
            layers,
            &DrawQueue::new(),
            &queue_3d,
            content,
            Vec::new(),
        );

        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [TEX, TEX],
            pixels_per_point: 1.0,
        };
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("test-content-before-3d"),
        });
        let _ = egui_wgpu::CallbackTrait::prepare(
            &callback,
            device,
            queue,
            &screen,
            &mut encoder,
            &mut resources,
        );
        ctx.submit(encoder.finish());
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll 失败");

        // 内容纹理本身（`Rgba8Unorm`）中心为纯红 ⇒ 录制/渲染路径正确。
        let tex_2d = resources
            .get::<ContentBackend2D>()
            .map(|cb| &cb.0)
            .and_then(|b| b.offscreen_texture(CONTENT_LAYER))
            .expect("内容纹理应已创建");
        let content_pixels = read_rgba8(device, queue, tex_2d, TEX, TEX);
        assert_eq!(
            pixel_at(&content_pixels, TEX, TEX / 2, TEX / 2),
            RED_RGBA,
            "内容纹理中心应为纯红"
        );

        // 3D 离屏（`Bgra8UnormSrgb`）中心像素 = 内容纹理的红 ⇒ 证明内容层
        // 在 3D 层渲染**之前**已完成注册（否则采到 1×1 透明兜底纹理，
        // 结果为材质色而非红）。
        let tex_3d = resources
            .get::<WgpuBackend3D>()
            .and_then(|b| b.offscreen_texture(VIEW_3D_LAYER))
            .expect("3D 离屏纹理应已创建");
        let pixels = read_rgba8(device, queue, &tex_3d, TEX, TEX);
        assert_eq!(
            pixel_at(&pixels, TEX, TEX / 2, TEX / 2),
            RED_BGRA,
            "3D 曲面中心应采样到内容纹理（红）⇒ 内容层先于 3D 层注册"
        );
    }
}
