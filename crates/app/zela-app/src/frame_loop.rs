//! 帧循环（B9 Step 3）：`zela-render` 与 `zela-verify` 共用的**唯一**渲染循环。
//!
//! # 为什么需要把它抽出来
//!
//! B9 要把 19 次进程调用压成 1 次（GPU 初始化 + 每个作品的运行时构建仍是必需的）。
//! 批量模式若另写一份循环，就会与单进程路径形成**第二条渲染路径** —— 那是双真源，
//! 也正是 B8 用 sink 抽象消灭的形态。⇒ 循环只此一份，两个二进制都调这里。
//!
//! # 谁负责什么
//!
//! - 本模块：逐帧推进、构图、渲染、回读、**逐帧分段计时**；
//! - 调用方：输出 / 校验（`FrameSink` 或 VM 闭包）、汇总与门禁。
//!
//! 调用方通过 [`FrameLoop::on_frame`] 拿到 [`FrameView`]：目标格式字节 +（按需的）RGBA8。
//! 回调返回**其中的哈希耗时**（不哈希就返回 `Duration::ZERO`），循环据此把
//! `t_hash` 从 `t_sink` 里拆出来 —— 单进程与批量两条路径因此产出同一套计时字段。

use std::time::{Duration, Instant};

use ze_notion::{Rect2D, Vec2};
use zela_playback::Runtime;
use zela_render::ViewRect;
use zela_render_backends::registry::{AdapterFacts, BackendKind};
use zela_render_backends::{
    HeadlessTarget, RenderContext, ScreenParams, StandardBackends,
    render_frame_headless_cached_timed,
};
use zela_scene_ecs::DriverCommand;
use zela_scene_ecs::contract::WorldView;

use zela_render_backends::routing as backend_router;
pub use zela_render_backends::routing::LoopBackend;
use zela_render_backends::RoutingDecision;

/// 一次渲染作业的画幅 / 帧率 / 后端。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameLoopOptions {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub backend: LoopBackend,
    /// **B10 Step 4**：矢量后端的着色栈覆盖。
    ///
    /// `None` ⇒ 按后端能力取默认（矢量 = `PostCaps::all()` ⇒ tone/bloom/ao 全开）；
    /// `Some(_)` ⇒ 用调用方给的值。存在的唯一理由是**让证据可测**：
    /// "AgX ≠ clip / bloom 外溢 / AO 压角"必须能在同一份产物上逐项开关，
    /// 而不是只靠"没报错"（计划 §6.2）。生产调用方（`zela-render` / `zela-verify`）恒传 `None`。
    pub vector_params: Option<zela_scene_bundle::RenderParams>,
}

/// 一帧交给调用方的像素视图。
pub struct FrameView<'a> {
    /// 目标纹理格式的字节（BGRA 等）。
    pub raw: &'a [u8],
    /// 规范化 RGBA8；调用方声明不需要时为空。
    pub rgba: &'a [u8],
    pub size: (u32, u32),
}

/// 一帧的分段耗时（B8 Step 1 的字段，B9 起由本模块产出）。
///
/// 段边界见 `zela-render` 的同名文档：`prepare` 含三角化与 `write_buffer`，
/// `read` 含 GPU 等待，`verify` 是 GPU 校验 pass 的占位（当前恒 0）。
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameTiming {
    pub advance: Duration,
    pub build: Duration,
    pub prepare: Duration,
    pub paint: Duration,
    pub submit: Duration,
    pub read: Duration,
    pub sink: Duration,
    pub hash: Duration,
    pub verify: Duration,
    pub end: Duration,
}

impl FrameTiming {
    pub fn total(&self) -> Duration {
        self.advance
            + self.build
            + self.prepare
            + self.paint
            + self.submit
            + self.read
            + self.sink
            + self.hash
            + self.verify
            + self.end
    }
}

/// 帧数 = `ceil(duration × fps)`，至少 1 帧（单进程与批量共用同一口径）。
pub fn frames_for(runtime: &Runtime, fps: f64) -> u32 {
    if runtime.duration > 0.0 {
        (runtime.duration * fps).ceil() as u32
    } else {
        1
    }
    .max(1)
}


/// 矢量后端自述的能力（B10 Step 4）：后处理四项全开，采样数取 4（与 2D 路径同口径）。
/// 只用于 [`zela_scene_bundle::RenderParams::default_for`]：**默认值绑后端能力**的机制
/// 需要一份 `Capabilities`，而矢量后端不走 `StandardBackends`（进程级开关）。
fn vector_capabilities() -> ze_render_ir::Capabilities {
    ze_render_ir::Capabilities {
        max_sample_count: 4,
        color_format: ze_render_ir::ColorFormat::Rgba8UnormSrgb,
        depth_format: ze_render_ir::DepthFormat::Depth32Float,
        post: zela_render_backends::BackendKind::Vector.post_caps(),
    }
}

/// 逐帧渲染（**唯一**实现）。
///
/// - 开始前写入 `ViewRect`（headless 没有 egui 布局：视口 = 整幅画面）；
/// - 按后端能力校验渲染参数（不支持 ⇒ 显式报错，不静默降级）；
/// - 每帧顺序：advance → 构图 → 渲染 + 回读 → `on_frame` → `end_frame`。
///
/// `on_frame` 收到 `(帧号, 像素视图)`，返回该帧**哈希耗时**（未哈希 ⇒ `ZERO`）。
/// 返回逐帧计时（调用方负责汇总与门禁）。
pub fn run_frames(
    ctx: &RenderContext,
    runtime: &mut Runtime,
    backends: &mut StandardBackends,
    target: &mut HeadlessTarget,
    options: FrameLoopOptions,
    want_rgba: bool,
    on_frame: &mut dyn FnMut(u32, FrameView<'_>) -> Result<Duration, String>,
) -> Result<Vec<FrameTiming>, String> {
    let (width, height) = (options.width, options.height);
    runtime.driver.with_scheduler_mut(|scheduler| {
        scheduler.resources_mut().insert(ViewRect {
            rect: Rect2D::new(Vec2::ZERO, Vec2::new(width as f32, height as f32)),
        });
    });

    // B4 Step 2 / B6 §0.3：默认值按后端能力取（wgpu `PostCaps::none()` ⇒ 与旧 `Default`
    // 逐位相同）；显式请求不支持项仍报错。
    let caps = match ctx.adapter() {
        Some(adapter) => Some(
            backends
                .backend_3d_mut()
                .capabilities(adapter)
                .map_err(|e| format!("查询后端能力失败：{e}"))?,
        ),
        None => None,
    };
    // B11 Step 4：作品级路由 —— 第一帧决定后固定（`AutoFrame` 每帧重算，诊断用）。
    let mut pinned: Option<RoutingDecision> = None;

    let screen = ScreenParams::new([width, height], 1.0);
    // 背景与 wgpu 分支**同源**：都用同一个 clear（不读 `SceneView::background`，
    // 否则同一个脚本换 `--backend` 就换了底色）。
    let clear = wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    let dt = 1.0 / options.fps as f32;
    let frames = frames_for(runtime, options.fps);
    let mut timings = Vec::with_capacity(frames as usize);

    for index in 0..frames {
        let mut timing = FrameTiming::default();

        // **与 GUI 同一条命令**：固定 dt（导出比实时墙钟更确定）。
        let mark = Instant::now();
        runtime
            .driver
            .submit(DriverCommand::Advance { dt })
            .wait();
        timing.advance = mark.elapsed();

        // ── B11 Step 4：先选后端（作品级：第一帧决定后固定） ──────────────────
        let build_mark = Instant::now();
        let mut snapshot: Option<zela_playback::vector_frame::VectorFrame> =
            if options.backend == LoopBackend::Wgpu {
                None
            } else {
                Some(zela_playback::vector_frame::VectorFrame::take(&runtime)?)
            };
        let snapshot_time = build_mark.elapsed();
        let wanted = if options.backend == LoopBackend::Wgpu {
            // 显式 wgpu：与改动前逐位相同，不付快照代价。
            BackendKind::Wgpu
        } else {
            let snap = snapshot.as_ref().expect("非 wgpu 路径已取快照");
            let decision = match (&pinned, options.backend) {
                (Some(pinned), LoopBackend::Auto) | (Some(pinned), LoopBackend::Vector) => {
                    pinned.clone()
                }
                _ => backend_router::route(
                    options.backend,
                    snap.content(),
                    ctx.adapter().map(AdapterFacts::of),
                )?,
            };
            if pinned.as_ref() != Some(&decision) {
                eprintln!("[backend] {}", decision.report());
            }
            if options.backend != LoopBackend::AutoFrame {
                pinned = Some(decision.clone());
            }
            decision.backend
        };

        // 渲染参数：默认值按**实际**后端能力取（wgpu 仍走 `default_for(caps)` ⇒ 逐位相同）。
        let mut params = match options.vector_params {
            Some(params) => params,
            None => match wanted {
                BackendKind::Wgpu => match &caps {
                    Some(caps) => zela_scene_bundle::RenderParams::default_for(caps),
                    None => zela_scene_bundle::RenderParams::default(),
                },
                // **B10 Step 4**：矢量后端自述 `PostCaps::all()` ⇒ `default_for` 打开
                // tone / bloom / ao。这不是静默降级：调用方**没有请求**，按该后端能力的默认值取。
                BackendKind::Vector => {
                    zela_scene_bundle::RenderParams::default_for(&vector_capabilities())
                }
            },
        };
        params.size = (width, height);
        params
            .validate()
            .map_err(|e| format!("渲染参数非法：{e}"))?;
        if wanted == BackendKind::Wgpu {
            if let Some(caps) = &caps {
                zela_playback::check_render_params_supported(&params, caps)
                    .map_err(|e| e.to_string())?;
            }
        }

        let convert: Duration;
        let (raw, rgba): (Vec<u8>, Vec<u8>) = if wanted == BackendKind::Vector {
            // 不支持的内容（纹理图层 / 混排 / 线框）已在路由层被拒 ⇒ 这里不再重复判断。
            let snap = snapshot.take().expect("矢量路径已有快照");
            timing.build = snapshot_time;
            let mark = Instant::now();
            let rgba = snap.render(
                &runtime,
                width,
                height,
                &params,
                [
                    clear.r as f32,
                    clear.g as f32,
                    clear.b as f32,
                    clear.a as f32,
                ],
            )?;
            timing.prepare = mark.elapsed();

            // 矢量帧已是 RGBA8；sink 的写入口径是目标格式（BGRA），这里做一次通道交换。
            let mark = Instant::now();
            let raw = super::export::rgba_to_target(ctx, &rgba);
            convert = mark.elapsed();
            (raw, rgba)
        } else {
            let mark = Instant::now();
            let callback = runtime
                .driver
                .with_scheduler(|scheduler| {
                    let view_options = zela_render::FrameOptions::default();
                    let view = zela_render::frame::build_scene_view(
                        &WorldView::new(scheduler.world()),
                        scheduler.resources(),
                        view_options,
                    );
                    zela_render::frame::build_frame_inputs(
                        &view,
                        scheduler.resources(),
                        view_options,
                    )
                    .and_then(zela_render_backends::frame::build_scene_callback)
                })
                .ok_or_else(|| "本帧没有可渲染图层（场景为空）".to_string())?;
            timing.build = mark.elapsed();

            let (pixels, headless) = render_frame_headless_cached_timed(
                ctx, backends, &callback, screen, clear, target,
            );
            timing.prepare = headless.render.prepare;
            timing.paint = headless.render.paint;
            timing.submit = headless.render.submit;
            timing.read = headless.read;

            let mark = Instant::now();
            let rgba = if want_rgba {
                super::export::to_rgba(ctx, &pixels)
            } else {
                Vec::new()
            };
            convert = mark.elapsed();
            (pixels, rgba)
        };

        // 输出 / 校验：渲染核心不感知它们是什么。
        let mark = Instant::now();
        let hash_time = {
            let frame = FrameView {
                raw: &raw,
                rgba: &rgba,
                size: (width, height),
            };
            on_frame(index, frame)?
        };
        let sink_all = mark.elapsed() + convert;
        timing.hash = hash_time;
        timing.sink = sink_all.saturating_sub(hash_time);

        let mark = Instant::now();
        runtime.driver.with_scheduler_mut(|scheduler| {
            zela_render::frame::end_frame(scheduler.resources_mut());
        });
        timing.end = mark.elapsed();

        timings.push(timing);
    }

    Ok(timings)
}
