//! `zela-render`：headless 薄壳 —— 脚本直接渲到视频。
//!
//! ```text
//! zela-render <脚本路径> [--frames DIR] [--fps N] [--out FILE] [--size WxH] [--hash]
//! zela-render --bundle <file.zsb> [--frames DIR] [--fps N] [--out FILE] [--size WxH] [--hash]
//! ```
//!
//! - 默认 **ffmpeg 管道**（rawvideo → libx264）；ffmpeg 不在 PATH ⇒ **明确报错**并提示 `--frames`；
//! - `--frames DIR` 走序列帧（PNG，交给外部工具合成）；
//! - **B8 Step 2**：逐帧哈希是一个**可选 sink**（`--hash` 打印、`--hash-out FILE` 打印并写文件）。
//!   主流程**默认不挂**它 ⇒ 不付哈希代价；挂与不挂的像素产出逐位相同（同一渲染核心）。
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

use zela_app::export::{ExportError, target_pix_fmt, write_png};
use zela_app::frame_loop::{
    FrameLoopOptions, FrameView, LoopBackend, frames_for, run_frames, timing_json,
};
use zela_app::{
    ScriptLanguage, build_bundle_runtime, build_script_runtime, language_from_path,
};
use zela_render_backends::{HeadlessTarget, RenderContext, StandardBackends};
use zela_scene_bundle::Bundle;
use zela_scene_bundle::frame_hash::FrameSequenceHash;

struct Args {
    script: Option<PathBuf>,
    bundle: Option<PathBuf>,
    frames_dir: Option<PathBuf>,
    hash_out: Option<PathBuf>,
    /// B8 Step 2：是否挂 `HashSink`（主流程默认**不挂**）。
    ///
    /// `--hash-out` 隐含开启（它要拿这个值写文件）；显式 `--hash` 只打印不写文件。
    hash: bool,
    fps: f64,
    out: PathBuf,
    size: (u32, u32),
    /// B11 Step 4：`--backend auto|auto-frame|wgpu|vector`。
    ///
    /// **默认仍是 `wgpu`**（不是 `auto`）：默认路径必须与 16 条基线逐位相同（C3/A1），
    /// 而 `auto` 会把矢量注册进 features 并集、并多取一份帧快照。
    /// 需要"按能力路由"时显式 `--backend auto`（页面默认就是 auto，见 §7.2）。
    backend: LoopBackend,
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
        let mut hash = false;
        let mut fps = 30.0f64;
        let mut out = PathBuf::from("out.mp4");
        let mut size = (640u32, 480u32);
        let mut backend = LoopBackend::Wgpu;

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
                // B8 Step 2：显式挂 HashSink（默认不挂 ⇒ 主流程不付哈希代价）。
                "--hash" => hash = true,
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
                // B11 Step 4：后端选择 / 路由偏好（默认 auto）。
                "--backend" => {
                    i += 1;
                    backend = raw
                        .get(i)
                        .and_then(|value| LoopBackend::parse(value))
                        .ok_or_else(|| {
                            format!(
                                "--backend 只接受 auto | auto-frame | wgpu | vector（收到 {:?}）",
                                raw.get(i)
                            )
                        })?;
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
            hash,
            fps,
            out,
            size,
            backend,
        })
    }
}

fn main() {
    // B12 Step 1.1：headless 入口接上订阅器（默认 warn；ZELA_LOG / ZELA_LOG_FILE 可调）。
    zela_schedule::telemetry::init_tracing_headless();
    if let Err(message) = run() {
        eprintln!("zela-render: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;

    // ── B11：注册表 → device features 并集 →（矢量可用时）注入同一块 device ──
    //
    // 显式 `--backend wgpu` 与改动前**逐位相同**：注册表只有 wgpu ⇒ features 为空集。
    let mut registry = zela_render_backends::BackendRegistry::with_standard();
    if args.backend != LoopBackend::Wgpu {
        registry
            .register(zela_render_backends::BackendKind::Vector)
            .map_err(|e| e.to_string())?;
    }
    let ctx = if args.backend == LoopBackend::Wgpu {
        RenderContext::headless().map_err(|e| format!("无法创建 headless GPU 上下文：{e}"))?
    } else {
        RenderContext::headless_with_registry(
            zela_render_backends::DEFAULT_TARGET_FORMAT,
            &registry,
        )
        .map_err(|e| format!("无法创建 headless GPU 上下文：{e}"))?
    };
    if args.backend != LoopBackend::Wgpu {
        let adapter = ctx
            .adapter()
            .ok_or_else(|| "上下文没有 adapter：无法判定后端可用性".to_string())?;
        for entry in registry.availability(adapter) {
            match entry.reason {
                Some(reason) => eprintln!("[backend] {:?} 不可用：{reason}", entry.kind),
                None => eprintln!("[backend] {:?} 可用", entry.kind),
            }
        }
        // 矢量可用 ⇒ 注入同一块 device（此后它**不再**自行 request_device）。
        if registry
            .select_on(zela_render_backends::BackendKind::Vector, adapter)
            .is_ok()
        {
            zela_render_vector::init_gpu(ctx.device.clone(), ctx.queue.clone(), adapter)?;
        }
    }

    let mut runtime = if let Some(script_path) = &args.script {
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
    let frames = frames_for(&runtime, args.fps);
    eprintln!(
        "渲染 {frames} 帧（{}×{} @ {} fps，时长 {:.3}s）…",
        width, height, args.fps, runtime.duration
    );

    // B2-6（D7）：**分层标签旁路** —— `ZELA_FRAME_DIGEST_SAMPLE=N` 时每 N 帧打印
    // 该帧的独立 digest。旁路不进入 `frame_hash`（判定与诊断分离）。
    let digest_sample: u32 = std::env::var("ZELA_FRAME_DIGEST_SAMPLE")
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
        .filter(|n| *n > 0)
        .unwrap_or(0);

    // B8 Step 2：sink 装配。输出 sink 恒有一个（PNG 或 mp4）；HashSink 只有显式要求才挂。
    let mut sinks: Vec<Box<dyn FrameSink>> = Vec::new();
    match &args.frames_dir {
        Some(dir) => sinks.push(Box::new(PngSink { dir: dir.clone() })),
        None => sinks.push(Box::new(FfmpegSink(Ffmpeg::spawn(
            &ctx,
            &args.out,
            args.fps,
            (width, height),
        )?))),
    }
    if args.hash || args.hash_out.is_some() {
        sinks.push(Box::new(HashSink {
            hash: FrameSequenceHash::new(width, height),
            size: (width, height),
            digest_sample,
            hash_out: args.hash_out.clone(),
            last: Duration::ZERO,
        }));
    }
    // 只有某个 sink 需要 RGBA8 时才做通道规范化（省一次全帧拷贝）。
    let need_rgba = sinks.iter().any(|sink| sink.wants_rgba());

    let mut backends = StandardBackends::new(&ctx);
    // B8 Step 0：目标纹理跨帧复用（原先每帧 create_texture）。
    let mut target = HeadlessTarget::new();

    // B8 Step 1：逐帧分段计时（默认只在窗口汇总 / 门禁上用；逐帧 JSON 由环境变量开启）。
    let timing_lines = std::env::var("ZELA_FRAME_TIMING").is_ok_and(|value| value != "0");
    let mut stats = zela_app::frame_loop::FrameStats::new();

    // B12 Step 2：决策日志需要一个作品标签（单进程路径取脚本 / 产物文件名）。
    let work_label = args
        .script
        .as_ref()
        .or(args.bundle.as_ref())
        .and_then(|path| path.file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "inline".to_string());

    // B9 Step 3：帧循环唯一实现在 `zela_app::frame_loop`（与 `zela-verify` 共用）。
    let run = run_frames(
        &ctx,
        &mut runtime,
        &mut backends,
        &mut target,
        FrameLoopOptions {
            width,
            height,
            fps: args.fps,
            backend: args.backend,
            vector_params: None,
        },
        need_rgba,
        &mut |index, frame| {
            // B2-6：旁路采样（只打印，不改序列哈希）。
            if digest_sample > 0 && index % digest_sample == 0 {
                let digest =
                    zela_scene_bundle::sha256::hex(&zela_scene_bundle::sha256::sha256(frame.rgba));
                eprintln!("[frame-digest] frame = {index} digest = {digest}");
            }
            for sink in &mut sinks {
                sink.on_frame(index, &frame)?;
            }
            Ok(sinks.iter().map(|sink| sink.last_hash_time()).sum())
        },
    )?;

    // B12 Step 2：决策落盘（与 B13 对拍同一份字段；拒绝理由非空由 `run_frames` 校验）。
    for decision in &run.decisions {
        eprintln!("{}", decision.json(&work_label, "render"));
    }
    // B12 Step 4：GPU 计时状态（`disabled` 是默认，不刷屏；其余都显式打印）。
    if run.gpu_status.state != zela_render_backends::gpu_timing::GpuTimingState::Disabled {
        eprintln!("[gpu-timing] {}", run.gpu_json());
    }

    for (index, timing) in run.timings.iter().enumerate() {
        if timing_lines {
            eprintln!("{}", timing_json(timing, index as u32));
        }
        stats.record(index as u32, timing);
    }

    // B8 Step 2：收尾交给 sink（`HashSink` 在这里打印 `[frame-hash]` 并写 `--hash-out`）。
    for sink in sinks {
        sink.finish()?;
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

/// 逐帧输出 / 校验的挂载点（B8 Step 2 / 补充更正 §3.4）。
///
/// 硬约束（V3-12）：渲染核心**不感知** sink —— 命令序列与像素产出与"挂了哪些 sink"
/// 完全无关。挂不挂 `HashSink` 只决定要不要多做一次哈希，不改走哪条渲染路径。
trait FrameSink {
    /// 是否需要 RGBA8（决定主循环要不要做通道规范化）。
    fn wants_rgba(&self) -> bool {
        false
    }

    fn on_frame(&mut self, index: u32, frame: &FrameView<'_>) -> Result<(), String>;

    /// 上一帧的校验耗时（只有 [`HashSink`] 报），用于把 `t_hash` 从 `t_sink` 里拆出来。
    fn last_hash_time(&self) -> Duration {
        Duration::ZERO
    }

    fn finish(self: Box<Self>) -> Result<(), String>;
}

/// PNG 序列（`--frames DIR`）。
struct PngSink {
    dir: PathBuf,
}

impl FrameSink for PngSink {
    fn wants_rgba(&self) -> bool {
        true
    }

    fn on_frame(&mut self, index: u32, frame: &FrameView<'_>) -> Result<(), String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|e| format!("创建目录 {} 失败：{e}", self.dir.display()))?;
        let path = self.dir.join(format!("frame_{index:05}.png"));
        write_png(&path, frame.size.0, frame.size.1, frame.rgba).map_err(|e| e.to_string())
    }

    fn finish(self: Box<Self>) -> Result<(), String> {
        eprintln!("完成：序列帧在 {}", self.dir.display());
        Ok(())
    }
}

/// mp4（`--out FILE`，经 ffmpeg 管道）。
struct FfmpegSink(Ffmpeg);

impl FrameSink for FfmpegSink {
    fn on_frame(&mut self, _index: u32, frame: &FrameView<'_>) -> Result<(), String> {
        self.0.write(frame.raw)
    }

    fn finish(self: Box<Self>) -> Result<(), String> {
        self.0.finish()
    }
}

/// 帧序列哈希（**可选**；`--hash` 或 `--hash-out` 才挂）。
///
/// 这是唯一会做像素级计算（SHA-256）的 sink；它的存在与否不改变像素产出。
struct HashSink {
    hash: FrameSequenceHash,
    size: (u32, u32),
    digest_sample: u32,
    hash_out: Option<PathBuf>,
    last: Duration,
}

impl FrameSink for HashSink {
    fn wants_rgba(&self) -> bool {
        true
    }

    fn on_frame(&mut self, index: u32, frame: &FrameView<'_>) -> Result<(), String> {
        // B2-6：旁路采样（只打印，不改序列哈希）。
        if self.digest_sample > 0 && index % self.digest_sample == 0 {
            let digest =
                zela_scene_bundle::sha256::hex(&zela_scene_bundle::sha256::sha256(frame.rgba));
            eprintln!("[frame-digest] frame = {index} digest = {digest}");
        }
        let mark = Instant::now();
        self.hash.push_frame(frame.rgba);
        self.last = mark.elapsed();
        Ok(())
    }

    fn last_hash_time(&self) -> Duration {
        self.last
    }

    fn finish(self: Box<Self>) -> Result<(), String> {
        let sequence = self.hash.hex();
        eprintln!(
            "[frame-hash] sequence = {sequence}（{} 帧，{}×{}，RGBA8）",
            self.hash.frames(),
            self.size.0,
            self.size.1
        );
        if let Some(path) = &self.hash_out {
            if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("创建目录 {} 失败：{e}", parent.display()))?;
            }
            std::fs::write(path, format!("{sequence}\n"))
                .map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
        }
        Ok(())
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
