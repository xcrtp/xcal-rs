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
/// `read` 含 GPU 等待，`verify` 是 GPU 校验 pass 的占位。
///
/// ⚠️ **`verify` 未启用**（B12 Step 1.3）：该 pass 尚未实现 ⇒ 恒 0，
/// **不代表"校验耗时 0"**，而是"没有校验"。`FrameStats` 的汇总里会单独标一行。
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

/// 性能**量级门禁**阈值（B8 §3.3 / B12 Step 6）：只拦灾难性退化，不卡正常波动。
pub const SLOW_FRAME_MS: f64 = 500.0;
/// 汇总窗口：实测数据按"前 30 帧"回报（B8 §3.4），另给全程汇总。
pub const TIMING_WINDOW: usize = 30;

/// 逐帧计时累加器（**唯一实现**：`zela-render` 与 `zela-verify` 共用，B12 Step 1.2）。
///
/// 字段名必须与 `[timing]` / `[timing-summary]` 的 JSON 一致，否则前后无法对比。
pub struct FrameStats {
    totals: FrameTiming,
    seen: usize,
    window: FrameTiming,
    window_frames: usize,
    slow: Vec<(u32, f64)>,
}

impl FrameStats {
    pub fn new() -> Self {
        Self {
            totals: FrameTiming::default(),
            seen: 0,
            window: FrameTiming::default(),
            window_frames: 0,
            slow: Vec::new(),
        }
    }

    /// `index` = 帧号；首帧（0）不计入窗口与门禁（含管线创建）。
    pub fn record(&mut self, index: u32, timing: &FrameTiming) {
        add_timing(&mut self.totals, timing);
        self.seen += 1;
        if index > 0 {
            if self.window_frames < TIMING_WINDOW {
                add_timing(&mut self.window, timing);
                self.window_frames += 1;
            }
            let total_ms = timing.total().as_secs_f64() * 1000.0;
            if total_ms > SLOW_FRAME_MS {
                self.slow.push((index, total_ms));
            }
        }
    }

    /// 打印汇总（与 `zela-render` 同一份 JSON）+ 量级门禁。
    ///
    /// 单帧作品没有"1..30"窗口，跳过该行（否则打印一列 0，易误读）。
    pub fn report(&self) -> Result<(), String> {
        if self.window_frames > 0 {
            eprintln!(
                "{}",
                summary_json(
                    &self.window,
                    &format!("1..{}", self.window_frames),
                    self.window_frames
                )
            );
        }
        eprintln!("{}", summary_json(&self.totals, "all", self.seen));
        eprintln!(
            "[timing-note] verify_ms 恒 0 是因为 GPU 校验 pass **未启用**（B12 Step 1.3），不代表校验耗时 0"
        );
        if let Some((frame, ms)) = self.slow.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
            return Err(format!(
                "性能量级门禁失败：第 {frame} 帧耗时 {ms:.1}ms > {SLOW_FRAME_MS}ms（B8 §3.3；\
                 共 {} 帧超限）",
                self.slow.len()
            ));
        }
        Ok(())
    }
}

/// 一帧的 JSON 行（B8 Step 1；字段定义在 [`FrameTiming`]）。
pub fn timing_json(t: &FrameTiming, frame: u32) -> String {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    format!(
        "[timing] {{\"frame\":{frame},\"advance_ms\":{:.3},\"build_ms\":{:.3},\
         \"prepare_ms\":{:.3},\"paint_ms\":{:.3},\"submit_ms\":{:.3},\
         \"read_ms\":{:.3},\"sink_ms\":{:.3},\"hash_ms\":{:.3},\"verify_ms\":{:.3},\
         \"end_ms\":{:.3},\"total_ms\":{:.3}}}",
        ms(t.advance),
        ms(t.build),
        ms(t.prepare),
        ms(t.paint),
        ms(t.submit),
        ms(t.read),
        ms(t.sink),
        ms(t.hash),
        ms(t.verify),
        ms(t.end),
        ms(t.total()),
    )
}

/// 窗口汇总（JSON）：各段平均值与占比（占比按 `total` 算）。
pub fn summary_json(t: &FrameTiming, window: &str, frames: usize) -> String {
    let n = frames.max(1) as f64;
    let avg_ms = |d: Duration| d.as_secs_f64() * 1000.0 / n;
    let total = avg_ms(t.total()).max(f64::MIN_POSITIVE);
    let share = |d: Duration| avg_ms(d) / total * 100.0;
    format!(
        "[timing-summary] {{\"window\":\"{window}\",\"frames\":{frames},\
         \"avg_total_ms\":{:.3},\
         \"avg\":{{\"advance\":{:.3},\"build\":{:.3},\"prepare\":{:.3},\"paint\":{:.3},\
         \"submit\":{:.3},\"read\":{:.3},\"sink\":{:.3},\"hash\":{:.3},\"verify\":{:.3},\
         \"end\":{:.3}}},\
         \"share_pct\":{{\"advance\":{:.1},\"build\":{:.1},\"prepare\":{:.1},\"paint\":{:.1},\
         \"submit\":{:.1},\"read\":{:.1},\"sink\":{:.1},\"hash\":{:.1},\"verify\":{:.1},\
         \"end\":{:.1}}}}}",
        avg_ms(t.total()),
        avg_ms(t.advance),
        avg_ms(t.build),
        avg_ms(t.prepare),
        avg_ms(t.paint),
        avg_ms(t.submit),
        avg_ms(t.read),
        avg_ms(t.sink),
        avg_ms(t.hash),
        avg_ms(t.verify),
        avg_ms(t.end),
        share(t.advance),
        share(t.build),
        share(t.prepare),
        share(t.paint),
        share(t.submit),
        share(t.read),
        share(t.sink),
        share(t.hash),
        share(t.verify),
        share(t.end),
    )
}

fn add_timing(acc: &mut FrameTiming, other: &FrameTiming) {
    acc.advance += other.advance;
    acc.build += other.build;
    acc.prepare += other.prepare;
    acc.paint += other.paint;
    acc.submit += other.submit;
    acc.read += other.read;
    acc.sink += other.sink;
    acc.hash += other.hash;
    acc.verify += other.verify;
    acc.end += other.end;
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
///
/// 返回 `(逐帧计时, 去重后的路由决策)`：计时交给调用方汇总与门禁；决策交给调用方落盘
/// （B12 Step 2 —— B11 只打印，没沉淀）。
pub fn run_frames(
    ctx: &RenderContext,
    runtime: &mut Runtime,
    backends: &mut StandardBackends,
    target: &mut HeadlessTarget,
    options: FrameLoopOptions,
    want_rgba: bool,
    on_frame: &mut dyn FnMut(u32, FrameView<'_>) -> Result<Duration, String>,
) -> Result<(Vec<FrameTiming>, Vec<RoutingDecision>), String> {
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
    // B12 Step 2：决策日志（相邻重复只留一条；`AutoFrame` 可能真的换后端）。
    let mut decisions: Vec<RoutingDecision> = Vec::new();

    for index in 0..frames {
        let mut timing = FrameTiming::default();

        // **与 GUI 同一条命令**：固定 dt（导出比实时墙钟更确定）。
        let mark = Instant::now();
        runtime
            .driver
            .submit(DriverCommand::Advance { dt })
            .wait();
        timing.advance = mark.elapsed();

        // ── B11 Step 4 + B12 Step 2：先选后端（作品级：第一帧决定后固定） ──────
        // 决策本身是可观测对象：逐条校验（拒绝理由不得为空），并收集起来交给调用方落盘。
        let build_mark = Instant::now();
        let mut snapshot: Option<zela_playback::vector_frame::VectorFrame> =
            if options.backend == LoopBackend::Wgpu {
                None
            } else {
                Some(zela_playback::vector_frame::VectorFrame::take(&runtime)?)
            };
        let snapshot_time = build_mark.elapsed();
        let decision = if options.backend == LoopBackend::Wgpu {
            // 显式 wgpu：与改动前逐位相同，不付快照代价。
            RoutingDecision::explicit_wgpu()
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
            decision
        };
        // C4：空拒绝理由 = 静默决策 ⇒ 在这里就失败，不让它流进产物。
        decision.validate()?;
        if decisions.last() != Some(&decision) {
            decisions.push(decision.clone());
        }
        let wanted = decision.backend;

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

    Ok((timings, decisions))
}
