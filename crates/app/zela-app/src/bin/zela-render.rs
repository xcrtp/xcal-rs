//! `zela-render`：headless 薄壳 —— 脚本直接渲到视频。
//!
//! ```text
//! zela-render <脚本路径> [--frames DIR] [--fps N] [--out FILE] [--size WxH]
//! zela-render --bundle <file.zsb> [--frames DIR] [--fps N] [--out FILE] [--size WxH]
//! ```
//!
//! - 默认 **ffmpeg 管道**（rawvideo → libx264）；ffmpeg 不在 PATH ⇒ **明确报错**并提示 `--frames`；
//! - `--frames DIR` 走序列帧（PNG，交给外部工具合成）；
//! - `--hash-out FILE` 额外写出**帧像素序列哈希**（只覆盖画面像素，不含编码器元数据；
//!   两种 sink 都会计算并打印 `[frame-hash] sequence = …`）；
//! - 帧数 = `ceil(duration × fps)`，时长由脚本 / 产物决定；
//! - 脚本语言按扩展名推断（`.lua` / `.py`）；`.py` 但未开 `python` feature ⇒ 明确报错；
//! - `--bundle` 读 ZSB 产物：先做可还原性检查，再走**与脚本完全相同**的 Runtime 路径。
//!
//! # 与 GUI 的一致性
//!
//! 场景推进只经 **Driver pod** 的 `Advance(dt)` 命令（固定 `dt = 1/fps`），
//! 渲染帧体来自 `zela_render::frame` + `zela_render_backends::frame` ——
//! 与 GUI 是同一份构造。因此导出路径不存在"另写一套简化帧循环"的分叉。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ze_notion::{Rect2D, Vec2};
use zela_app::export::{ExportError, target_pix_fmt, to_rgba, write_png};
use zela_app::{
    ScriptLanguage, build_bundle_runtime, build_script_runtime, language_from_path,
};
use zela_render::ViewRect;
use zela_render_backends::{
    RenderContext, ScreenParams, StandardBackends, render_frame_headless_cached_timed,
};
use zela_scene_ecs::DriverCommand;
use zela_scene_ecs::contract::WorldView;
use zela_scene_bundle::Bundle;
use zela_scene_bundle::frame_hash::FrameSequenceHash;

struct Args {
    script: Option<PathBuf>,
    bundle: Option<PathBuf>,
    frames_dir: Option<PathBuf>,
    hash_out: Option<PathBuf>,
    fps: f64,
    out: PathBuf,
    size: (u32, u32),
    /// B6 Step 2.3：`--backend wgpu|vector`（默认 wgpu ⇒ 既有基线与行为不变）。
    backend: Backend,
}

/// 渲染后端选择（B6 Step 2.3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Backend {
    Wgpu,
    Vector,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let raw: Vec<String> = std::env::args().skip(1).collect();
        if raw.is_empty() || raw.iter().any(|a| a == "--help" || a == "-h") {
            return Err(
                "用法：zela-render <脚本路径> [--frames DIR] [--fps N] [--out FILE] [--size WxH]"
                    .to_string(),
            );
        }

        let mut script: Option<PathBuf> = None;
        let mut bundle: Option<PathBuf> = None;
        let mut frames_dir = None;
        let mut hash_out = None;
        let mut fps = 30.0f64;
        let mut out = PathBuf::from("out.mp4");
        let mut size = (640u32, 480u32);
        let mut backend = Backend::Wgpu;

        let mut i = 0;
        while i < raw.len() {
            match raw[i].as_str() {
                "--frames" => {
                    i += 1;
                    frames_dir = Some(PathBuf::from(
                        raw.get(i).ok_or("--frames 需要一个目录参数")?,
                    ));
                }
                // 帧像素序列哈希（跨运行时对拍口径；与 sink 无关）。
                "--hash-out" => {
                    i += 1;
                    hash_out = Some(PathBuf::from(
                        raw.get(i).ok_or("--hash-out 需要一个文件参数")?,
                    ));
                }
                // 与位置参数等价（计划 A0 的验收命令使用 `--script PATH`）。
                "--script" => {
                    i += 1;
                    if script.is_some() {
                        return Err("脚本路径重复（--script 与位置参数只能给一个）".to_string());
                    }
                    script = Some(PathBuf::from(
                        raw.get(i).ok_or("--script 需要一个脚本路径")?,
                    ));
                }
                "--bundle" => {
                    i += 1;
                    if bundle.is_some() {
                        return Err("--bundle 重复".to_string());
                    }
                    bundle = Some(PathBuf::from(
                        raw.get(i).ok_or("--bundle 需要一个 .zsb 路径")?,
                    ));
                }
                "--fps" => {
                    i += 1;
                    fps = raw
                        .get(i)
                        .ok_or("--fps 需要一个数字参数")?
                        .parse()
                        .map_err(|_| "--fps 必须是数字".to_string())?;
                }
                "--out" => {
                    i += 1;
                    out = PathBuf::from(raw.get(i).ok_or("--out 需要一个文件参数")?);
                }
                "--size" => {
                    i += 1;
                    let value = raw.get(i).ok_or("--size 需要 WxH 参数")?;
                    let (w, h) = value
                        .split_once('x')
                        .ok_or_else(|| "--size 必须是 WxH（如 640x480）".to_string())?;
                    size = (
                        w.parse().map_err(|_| "--size 宽度不是数字".to_string())?,
                        h.parse().map_err(|_| "--size 高度不是数字".to_string())?,
                    );
                }
                // B6 Step 2.3：后端选择（默认 wgpu）。
                "--backend" => {
                    i += 1;
                    match raw.get(i).map(String::as_str) {
                        Some("wgpu") => backend = Backend::Wgpu,
                        Some("vector") => backend = Backend::Vector,
                        other => {
                            return Err(format!(
                                "--backend 只接受 wgpu | vector（收到 {other:?}）"
                            ));
                        }
                    }
                }
                other if other.starts_with("--") => {
                    return Err(format!("未知参数：{other}"));
                }
                other => {
                    if script.is_some() {
                        return Err(format!("多余的参数：{other}"));
                    }
                    script = Some(PathBuf::from(other));
                }
            }
            i += 1;
        }

        if script.is_some() && bundle.is_some() {
            return Err("脚本路径与 --bundle 只能给一个".to_string());
        }
        if script.is_none() && bundle.is_none() {
            return Err("缺少脚本路径（或 --bundle <file.zsb>）".to_string());
        }

        Ok(Self {
            script,
            bundle,
            frames_dir,
            hash_out,
            fps,
            out,
            size,
            backend,
        })
    }
}

fn main() {
    if let Err(message) = run() {
        eprintln!("zela-render: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;

    let ctx =
        RenderContext::headless().map_err(|e| format!("无法创建 headless GPU 上下文：{e}"))?;

    let runtime = if let Some(script_path) = &args.script {
        let source = std::fs::read_to_string(script_path)
            .map_err(|e| format!("读取脚本 {} 失败：{e}", script_path.display()))?;
        let language = language_from_path(script_path).map_err(|e| e.to_string())?;
        if language == ScriptLanguage::Python && !cfg!(feature = "python") {
            return Err(
                "脚本是 .py，但本二进制编译时未启用 `python` feature（请重新构建或改用 .lua）"
                    .to_string(),
            );
        }
        build_script_runtime(ctx.clone(), source, language).map_err(|e| e.to_string())?
    } else if let Some(bundle_path) = &args.bundle {
        let bytes = std::fs::read(bundle_path)
            .map_err(|e| format!("读取 ZSB {} 失败：{e}", bundle_path.display()))?;
        let bundle = Bundle::from_bytes(&bytes)
            .map_err(|e| format!("ZSB 解析失败（{}）：{e}", bundle_path.display()))?;
        eprintln!(
            "[zsb] payload hash = {}（{} 个对象，{}s）",
            bundle.payload_hash_hex(),
            bundle.meta().object_count,
            bundle.meta().duration
        );
        build_bundle_runtime(ctx.clone(), &bundle).map_err(|e| e.to_string())?
    } else {
        unreachable!("Args::parse 已保证二者恰有一个")
    };

    let (width, height) = args.size;
    // headless 没有 egui 布局：视口 = 整幅画面（GUI 里由 `update_view_rect` 写入）。
    runtime.driver.with_scheduler_mut(|scheduler| {
        scheduler.resources_mut().insert(ViewRect {
            rect: Rect2D::new(Vec2::ZERO, Vec2::new(width as f32, height as f32)),
        });
    });

    let dt = 1.0 / args.fps as f32;
    let frames = if runtime.duration > 0.0 {
        (runtime.duration * args.fps).ceil() as u32
    } else {
        1
    }
    .max(1);
    eprintln!(
        "渲染 {frames} 帧（{}×{} @ {} fps，时长 {:.3}s）…",
        width, height, args.fps, runtime.duration
    );

    let screen = ScreenParams::new([width, height], 1.0);
    let clear = wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    let mut backends = StandardBackends::new(&ctx);
    // B8 Step 0：目标纹理跨帧复用（原先每帧 create_texture）。
    let mut target = zela_render_backends::HeadlessTarget::new();

    // B4 Step 2：渲染参数（档 1 默认值 = 当前行为；档 2 命令行覆盖 `--size`）。
    // B6 §0.3：默认值**按后端能力**取（`default_for`）——wgpu 的 `PostCaps::none()` ⇒
    // 与旧 `Default` 逐位相同（6 条基线不变）；显式请求不支持项仍由下面的一致性检查报错。
    let caps = match ctx.adapter() {
        Some(adapter) => Some(
            backends
                .backend_3d_mut()
                .capabilities(adapter)
                .map_err(|e| format!("查询后端能力失败：{e}"))?,
        ),
        None => None,
    };
    let mut params = match &caps {
        Some(caps) => zela_scene_bundle::RenderParams::default_for(caps),
        None => zela_scene_bundle::RenderParams::default(),
    };
    params.size = args.size;
    // 有效性 + 一致性都在开渲前拦住（§0.3：不静默降级）。
    params
        .validate()
        .map_err(|e| format!("渲染参数非法：{e}"))?;
    if let Some(caps) = &caps {
        zela_playback::check_render_params_supported(&params, caps).map_err(|e| e.to_string())?;
    }
    // 跨运行时对拍口径：逐帧累积**规范化 RGBA8 像素**的序列哈希。
    // 与 sink 无关（PNG / ffmpeg 都算同一个值），也不含任何编码器元数据。
    let mut frame_hash = FrameSequenceHash::new(width, height);
    // B2-6（D7）：**分层标签旁路** —— `ZELA_FRAME_DIGEST_SAMPLE=N` 时每 N 帧打印
    // 该帧的独立 digest。旁路不进入 `frame_hash`（判定与诊断分离）。
    let digest_sample: u32 = std::env::var("ZELA_FRAME_DIGEST_SAMPLE")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(0);

    let mut sink = match &args.frames_dir {
        Some(dir) => Sink::Png {
            dir: dir.clone(),
            width,
            height,
        },
        None => Sink::Ffmpeg(Ffmpeg::spawn(&ctx, &args.out, args.fps, (width, height))?),
    };

    // B8 Step 1：逐帧分段计时（默认只在窗口汇总 / 门禁上用；逐帧 JSON 由环境变量开启）。
    let timing_lines = std::env::var("ZELA_FRAME_TIMING").is_ok_and(|value| value != "0");
    let mut stats = FrameStats::new();

    for index in 0..frames {
        let mut timing = FrameTiming::default();

        // **与 GUI 同一条命令**：固定 dt（导出比实时墙钟更确定）。
        let mark = Instant::now();
        runtime.driver.submit(DriverCommand::Advance { dt }).wait();
        timing.advance = mark.elapsed();

        // B6 Step 2.3：矢量后端走"同一个场景 → 快照 → 记录"的路径；只支持纯 2D 帧
        // （3D / 内容纹理图层没有对应的矢量记录，显式报错，不静默跳过）。
        let rgba = if args.backend == Backend::Vector {
            let mark = Instant::now();
            let (layers, draws) = runtime.driver.with_scheduler(|scheduler| {
                let options = zela_render::FrameOptions::default();
                let view = zela_render::frame::build_scene_view(
                    &WorldView::new(scheduler.world()),
                    scheduler.resources(),
                    options,
                );
                let layers = view.layers.clone();
                let draws = scheduler
                    .resources()
                    .get::<zela_render::draw::DrawQueue>()
                    .map(zela_render_backends::snapshot_queue)
                    .unwrap_or_default();
                (layers, draws)
            });
            if let Some(layer) = layers
                .iter()
                .find(|l| !matches!(l.kind, ze_scene_core::LayerKind::Viewport2D { .. }))
            {
                return Err(format!(
                    "--backend vector 目前只支持纯 2D 帧；本帧含图层 {:?}（见 GAPS V2-12/V2-13）",
                    layer.kind
                ));
            }
            let items: Vec<zela_render_vector::SceneItem<'_>> = draws
                .iter()
                .map(|recorded| zela_render_vector::SceneItem {
                    shape: &recorded.shape,
                    transform: recorded.transform,
                    style: recorded.style,
                })
                .collect();
            // 背景与 wgpu 分支**同源**：都用本次 CLI 的 `clear`，不读 `SceneView::background`
            // —— 否则同一个脚本换 `--backend` 就换了底色（两套 headless 语义，难解释）。
            let background = [
                clear.r as f32,
                clear.g as f32,
                clear.b as f32,
                clear.a as f32,
            ];
            timing.build = mark.elapsed();

            let mark = Instant::now();
            let rgba = zela_render_vector::render_scene_2d(width, height, 4, background, &items)?;
            timing.prepare = mark.elapsed();

            // 矢量帧已是 RGBA8；sink 的写入口径是目标格式（BGRA），这里做一次通道交换。
            let mark = Instant::now();
            sink.write(&ctx, index, &rgba_to_target(&ctx, &rgba))?;
            timing.sink = mark.elapsed();
            rgba
        } else {
            let mark = Instant::now();
            let callback = runtime
                .driver
                .with_scheduler(|scheduler| {
                    let options = zela_render::FrameOptions::default();
                    let view = zela_render::frame::build_scene_view(
                        &WorldView::new(scheduler.world()),
                        scheduler.resources(),
                        options,
                    );
                    zela_render::frame::build_frame_inputs(
                        &view,
                        scheduler.resources(),
                        options,
                    )
                    .and_then(zela_render_backends::frame::build_scene_callback)
                })
                .ok_or_else(|| "本帧没有可渲染图层（场景为空）".to_string())?;
            timing.build = mark.elapsed();

            let (pixels, headless) = render_frame_headless_cached_timed(
                &ctx,
                &mut backends,
                &callback,
                screen,
                clear,
                &mut target,
            );
            timing.prepare = headless.render.prepare;
            timing.paint = headless.render.paint;
            timing.submit = headless.render.submit;
            timing.read = headless.read;

            let mark = Instant::now();
            // 先规范化成 RGBA8（与 PNG 写盘同一份字节），再进序列哈希。
            let rgba = to_rgba(&ctx, &pixels);
            sink.write(&ctx, index, &pixels)?;
            timing.sink = mark.elapsed();
            rgba
        };
        // B2-6：旁路采样（只打印，不改序列哈希）。
        if digest_sample > 0 && index % digest_sample == 0 {
            let digest =
                zela_scene_bundle::sha256::hex(&zela_scene_bundle::sha256::sha256(&rgba));
            eprintln!("[frame-digest] frame = {index} digest = {digest}");
        }

        let mark = Instant::now();
        frame_hash.push_frame(&rgba);
        timing.hash = mark.elapsed();

        let mark = Instant::now();
        runtime.driver.with_scheduler_mut(|scheduler| {
            zela_render::frame::end_frame(scheduler.resources_mut());
        });
        timing.end = mark.elapsed();

        if timing_lines {
            eprintln!("{}", timing.json(index));
        }
        stats.record(index, &timing);
    }

    sink.finish()?;

    let sequence = frame_hash.hex();
    eprintln!(
        "[frame-hash] sequence = {sequence}（{} 帧，{}×{}，RGBA8）",
        frame_hash.frames(),
        width,
        height
    );
    if let Some(path) = &args.hash_out {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录 {} 失败：{e}", parent.display()))?;
        }
        std::fs::write(path, format!("{sequence}\n"))
            .map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
    }

    // B8 Step 1：分段耗时汇总（JSON 行）+ 量级门禁（> 500ms 才失败）。
    stats.report()?;

    // 软失败汇总：三角化失败只跳过该绘制，不中断导出、不改退出码。
    let skipped = zela_render_backends::diagnostics::total_skipped();
    if skipped > 0 {
        eprintln!(
            "[zela-render] 三角化失败累计跳过 {skipped} 个绘制（软失败：输出仍已产出；原因见上方命名日志）"
        );
    }
    Ok(())
}

/// 一帧的分段耗时（B8 Step 1）。
///
/// 段的边界严格对应代码里能独立计时的位置：
/// - `advance`：`DriverCommand::Advance`（含场景系统推进）；
/// - `build`：`build_scene_view` + `build_frame_inputs` + `build_scene_callback`
///   （矢量后端：view 构造 + 快照，不含绘制）；
/// - `prepare`：`prepare_frame` —— **含三角化与 `write_buffer`**（B8 计划里的 `t_tess`，
///   在本代码结构里与"构帧体"不可分：都在 `WgpuBackend2D::finish_frame` 内）；
///   矢量后端这一格是整帧 `render_scene_2d` 的 CPU 侧耗时；
/// - `paint`：主 pass 录制；
/// - `submit`：`queue.submit`（**不含** GPU 执行）；
/// - `read`：回读，**包含 GPU 等待**（`poll(wait)`）⇒ 这是唯一能看到 GPU 时间的段；
/// - `sink`：PNG / mp4 编码（含目标格式 ↔ RGBA 的通道转换）；
/// - `hash`：`FrameSequenceHash::push_frame`；
/// - `verify`：**GPU 校验 pass**（B8 补充更正 §二.2）—— 当前恒为 0，Step 3′ 后填真实值；
/// - `end`：`end_frame`（帧末清理）。
#[derive(Debug, Clone, Copy, Default)]
struct FrameTiming {
    advance: Duration,
    build: Duration,
    prepare: Duration,
    paint: Duration,
    submit: Duration,
    read: Duration,
    sink: Duration,
    hash: Duration,
    /// GPU 校验 pass（占位：字段先定死，Step 3′ 前后是同一套字段）。
    verify: Duration,
    end: Duration,
}

impl FrameTiming {
    fn total(&self) -> Duration {
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

    fn add(&mut self, other: &Self) {
        self.advance += other.advance;
        self.build += other.build;
        self.prepare += other.prepare;
        self.paint += other.paint;
        self.submit += other.submit;
        self.read += other.read;
        self.sink += other.sink;
        self.hash += other.hash;
        self.verify += other.verify;
        self.end += other.end;
    }

    fn json(&self, frame: u32) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        format!(
            "[timing] {{\"frame\":{frame},\"advance_ms\":{:.3},\"build_ms\":{:.3},\
             \"prepare_ms\":{:.3},\"paint_ms\":{:.3},\"submit_ms\":{:.3},\
             \"read_ms\":{:.3},\"sink_ms\":{:.3},\"hash_ms\":{:.3},\"verify_ms\":{:.3},\
             \"end_ms\":{:.3},\"total_ms\":{:.3}}}",
            ms(self.advance),
            ms(self.build),
            ms(self.prepare),
            ms(self.paint),
            ms(self.submit),
            ms(self.read),
            ms(self.sink),
            ms(self.hash),
            ms(self.verify),
            ms(self.end),
            ms(self.total()),
        )
    }

    /// 窗口汇总（JSON）：各段平均值与占比（占比按 `total` 算）。
    fn summary_json(&self, window: &str, frames: usize) -> String {
        let n = frames.max(1) as f64;
        let avg_ms = |d: Duration| d.as_secs_f64() * 1000.0 / n;
        let total = avg_ms(self.total()).max(f64::MIN_POSITIVE);
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
            avg_ms(self.total()),
            avg_ms(self.advance),
            avg_ms(self.build),
            avg_ms(self.prepare),
            avg_ms(self.paint),
            avg_ms(self.submit),
            avg_ms(self.read),
            avg_ms(self.sink),
            avg_ms(self.hash),
            avg_ms(self.verify),
            avg_ms(self.end),
            share(self.advance),
            share(self.build),
            share(self.prepare),
            share(self.paint),
            share(self.submit),
            share(self.read),
            share(self.sink),
            share(self.hash),
            share(self.verify),
            share(self.end),
        )
    }
}

/// 性能**量级门禁**（B8 §3.3）：只拦灾难性退化，不卡正常波动。
///
/// 首帧包含管线 / 着色器创建，不计入；阈值取实测均值的约 4 倍。
const SLOW_FRAME_MS: f64 = 500.0;
/// 汇总窗口：实测数据按"前 30 帧"回报（B8 §3.4），另给全程汇总。
const TIMING_WINDOW: usize = 30;

/// 逐帧计时累加器。
struct FrameStats {
    totals: FrameTiming,
    seen: usize,
    window: FrameTiming,
    window_frames: usize,
    slow: Vec<(u32, f64)>,
}

impl FrameStats {
    fn new() -> Self {
        Self {
            totals: FrameTiming::default(),
            seen: 0,
            window: FrameTiming::default(),
            window_frames: 0,
            slow: Vec::new(),
        }
    }

    /// `index` = 帧号；首帧（0）不计入窗口与门禁（含管线创建）。
    fn record(&mut self, index: u32, timing: &FrameTiming) {
        self.totals.add(timing);
        self.seen += 1;
        if index > 0 {
            if self.window_frames < TIMING_WINDOW {
                self.window.add(timing);
                self.window_frames += 1;
            }
            let total_ms = timing.total().as_secs_f64() * 1000.0;
            if total_ms > SLOW_FRAME_MS {
                self.slow.push((index, total_ms));
            }
        }
    }

    fn report(&self) -> Result<(), String> {
        eprintln!(
            "{}",
            self.window
                .summary_json(&format!("1..{}", self.window_frames), self.window_frames)
        );
        eprintln!("{}", self.totals.summary_json("all", self.seen));
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

/// RGBA8 → 目标纹理格式字节序（默认 BGRA）；与 `to_rgba` 互逆。
fn rgba_to_target(ctx: &RenderContext, rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    let format = ctx.target_format();
    if matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    ) {
        for px in out.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
    }
    out
}

enum Sink {
    Png {
        dir: PathBuf,
        width: u32,
        height: u32,
    },
    Ffmpeg(Ffmpeg),
}

impl Sink {
    fn write(&mut self, ctx: &RenderContext, index: u32, pixels: &[u8]) -> Result<(), String> {
        match self {
            Self::Png { dir, width, height } => {
                std::fs::create_dir_all(&dir)
                    .map_err(|e| format!("创建目录 {} 失败：{e}", dir.display()))?;
                let path = dir.join(format!("frame_{index:05}.png"));
                let rgba = to_rgba(ctx, pixels);
                write_png(&path, *width, *height, &rgba).map_err(|e| e.to_string())
            }
            Self::Ffmpeg(encoder) => encoder.write(pixels),
        }
    }

    fn finish(self) -> Result<(), String> {
        match self {
            Self::Png { dir, .. } => {
                eprintln!("完成：序列帧在 {}", dir.display());
                Ok(())
            }
            Self::Ffmpeg(encoder) => encoder.finish(),
        }
    }
}

struct Ffmpeg {
    child: Child,
    out: PathBuf,
}

impl Ffmpeg {
    fn spawn(
        ctx: &RenderContext,
        out: &Path,
        fps: f64,
        (width, height): (u32, u32),
    ) -> Result<Self, String> {
        // ffmpeg 不在 PATH ⇒ **明确报错**（不静默降级到序列帧）。
        if Command::new("ffmpeg")
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_err()
        {
            return Err(
                "未找到 ffmpeg（不在 PATH）。请安装 ffmpeg，或改用 `--frames DIR` 导出序列帧。"
                    .to_string(),
            );
        }

        // **查询**目标格式决定 `-pix_fmt`（不硬编码 bgra）。
        let pix_fmt = target_pix_fmt(ctx).map_err(|e: ExportError| e.to_string())?;
        // ffmpeg 不会替我们建目录：输出文件的父目录先建好。
        if let Some(parent) = out.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("创建目录 {} 失败：{e}", parent.display()))?;
        }
        let child = Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "rawvideo",
                "-pix_fmt",
                pix_fmt,
                "-s",
                &format!("{width}x{height}"),
                "-r",
                &format!("{fps}"),
                "-i",
                "-",
                "-an",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(out)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("启动 ffmpeg 失败：{e}"))?;
        eprintln!("ffmpeg 管道已启动（{}），输出 {}", pix_fmt, out.display());
        Ok(Self {
            child,
            out: out.to_path_buf(),
        })
    }

    fn write(&mut self, pixels: &[u8]) -> Result<(), String> {
        let stdin = self.child.stdin.as_mut().ok_or("ffmpeg stdin 不可用")?;
        stdin
            .write_all(pixels)
            .map_err(|e| format!("写入 ffmpeg 失败：{e}"))
    }

    fn finish(mut self) -> Result<(), String> {
        drop(self.child.stdin.take());
        let status = self
            .child
            .wait()
            .map_err(|e| format!("等待 ffmpeg 失败：{e}"))?;
        if !status.success() {
            return Err(format!(
                "ffmpeg 退出码 {status}（输出 {}）",
                self.out.display()
            ));
        }
        eprintln!("完成：{}", self.out.display());
        Ok(())
    }
}
