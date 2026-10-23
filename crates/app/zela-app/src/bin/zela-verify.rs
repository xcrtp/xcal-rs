//! `zela-verify`：**批量**对拍入口（B9 Step 3）。
//!
//! ```text
//! zela-verify --scripts DIR --tmp DIR --work name=WxH[:vector] [--work ...]
//! ```
//!
//! # 为什么需要它
//!
//! `cargo verify --step hashes` 原先每个作品要付两次进程启动（每次 ~0.5s GPU 初始化
//! + ~0.7s 运行时构帧准备）。19 次进程调用 ≈ 46s，与"渲染本身"同量级。
//! 本二进制把 GPU 初始化压成**一次**，每个作品只重建自己的 Runtime（新 World 是必须的）。
//!
//! # 与单进程路径的关系（不得漂移）
//!
//! 帧循环**只此一份**（[`zela_app::frame_loop`]），与 `zela-render` 共用；
//! 本二进制只是"同一循环 + 一个哈希回调"，不重写渲染代码。
//! `xtask` 保留 `--independent` 开关走旧的独立进程路径 —— 两者必须逐位相同（B9 §4.3）。
//!
//! # 输出（机器可读，一行一个作品）
//!
//! ```text
//! [verify] {"work":"fourier","size":"640x480","frames":216,"script":"<64 hex>","bundle":"<64 hex>","payload":"<64 hex>"}
//! ```
//!
//! 失败时**点名**作品与路径（`work · script|bundle`），不报"批量失败"。

use std::path::{Path, PathBuf};

use zela_app::frame_loop::{
    FrameLoopOptions, FrameStats, FrameTiming, LoopBackend, frames_for, run_frames, timing_json,
};
use zela_app::{
    ScriptLanguage, build_bundle_runtime, build_script_runtime, compile_script, language_from_path,
};
use zela_render_backends::{HeadlessTarget, RenderContext, StandardBackends};
use zela_scene_bundle::frame_hash::FrameSequenceHash;
use zela_scene_bundle::{Bundle, MetaInput};

/// 一个待对拍的作品。
struct Work {
    name: String,
    width: u32,
    height: u32,
    backend: LoopBackend,
}

struct Args {
    scripts: PathBuf,
    tmp: PathBuf,
    fps: f64,
    works: Vec<Work>,
    /// B7 Step 0.3：把 V3-12（挂/不挂 HashSink 的像素一致性）并入本进程。
    sink_bypass: bool,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let raw: Vec<String> = std::env::args().skip(1).collect();
        if raw.is_empty() || raw.iter().any(|a| a == "--help" || a == "-h") {
            return Err(
                "用法：zela-verify --scripts DIR --tmp DIR --work name=WxH[:vector] [--work ...]\n\
                 　　：zela-verify --scripts DIR --tmp DIR --sink-bypass（V3-12 检查）"
                    .to_string(),
            );
        }
        let mut scripts: Option<PathBuf> = None;
        let mut tmp: Option<PathBuf> = None;
        let mut fps = 30.0f64;
        let mut works = Vec::new();
        let mut sink_bypass = false;
        let mut i = 0;
        while i < raw.len() {
            match raw[i].as_str() {
                "--scripts" => {
                    i += 1;
                    scripts = Some(PathBuf::from(
                        raw.get(i).ok_or("--scripts 需要一个目录参数")?,
                    ));
                }
                "--tmp" => {
                    i += 1;
                    tmp = Some(PathBuf::from(raw.get(i).ok_or("--tmp 需要一个目录参数")?));
                }
                "--fps" => {
                    i += 1;
                    fps = raw
                        .get(i)
                        .ok_or("--fps 需要一个数字参数")?
                        .parse()
                        .map_err(|_| "--fps 必须是数字".to_string())?;
                }
                "--work" => {
                    i += 1;
                    works.push(parse_work(raw.get(i).ok_or("--work 需要 name=WxH[:vector]")?)?);
                }
                "--sink-bypass" => sink_bypass = true,
                other => return Err(format!("未知参数：{other}")),
            }
            i += 1;
        }
        Ok(Self {
            scripts: scripts.ok_or("缺少 --scripts")?,
            tmp: tmp.ok_or("缺少 --tmp")?,
            fps,
            works,
            sink_bypass,
        })
    }
}

/// `fourier=640x480` / `vector_paths=640x480:vector`。
fn parse_work(spec: &str) -> Result<Work, String> {
    let (name, rest) = spec
        .split_once('=')
        .ok_or_else(|| format!("--work 需要一个 name=WxH[:backend]：{spec}"))?;
    let (size, backend) = match rest.split_once(':') {
        Some((size, "vector")) => (size, LoopBackend::Vector),
        Some((size, "wgpu")) => (size, LoopBackend::Wgpu),
        Some((_, other)) => return Err(format!("--work 的后端只接受 wgpu|vector：{other}")),
        None => (rest, LoopBackend::Wgpu),
    };
    let (w, h) = size
        .split_once('x')
        .ok_or_else(|| format!("--work 的尺寸必须是 WxH：{size}"))?;
    Ok(Work {
        name: name.to_string(),
        width: w.parse().map_err(|_| format!("宽度不是数字：{w}"))?,
        height: h.parse().map_err(|_| format!("高度不是数字：{h}"))?,
        backend,
    })
}

fn main() {
    // B12 Step 1.1：门禁路径也接上订阅器（此前默认采集、默认丢弃）。
    zela_schedule::telemetry::init_tracing_headless();
    if let Err(message) = run() {
        eprintln!("zela-verify: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;
    std::fs::create_dir_all(&args.tmp)
        .map_err(|e| format!("创建目录 {} 失败：{e}", args.tmp.display()))?;

    // **一次** GPU 初始化：19 次进程调用的固定开销在这里被摊平。
    let ctx = RenderContext::headless().map_err(|e| format!("无法创建 headless GPU 上下文：{e}"))?;

    if args.sink_bypass {
        return run_sink_bypass(&ctx, &args);
    }

    for work in &args.works {
        let (script_path, language) = locate_script(&args.scripts, &work.name)?;
        let source = std::fs::read_to_string(&script_path)
            .map_err(|e| format!("[{}] 读取脚本 {} 失败：{e}", work.name, script_path.display()))?;

        // ── 编译（§4.5：也在同一进程里，避免 7 次 `zela-compile` 的启动开销）──
        let payload = compile_and_write(
            &work.name,
            &source,
            language,
            &args.tmp.join(format!("{}.zsb", work.name)),
        )?;

        // ── 路径 1：脚本 ──
        let mut runtime = build_script_runtime(ctx.clone(), source, language)
            .map_err(|e| format!("[{}] 脚本装配失败：{e}", work.name))?;
        let script_hash = render_hash(&ctx, &mut runtime, work, args.fps, "script")?;
        drop(runtime);

        // ── 路径 2：同一份 ZSB ──
        let bytes = std::fs::read(args.tmp.join(format!("{}.zsb", work.name)))
            .map_err(|e| format!("[{}] 读取 ZSB 失败：{e}", work.name))?;
        let bundle = Bundle::from_bytes(&bytes)
            .map_err(|e| format!("[{}] ZSB 解析失败：{e}", work.name))?;
        let mut runtime = build_bundle_runtime(ctx.clone(), &bundle)
            .map_err(|e| format!("[{}] bundle 装配失败：{e}", work.name))?;
        let bundle_hash = render_hash(&ctx, &mut runtime, work, args.fps, "bundle")?;

        println!(
            "[verify] {{\"work\":\"{}\",\"size\":\"{}x{}\",\"frames\":{},\"script\":\"{}\",\
             \"bundle\":\"{}\",\"payload\":\"{}\"}}",
            work.name,
            work.width,
            work.height,
            frames_for(&runtime, args.fps),
            script_hash,
            bundle_hash,
            payload,
        );
    }
    Ok(())
}

/// 脚本 → ZSB（与 `zela-compile` 同一装配路径），返回 payload hash。
fn compile_and_write(
    name: &str,
    source: &str,
    language: ScriptLanguage,
    out: &Path,
) -> Result<String, String> {
    let compiled = compile_script(source, language)
        .map_err(|e| format!("[{name}] 编译失败：{e}"))?;
    let bundle = Bundle::encode(
        &compiled.script,
        MetaInput {
            capabilities: compiled.capabilities.clone(),
            required_capabilities: compiled.required_capabilities.clone(),
            pods: compiled.pods.clone(),
            required_pods: compiled.required_pods.clone(),
            downgrades: Vec::new(),
            source_sha256: Some(compiled.source_sha256.clone()),
            render_params: Default::default(),
        },
    )
    .map_err(|e| format!("[{name}] 编码 ZSB 失败：{e}"))?;
    std::fs::write(out, bundle.to_bytes())
        .map_err(|e| format!("[{name}] 写 {} 失败：{e}", out.display()))?;
    Ok(bundle.payload_hash_hex())
}

/// 按 `.lua` / `.py` / `.js` 顺序探测作品脚本（B7 Step 0.1：不再硬编码扩展名）。
///
/// B7 Step 3：作品名**自带扩展名**时按显式文件名取（`surface_matrix.js` 与
/// `surface_matrix.lua` 并存，探测顺序选不出后者）。
///
/// 语义不变：**找不到 ⇒ 显式报错并列出试过的扩展名**（不静默跳过）。
/// 语言判断复用 [`language_from_path`]，不另写一套。
fn locate_script(dir: &Path, name: &str) -> Result<(PathBuf, ScriptLanguage), String> {
    if let Some(extension) = Path::new(name).extension().and_then(|ext| ext.to_str()) {
        if matches!(extension, "lua" | "py" | "js") {
            let path = dir.join(name);
            if path.is_file() {
                let language = language_from_path(&path).map_err(|e| e.to_string())?;
                return Ok((path, language));
            }
            return Err(format!(
                "[{name}] 显式指定了扩展名，但 {} 不存在",
                path.display()
            ));
        }
    }
    let mut tried = Vec::new();
    for ext in ["lua", "py", "js"] {
        let path = dir.join(format!("{name}.{ext}"));
        if path.is_file() {
            let language = language_from_path(&path).map_err(|e| e.to_string())?;
            return Ok((path, language));
        }
        tried.push(ext);
    }
    Err(format!(
        "[{name}] 找不到脚本：{} 下没有 .{} 中的任何一个",
        dir.display(),
        tried.join(" / .")
    ))
}

/// **V3-12 检查**（并入本进程，B7 Step 0.3）：挂 / 不挂哈希时的像素产出必须逐位相同。
///
/// 比 PNG **字节**而不是序列哈希 —— 不挂哈希时本来就没有哈希值可拿，比它是自证。
fn run_sink_bypass(ctx: &RenderContext, args: &Args) -> Result<(), String> {
    let work = Work {
        name: "vector_paths".to_string(),
        width: 640,
        height: 480,
        backend: LoopBackend::Vector,
    };
    let (script_path, language) = locate_script(&args.scripts, &work.name)?;
    let source = std::fs::read_to_string(&script_path)
        .map_err(|e| format!("读取脚本 {} 失败：{e}", script_path.display()))?;
    let root = args.tmp.join("sink-bypass");
    let mut sizes = Vec::new();
    for (name, hash) in [("with-hash", true), ("no-hash", false)] {
        let dir = root.join(name);
        sizes.push(render_png(ctx, &source, language, &work, args.fps, &dir, hash)?.len());
    }
    if sizes[0] != sizes[1] {
        return Err(format!(
            "V3-12 失败：挂 / 不挂 HashSink 的 PNG 字节不同（{} B vs {} B）",
            sizes[0], sizes[1]
        ));
    }
    // 比字节本身：只比长度不够，读回来再比一次。
    let frame = "frame_00000.png";
    let a = std::fs::read(root.join("with-hash").join(frame))
        .map_err(|e| format!("读取 with-hash 帧失败：{e}"))?;
    let b = std::fs::read(root.join("no-hash").join(frame))
        .map_err(|e| format!("读取 no-hash 帧失败：{e}"))?;
    if a != b {
        return Err("V3-12 失败：挂 / 不挂 HashSink 的 PNG 字节不同".to_string());
    }
    println!(
        "[verify] {{\"sink_bypass\":\"ok\",\"bytes\":{}}}",
        a.len()
    );
    Ok(())
}

/// B12 Step 1.2：`zela-verify` 是**唯一默认门禁路径**，必须与 `zela-render` 同字段计时。
///
/// 默认不打印（门禁输出要保持可读）；`ZELA_FRAME_TIMING=1` 时逐帧 `[timing]` + 每遍
/// `[timing-summary]`，字段名与 `zela-render` 完全一致。
fn report_timings(work: &str, pass: &str, timings: &[FrameTiming]) {
    if !std::env::var("ZELA_FRAME_TIMING").is_ok_and(|value| value != "0") {
        return;
    }
    let mut stats = FrameStats::new();
    for (index, timing) in timings.iter().enumerate() {
        eprintln!("{}", timing_json(timing, index as u32));
        stats.record(index as u32, timing);
    }
    eprintln!("[timing-work] {work} · {pass}");
    if let Err(error) = stats.report() {
        // 量级门禁在 Step 6 才接入退出码；这里只呈报。
        eprintln!("[timing] {error}");
    }
}

/// 渲一个作品并把 PNG 写进 `dir`；`hash` 决定是否同时累积序列哈希。
fn render_png(
    ctx: &RenderContext,
    source: &str,
    language: ScriptLanguage,
    work: &Work,
    fps: f64,
    dir: &Path,
    hash: bool,
) -> Result<Vec<u8>, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("创建目录 {} 失败：{e}", dir.display()))?;
    let mut runtime = build_script_runtime(ctx.clone(), source, language)
        .map_err(|e| format!("[{}] 装配失败：{e}", work.name))?;
    let mut backends = StandardBackends::new(ctx);
    let mut target = HeadlessTarget::new();
    let mut sequence = FrameSequenceHash::new(work.width, work.height);
    let mut first = Vec::new();
    let options = FrameLoopOptions {
        width: work.width,
        height: work.height,
        fps,
        backend: work.backend,
        vector_params: None,
    };
    let timings = run_frames(
        ctx,
        &mut runtime,
        &mut backends,
        &mut target,
        options,
        true,
        &mut |index, frame| {
            // 与 `PngSink` 同一个写盘函数（同一套编码参数）。
            let path = dir.join(format!("frame_{index:05}.png"));
            zela_app::export::write_png(&path, work.width, work.height, frame.rgba)
                .map_err(|e| e.to_string())?;
            if index == 0 {
                first = std::fs::read(&path).map_err(|e| format!("读回 {} 失败：{e}", path.display()))?;
            }
            if hash {
                sequence.push_frame(frame.rgba);
            }
            Ok(std::time::Duration::ZERO)
        },
    )
    .map_err(|e| format!("[{}] 渲染失败：{e}", work.name))?;
    report_timings(&work.name, "png", &timings);
    Ok(first)
}

/// 一个作品的一遍渲染 → 序列哈希。
///
/// **每个作品新建 `StandardBackends`**（§4.2 的安全取舍）：`wgpu_3d` 的离屏 / 深度目标
/// 按 `LayerId` 索引，复用后端可能让残留目标串味。GPU 初始化（大头）已在外面摊平。
fn render_hash(
    ctx: &RenderContext,
    runtime: &mut zela_playback::Runtime,
    work: &Work,
    fps: f64,
    path: &str,
) -> Result<String, String> {
    let mut backends = StandardBackends::new(ctx);
    let mut target = HeadlessTarget::new();
    let mut hash = FrameSequenceHash::new(work.width, work.height);
    let options = FrameLoopOptions {
        width: work.width,
        height: work.height,
        fps,
        backend: work.backend,
        vector_params: None,
    };
    let timings = run_frames(
        ctx,
        runtime,
        &mut backends,
        &mut target,
        options,
        true,
        &mut |_index, frame| {
            // 与 `zela-render --hash` 完全相同的输入与口径。
            hash.push_frame(frame.rgba);
            Ok(std::time::Duration::ZERO)
        },
    )
    .map_err(|e| format!("[{}] {path} 渲染失败：{e}", work.name))?;
    report_timings(&work.name, path, &timings);
    Ok(hash.hex())
}
