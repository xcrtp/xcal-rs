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

use ze_object::LayerKind;
use ze_render_ir::LayerId;
use zela_render::{
    DirectionalLightRecord, DrawQueue, DrawQueue3D, LightQueue, MorphItem, PointLightRecord,
    PostFilterChain, RevealItem, Viewport,
};
use zela_render::draw_queue_3d::{LineDraw, MeshDraw};
use ze_notion::{Rect2D, Vec2 as GeoVec2};

use crate::compositor::{CompositeFrame, CompositeOp, Compositor, PostBlit};
use crate::standard_backends::{BackendStore, ScreenParams};
use crate::wgpu_2d::{Frame2DData, Frame2DGeometry, OFFSCREEN_FORMAT, Recorded2D, snapshot_queue};

/// 本帧一个图层的声明（有效序，由宿主排好）。
///
/// 定义已上移到 `zela-render::frame::SceneLayer`（后端中立，避免后端依赖 `zela-scene`）。
pub use zela_render::SceneLayer;

/// 一帧合成结果 + 保活句柄。
///
/// **保活纪律**：旧的 `Frame2DData` / `Layer3DFrame` 靠自己进 `TypeMap` 来保证
/// 各自 uniform arena buffer 活到 `paint`；合并后两个帧结构都不再入表，
/// 所以它们的 `_arena_buffer` 必须挂在这里 —— 否则 `prepare` 一结束
/// buffer 就被析构，而 `CompositeFrame` 的 bind group 还引用着写在它上面的 uniform。
pub struct SceneCompositorFrame {
    /// 单个合成帧（层序 = 声明序）。
    pub frame: CompositeFrame,
    /// **C0/C1**：post 路径的滤镜链 blit 资源（空链时为 `None` ⇒ 走直通，画面逐位不变）。
    pub post_blit: Option<PostBlit>,
    /// 保活：本帧全部图层 uniform arena buffer（2D / 3D 各一）。
    pub _keep_alive: Vec<wgpu::Buffer>,
}

/// 内容纹理层**专用**的 2D 后端资源：已迁至
/// [`crate::standard_backends::ContentBackend2D`]（W1：它属于"后端装配"而非"回调"）。

/// 唯一回调（egui 路径）/ 一帧渲染请求（headless 路径，W1）。
///
/// **W1：本类型不再依赖 egui** —— 字段全是纯数据（`Rect2D` / 图层声明 / 队列快照 /
/// 滤镜链），故 headless 路径（[`crate::standard_backends::render_frame_headless`]）
/// 可以直接持有一个实例，与 egui 回调跑**同一份帧体**
/// （[`SceneCompositorCallback::prepare_frame`] + `paint_frame`）。
/// 只有 `impl egui_wgpu::CallbackTrait`（`prepare` / `paint`）仍门控在 `egui-wgpu` 下。
pub struct SceneCompositorCallback {
    /// 视口矩形（egui points，屏幕坐标，原点左上）。
    viewport_points: Rect2D,
    /// 本帧图层（**有效序**，即翻转开关已生效后的顺序）。
    layers: Vec<SceneLayer>,
    /// **C0–C2**：本帧的合成后滤镜链（空链 ⇒ 直通路径，不建中间纹理）。
    filters: PostFilterChain,
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
    /// B1-A：3D 光源快照（仅当有效序中存在 3D 图层时才采集）。
    /// 空 ⇒ 后端兜底默认光照（单方向光 (0,0,1) + ambient 0.2）。
    lights_3d: Vec<DirectionalLightRecord>,
    /// B1-B：3D **点光**快照（同样仅 3D 图层存在时采集；空 ⇒ 无点光）。
    points_3d: Vec<PointLightRecord>,
    /// 形变项快照（S4.3）：`from`/`to` 常量 + 逐帧 `t`，由 2D 图层用形变管线渲染。
    morphs: Vec<MorphItem>,
    /// 书写动画项快照（R0）：路径常量 + 逐帧区间 `[start, end]`，由 2D 图层渲染。
    reveals: Vec<RevealItem>,
}

impl SceneCompositorCallback {
    /// 从视口矩形、图层声明（有效序）、两个绘制队列与光源队列构造。
    ///
    /// 只快照**有效序里真的存在**的那一类队列（S4.1 后仅内容纹理层那个例外），
    /// 避免白拷一份用不到的几何。
    pub fn new(
        viewport_points: Rect2D,
        layers: Vec<SceneLayer>,
        queue_2d: &DrawQueue,
        queue_3d: &DrawQueue3D,
        lights: &LightQueue,
        content: Vec<(LayerId, Vec<Recorded2D>)>,
        morphs: Vec<MorphItem>,
        reveals: Vec<RevealItem>,
        filters: PostFilterChain,
    ) -> Self {
        let needs_2d = layers.iter().any(|l| matches!(l.kind, LayerKind::Viewport2D { .. }));
        let needs_3d = layers.iter().any(|l| matches!(l.kind, LayerKind::Viewport3D { .. }));
        Self {
            viewport_points,
            filters,
            draws_2d: if needs_2d { snapshot_queue(queue_2d) } else { Vec::new() },
            content,
            meshes_3d: if needs_3d { queue_3d.meshes().to_vec() } else { Vec::new() },
            lines_3d: if needs_3d { queue_3d.lines().to_vec() } else { Vec::new() },
            // B1-A：光源快照（仅 3D 图层存在时采集；空 ⇒ 后端默认光照）。
            lights_3d: if needs_3d {
                lights.directional.clone()
            } else {
                Vec::new()
            },
            // B1-B：点光快照（同样仅 3D 图层存在时采集）。
            points_3d: if needs_3d {
                lights.points.clone()
            } else {
                Vec::new()
            },
            // 形变项属于 2D 图层；没有 2D 图层时无处渲染，直接丢弃。
            morphs: if needs_2d { morphs } else { Vec::new() },
            // 书写动画项同理（R0）。
            reveals: if needs_2d { reveals } else { Vec::new() },
            layers,
        }
    }

    /// 本帧图层数（有效序）。
    pub fn layer_count(&self) -> usize {
        self.layers.len()
    }

    /// 本帧视口矩形（points）。
    ///
    /// headless 入口据此**自建同等尺寸的目标纹理**（egui 那边由宿主设 viewport）。
    pub fn viewport_points(&self) -> Rect2D {
        self.viewport_points
    }

    /// **诊断用**：本帧的**结构指纹**（只含计数 / 拓扑序，不含任何浮点数值）。
    ///
    /// 跨运行时对拍的**第一层**判据：必须逐位相同；不同即为逻辑错误，
    /// 不能归入舍入容差。
    #[doc(hidden)]
    pub fn debug_structure(&self) -> String {
        fn fnv(text: &str) -> u64 {
            let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
            for byte in text.as_bytes() {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
            hash
        }
        fn layer_kind(kind: &ze_object::LayerKind) -> &'static str {
            match kind {
                ze_object::LayerKind::Viewport2D { .. } => "v2d",
                ze_object::LayerKind::Viewport3D { .. } => "v3d",
                ze_object::LayerKind::ContentTexture { .. } => "ct",
            }
        }

        let mut topo = String::new();
        for layer in &self.layers {
            topo.push_str(layer_kind(&layer.kind));
            topo.push(';');
        }
        topo.push('|');
        for draw in &self.draws_2d {
            let space = match draw.style.space {
                ze_notion::MeasureSpace::World => "W",
                ze_notion::MeasureSpace::Screen => "S",
                ze_notion::MeasureSpace::WorldWithMinPx(_) => "WM",
            };
            // 结构层**不含浮点**：`draw.z` 是 f32，只影响绘制顺序；
            // 顺序本身已由队列迭代序 + 本串顺序表达，故不写入数值。
            topo.push_str(&format!(
                "{}:{}{}{};",
                draw.shape.kind_name(),
                if draw.style.fill.is_some() { "f" } else { "-" },
                if draw.style.stroke.is_some() { "s" } else { "-" },
                space
            ));
        }

        format!(
            "layers={};draws={};meshes={};lines={};morphs={};reveals={};topo={:016x}",
            self.layers.len(),
            self.draws_2d.len(),
            self.meshes_3d.len(),
            self.lines_3d.len(),
            self.morphs.len(),
            self.reveals.len(),
            fnv(&topo)
        )
    }

    /// **诊断用**：本帧 CPU 侧绘制内容的结构指纹（不含 GPU 光栅化结果）。
    ///
    /// 用于跨运行时定位差异：若 native 与浏览器的指纹相同而像素不同，
    /// 差异来自 GPU 后端（MSAA 采样位置 / 精度），而非场景求值。
    #[doc(hidden)]
    pub fn debug_fingerprint(&self) -> String {
        let text = format!(
            "vp={:?};layers={:?};draws={:?};meshes={:?};lines={:?};morphs={:?};reveals={:?}",
            self.viewport_points,
            self.layers,
            self.draws_2d,
            self.meshes_3d,
            self.lines_3d,
            self.morphs,
            self.reveals
        );
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in text.as_bytes() {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("{hash:016x}")
    }

    /// **帧体（唯一实现）**：渲染全部图层到各自离屏纹理 → 一次合成 → 产出
    /// [`SceneCompositorFrame`]。
    ///
    /// egui 路径（`CallbackTrait::prepare`）与 headless 路径
    /// （[`crate::standard_backends::render_frame_headless`]）**都调本函数**
    /// ⇒ 两条路径逐位一致是**结构保证**。
    ///
    /// 后端经 [`BackendStore`] 取用（egui = TypeMap，headless = 自持有），
    /// 故帧体本身不认识任何一种宿主。
    pub fn prepare_frame(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        screen: ScreenParams,
        store: &mut impl BackendStore,
    ) -> SceneCompositorFrame {
        let ppp = screen.pixels_per_point;
        let screen_size_px = screen.size_in_pixels;
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
        // encoder，内容 pass 在 GPU 上先于 3D pass 执行。
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
            // 避免同帧两次 `finish_frame` 互相覆盖（见 `ContentBackend2D`）。
            let frame = {
                let backend = store.content_backend_2d(device, queue);
                backend.begin_frame_at(geometry);
                for d in draws {
                    backend.push(&d.shape, &d.transform, &d.style, d.z);
                }
                backend.finish_frame(encoder, layer.id, &geometry)
            };

            let Frame2DData {
                offscreen,
                _arena_buffer,
                ..
            } = frame;
            keep_alive.push(_arena_buffer);

            // 注册给 3D 后端：会清空其材质 bind group 缓存，本帧视口层重建时
            // 即拿到新视图。无 3D 后端（纯 2D 场景）时静默跳过。
            if let Some(backend_3d) = store.backend_3d() {
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
                    let Some(frame) = store.backend_2d().map(|backend| {
                        backend.begin_frame_at(geometry);
                        for d in &self.draws_2d {
                            backend.push(&d.shape, &d.transform, &d.style, d.z);
                        }
                        // S4.3：形变项随 2D 图层走形变管线（`t` 走 uniform）。
                        for m in &self.morphs {
                            backend.push_morph(m.clone());
                        }
                        // R0：书写动画项随 2D 图层走（同一个 `pipeline_2d`，
                        // 区间 `[a,b]` 走 uniform）。
                        for r in &self.reveals {
                            backend.push_reveal(r.clone());
                        }
                        backend.finish_frame(encoder, layer.id, &geometry)
                    }) else {
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
                    // 离屏纹理按**物理像素**建（逻辑点 × ppp）。
                    let viewport = Viewport::new(
                        0,
                        0,
                        ((w_pt * ppp).round() as u32).max(1),
                        ((h_pt * ppp).round() as u32).max(1),
                    );
                    let view_proj = camera.view_projection(viewport.aspect());
                    let Some(frame) = store.backend_3d().map(|backend| {
                        backend.render_layer_offscreen(
                            encoder,
                            layer.id,
                            view_proj,
                            // B1-C：相机世界位置 → 场景级 uniform（PBR 视线向量用）。
                            camera.eye,
                            viewport,
                            &self.meshes_3d,
                            &self.lines_3d,
                            &self.lights_3d,
                            &self.points_3d,
                        )
                    }) else {
                        continue;
                    };
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
        //
        // C0/C1：滤镜链**非空**时走 post 路径 —— 先把全部图层合成到**中间全屏纹理**
        // （清屏透明），再由 `paint` 经滤镜链 blit 进主 pass；**空链走直通**（不建中间
        // 纹理、不多一次全屏 pass ⇒ 画面逐位不变）。
        let Some(compositor) = store.compositor() else {
            Self::report_soft_failures();
            return SceneCompositorFrame {
                frame: CompositeFrame::default(),
                post_blit: None,
                _keep_alive: keep_alive,
            };
        };
        // C3：管线惰性建 —— 先确保本帧需要的 (blend, 域转换) 组合就绪。
        let _ = compositor.ensure_for(device, &ops);
        let frame = compositor.prepare(device, &ops);

        let post_blit = if self.filters.needs_post() {
            // 视口物理像素尺寸（与 3D 离屏同口径：逻辑点 × ppp）。
            let size_px = (
                ((w_pt * ppp).round() as u32).max(1),
                ((h_pt * ppp).round() as u32).max(1),
            );
            let src = compositor.render_post(device, encoder, size_px, &frame);
            Some(compositor.build_post_blit(device, &self.filters, &src, size_px))
        } else {
            None
        };

        Self::report_soft_failures();
        SceneCompositorFrame {
            frame,
            post_blit,
            _keep_alive: keep_alive,
        }
    }

    /// 帧末汇报"软失败"（三角化失败而跳过的绘制数）。
    ///
    /// 口径见 [`crate::diagnostics`]：**软失败被接受**（不中断渲染、不改退出码），
    /// 但必须能事后发现 —— 每帧一行汇总，进程退出前另有累计值。
    fn report_soft_failures() {
        let skipped = crate::diagnostics::take_frame_skipped();
        if skipped > 0 {
            eprintln!(
                "[zela-render] 本帧 {skipped} 个绘制被跳过（三角化失败；已命名原因，见上方日志）"
            );
        }
    }

    /// **绘制帧体**：把上一段的合成结果画进当前 render pass（`paint` 的唯一实现）。
    ///
    /// 调用方负责在此之前设好 scissor / viewport：
    /// - egui 路径由宿主（egui-wgpu 为每个回调设 viewport，`paint` 里再收 scissor）；
    /// - headless 路径由 [`crate::standard_backends::render_frame_headless`] 设成
    ///   目标纹理整幅。
    pub fn paint_frame(
        &self,
        compositor: &Compositor,
        composed: &SceneCompositorFrame,
        pass: &mut wgpu::RenderPass<'_>,
    ) {
        // C0/C1：有滤镜链 ⇒ blit 中间纹理（经滤镜）；无 ⇒ 直通合成（画面逐位不变）。
        match &composed.post_blit {
            Some(blit) => compositor.blit_post(pass, blit),
            None => compositor.composite_layers(pass, &composed.frame),
        }
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
        use crate::standard_backends::TypeMapStore;

        // W1：本方法只是**唯一帧体**（[`SceneCompositorCallback::prepare_frame`]）的
        // egui 适配层 —— 把 `ScreenDescriptor` 译成 [`ScreenParams`]、把 TypeMap 包成
        // [`BackendStore`]、把产物写回 TypeMap 供 `paint` 取用。
        // 帧体本身不在这里 ⇒ headless 路径跑的是**同一份代码**。
        let frame = self.prepare_frame(
            device,
            queue,
            egui_encoder,
            ScreenParams::new(
                screen_descriptor.size_in_pixels,
                screen_descriptor.pixels_per_point,
            ),
            &mut TypeMapStore(callback_resources),
        );
        callback_resources.insert(frame);
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
        // W1：绘制帧体与 headless 路径**同一份实现**（view/scissor 由本方法设好）。
        self.paint_frame(compositor, scene, render_pass);
    }
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "egui-wgpu"))]
mod tests {
    use super::*;
    use ze_render_ir::LayerBlend;
    use crate::render_context::RenderContext;
    use crate::standard_backends::{StandardBackends, render_frame_headless};
    use crate::wgpu_3d::WgpuBackend3D;
    use zela_render::ZIndex;
    use zela_render::components::FillSolid;
    use zela_scene::scene::components::{
        Material3D, MeshGeometry, PrimitiveTopology, TextureHandle,
    };
    use ze_notion::Rect2D;
    use ze_object::{Camera2D, Camera3D, Projection};
    use ze_notion::Color32;

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

    /// 回读纹理像素（`width * 4` 按 256 对齐补行距）—— W1 起统一走
    /// [`RenderContext::read_texture`]（本模块的测试副本已消除）。
    fn read_rgba8(
        ctx: &RenderContext,
        texture: &wgpu::Texture,
        width: u32,
        height: u32,
    ) -> Vec<u8> {
        ctx.read_texture(texture, (width, height))
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
        let Some((_gpu, ctx)) = crate::gpu_test::require_gpu() else {
            return;
        };
        let device = ctx.device();
        let queue = ctx.queue();

        // W1：走**拆分后的两步装配**（`StandardBackends::new` + `install_into`）——
        // 与 `install_standard_backends(rs)` 同一份装配逻辑，只是换成 headless 上下文。
        // 这一步本身就验证了"装配不再依赖 egui 的 RenderState"。
        let mut resources = egui_wgpu::CallbackResources::new();
        StandardBackends::new(&ctx).install_into(&mut resources);

        // ── 内容层：整张红底 ───────────────────────────────────────────
        let mut content_queue = DrawQueue::new();
        content_queue.rect_fill(
            ZIndex::new(0),
            Rect2D::new(ze_notion::Vec2::ZERO, ze_notion::Vec2::splat(TEX as f32)),
            FillSolid::new(Color32::RED),
        );
        let content = vec![(CONTENT_LAYER, snapshot_queue(&content_queue))];

        // ── 3D 层：全屏四边形，材质引用内容纹理 ────────────────────────
        let quad = MeshGeometry::new(
            vec![
                ze_notion::Vec3::new(-1.0, -1.0, 0.0),
                ze_notion::Vec3::new(1.0, -1.0, 0.0),
                ze_notion::Vec3::new(1.0, 1.0, 0.0),
                ze_notion::Vec3::new(-1.0, 1.0, 0.0),
            ],
            Some(vec![0, 1, 2, 0, 2, 3]),
            PrimitiveTopology::Triangles,
        )
        .with_uvs(vec![
            ze_notion::Vec2::new(0.0, 1.0),
            ze_notion::Vec2::new(1.0, 1.0),
            ze_notion::Vec2::new(1.0, 0.0),
            ze_notion::Vec2::new(0.0, 0.0),
        ])
        .with_normals(vec![ze_notion::Vec3::Z; 4]);
        let mut material = Material3D::color(Color32::WHITE);
        material.texture = Some(TextureHandle::new(CONTENT_ID));
        let mut queue_3d = DrawQueue3D::new();
        queue_3d.draw_mesh(std::sync::Arc::new(quad), ze_notion::Mat4::IDENTITY, material);

        // 正交相机：垂直半高 1.0、aspect 1.0 ⇒ 覆盖世界 x/y ∈ [-1,1]，
        // 与四边形的世界范围完全对齐（视口 32×32，aspect = 1.0）。
        let camera = Camera3D::new(
            ze_notion::Vec3::new(0.0, 0.0, 5.0),
            ze_notion::Vec3::ZERO,
            ze_notion::Vec3::Y,
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
                        Rect2D::new(ze_notion::Vec2::ZERO, ze_notion::Vec2::splat(TEX as f32)),
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
            &LightQueue::new(),
            content,
            Vec::new(),
            Vec::new(),
            PostFilterChain::default(),
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
            .get::<crate::standard_backends::ContentBackend2D>()
            .map(|cb| &cb.0)
            .and_then(|b| b.offscreen_texture(CONTENT_LAYER))
            .expect("内容纹理应已创建");
        let content_pixels = read_rgba8(&ctx, tex_2d, TEX, TEX);
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
        let pixels = read_rgba8(&ctx, &tex_3d, TEX, TEX);
        assert_eq!(
            pixel_at(&pixels, TEX, TEX / 2, TEX / 2),
            RED_BGRA,
            "3D 曲面中心应采样到内容纹理（红）⇒ 内容层先于 3D 层注册"
        );
    }

    /// **W1 核心断言**：headless 入口与 egui 宿主路径**逐字节一致**。
    ///
    /// 一帧的输入刻意取"三类图层齐全"的形态：内容纹理层（会用到**惰性创建**的
    /// 第 4 个后端实例）+ 3D 视口（离屏 + 采样内容纹理）+ 2D 视口（两个图元）。
    /// 两条路径分别渲染：
    /// - **A（egui 路径）**：后端住 `CallbackResources`（TypeMap），走
    ///   `CallbackTrait::prepare`；
    /// - **B（headless 路径）**：[`render_frame_headless`]，后端住 [`StandardBackends`]。
    ///
    /// 断言两张目标纹理完全相同 —— 即"**装配方式与宿主差异不改变一个像素**"。
    /// 滤镜链跑两遍：空链（直通）与非空链（post 路径）。
    ///
    /// # 它到底锁住了什么（诚实边界）
    ///
    /// 本用例覆盖的是 headless 入口的**全部宿主侧动作**：自建目标纹理、清屏色、
    /// pass 的 viewport/scissor、`paint_frame` 分派、`submit`、回读 —— 以及
    /// [`BackendStore`] 两个实现（TypeMap 的路由/惰性插入 vs 自持有）的等价性。
    ///
    /// ⚠️ 它**不**覆盖"回调矩形是帧缓冲子矩形"的偏移语义：本用例的目标纹理尺寸
    /// 恰等于视口矩形、偏移为 0，故 headless 里显式设的 `set_viewport(0,0,w,h)`
    /// 与默认值等价。子矩形（含控制面板的实机布局）由 `wgpu_2d` 的
    /// `composite_lands_inside_scoped_viewport_on_real_device` 覆盖。
    ///
    /// **证伪**（任一改动都会让本用例 RED —— 注意只有**宿主侧**改动才可证伪；
    /// 改 `prepare_frame` 内部会同时影响两条路径，故本用例挡不住那种改动）：
    /// 1. 删掉 `render_frame_headless` 里的 `ctx.submit(encoder.finish())`
    ///    ⇒ B 只剩清屏色；
    /// 2. 把 B 的 `ScreenParams` 尺寸改成 `[TEX * 2, TEX * 2]` ⇒ 目标尺寸与视口
    ///    不再一致，`viewport_px` 算出的尺寸与 A 不同；
    /// 3. 把 B 传给 `ScreenParams` 的 `clear` 换成另一个颜色 ⇒ 背景不同；
    /// 4. 让 [`StandardBackends::content_backend_2d_mut`] 与视口层**共用同一个**
    ///    `WgpuBackend2D` ⇒ 同帧两次 `finish_frame` 互相覆盖 uniform arena，
    ///    内容纹理错乱（这正是当年分离实例要解决的问题）。
    #[test]
    fn headless_path_matches_egui_path_bit_for_bit() {
        use zela_render::{FilterSlot, StrokeSolid};
        use ze_object::Line;

        let Some((_gpu, ctx)) = crate::gpu_test::require_gpu() else {
            return;
        };
        let device = ctx.device();
        let queue = ctx.queue();

        // ── 内容层：整张红底（`composite = false`：只作纹理源）────────────────
        let mut content_queue = DrawQueue::new();
        content_queue.rect_fill(
            ZIndex::new(0),
            Rect2D::new(ze_notion::Vec2::ZERO, ze_notion::Vec2::splat(TEX as f32)),
            FillSolid::new(Color32::RED),
        );
        let content = vec![(CONTENT_LAYER, snapshot_queue(&content_queue))];

        // ── 2D 视口层：一个填充矩形 + 一条描边线段 ────────────────────────────
        let mut queue_2d = DrawQueue::new();
        queue_2d.rect_fill(
            ZIndex::new(0),
            Rect2D::new(
                ze_notion::Vec2::new(2.0, 2.0),
                ze_notion::Vec2::new(16.0, 10.0),
            ),
            FillSolid::new(Color32::GREEN),
        );
        queue_2d.line_stroke(
            ZIndex::new(1),
            Line::new((2.0, 18.0), (30.0, 30.0)),
            StrokeSolid::new(3.0, (0, 0, 255, 255)),
        );

        // ── 3D 视口层：全屏四边形，材质采样内容纹理 ───────────────────────────
        let quad = MeshGeometry::new(
            vec![
                ze_notion::Vec3::new(-1.0, -1.0, 0.0),
                ze_notion::Vec3::new(1.0, -1.0, 0.0),
                ze_notion::Vec3::new(1.0, 1.0, 0.0),
                ze_notion::Vec3::new(-1.0, 1.0, 0.0),
            ],
            Some(vec![0, 1, 2, 0, 2, 3]),
            PrimitiveTopology::Triangles,
        )
        .with_uvs(vec![
            ze_notion::Vec2::new(0.0, 1.0),
            ze_notion::Vec2::new(1.0, 1.0),
            ze_notion::Vec2::new(1.0, 0.0),
            ze_notion::Vec2::new(0.0, 0.0),
        ])
        .with_normals(vec![ze_notion::Vec3::Z; 4]);
        let mut material = Material3D::color(Color32::WHITE);
        material.texture = Some(TextureHandle::new(CONTENT_ID));
        let mut queue_3d = DrawQueue3D::new();
        queue_3d.draw_mesh(std::sync::Arc::new(quad), ze_notion::Mat4::IDENTITY, material);

        let camera = Camera3D::new(
            ze_notion::Vec3::new(0.0, 0.0, 5.0),
            ze_notion::Vec3::ZERO,
            ze_notion::Vec3::Y,
            Projection::orthographic(1.0, 1.0, 0.1, 100.0),
        );
        let viewport_points =
            Rect2D::from_min_size(GeoVec2::ZERO, GeoVec2::new(TEX as f32, TEX as f32));
        // 声明序刻意把**内容层放最后**：它必须被趟 1 提前渲染（结构保证，非声明序）。
        let layers = vec![
            SceneLayer {
                id: VIEW_3D_LAYER,
                kind: LayerKind::Viewport3D { camera },
                blend: LayerBlend::Alpha,
                opacity: 1.0,
            },
            SceneLayer {
                id: LayerId(3),
                // 视口 2D 的几何已是**屏幕空间**（`record_scene` 已烘进 `Shape2D`），
                // 帧体只用它的视口矩形，不读相机 ⇒ 这里的相机取值不影响结果。
                kind: LayerKind::Viewport2D {
                    camera: Camera2D::new(
                        (TEX as f32 / 2.0, TEX as f32 / 2.0).into(),
                        1.0,
                        Rect2D::new(ze_notion::Vec2::ZERO, ze_notion::Vec2::splat(TEX as f32)),
                    ),
                },
                blend: LayerBlend::Alpha,
                opacity: 1.0,
            },
            SceneLayer {
                id: CONTENT_LAYER,
                kind: LayerKind::ContentTexture {
                    camera: Camera2D::new(
                        (TEX as f32 / 2.0, TEX as f32 / 2.0).into(),
                        1.0,
                        Rect2D::new(ze_notion::Vec2::ZERO, ze_notion::Vec2::splat(TEX as f32)),
                    ),
                    size_px: [TEX, TEX],
                    texture: TextureHandle::new(CONTENT_ID),
                    composite: false,
                },
                blend: LayerBlend::Alpha,
                opacity: 1.0,
            },
        ];

        let mut non_empty_filters = PostFilterChain::new();
        non_empty_filters.push(FilterSlot::brightness(1.2));
        non_empty_filters.push(FilterSlot::blur(1.0));

        for (label, filters) in [
            ("空链（直通）", PostFilterChain::default()),
            ("非空链（post 路径）", non_empty_filters),
        ] {
            let callback = SceneCompositorCallback::new(
                viewport_points,
                layers.clone(),
                &queue_2d,
                &queue_3d,
                &LightQueue::new(),
                content.clone(),
                Vec::new(),
                Vec::new(),
                filters,
            );
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [TEX, TEX],
                pixels_per_point: 1.0,
            };
            let clear = wgpu::Color {
                r: 0.1,
                g: 0.2,
                b: 0.3,
                a: 1.0,
            };

            // ── A：egui 路径（后端住 TypeMap，走 `CallbackTrait::prepare`）──
            let mut resources = egui_wgpu::CallbackResources::new();
            StandardBackends::new(&ctx).install_into(&mut resources);
            let target_a = ctx.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("w1-egui-target"),
                size: wgpu::Extent3d {
                    width: TEX,
                    height: TEX,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: ctx.target_format(),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view_a = target_a.create_view(&wgpu::TextureViewDescriptor::default());
            let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("w1-egui-encoder"),
            });
            let _ = egui_wgpu::CallbackTrait::prepare(
                &callback,
                device,
                queue,
                &screen,
                &mut enc,
                &mut resources,
            );
            {
                // 复刻 egui-wgpu 的 "courtesy" 行为：viewport == scissor == 回调矩形。
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("w1-egui-main-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view_a,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_viewport(0.0, 0.0, TEX as f32, TEX as f32, 0.0, 1.0);
                pass.set_scissor_rect(0, 0, TEX, TEX);
                let compositor = resources.get::<Compositor>().expect("合成器在 TypeMap 里");
                let composed = resources
                    .get::<SceneCompositorFrame>()
                    .expect("prepare 已写入合成帧");
                callback.paint_frame(compositor, composed, &mut pass);
            }
            ctx.submit(enc.finish());
            let px_a = ctx.read_texture(&target_a, (TEX, TEX));

            // ── B：headless 路径（后端自持有，走 `render_frame_headless`）──
            let mut backends = StandardBackends::new(&ctx);
            let px_b = render_frame_headless(
                &ctx,
                &mut backends,
                &callback,
                ScreenParams::new([TEX, TEX], 1.0),
                clear,
            );

            assert_eq!(
                px_a.len(),
                px_b.len(),
                "{label}：两条路径的输出长度必须相同"
            );
            assert!(
                px_a == px_b,
                "{label}：headless 与 egui 路径必须**逐字节一致**（首个不同字节：{:?}）",
                px_a.iter()
                    .zip(&px_b)
                    .position(|(a, b)| a != b)
                    .map(|i| (i, px_a[i], px_b[i]))
            );
        }
    }
}
