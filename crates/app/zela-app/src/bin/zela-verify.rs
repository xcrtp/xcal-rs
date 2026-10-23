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

use zela_app::frame_loop::{FrameLoopOptions, LoopBackend, frames_for, run_frames};
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
}

impl Args {
    fn parse() -> Result<Self, String> {
        let raw: Vec<String> = std::env::args().skip(1).collect();
        if raw.is_empty() || raw.iter().any(|a| a == "--help" || a == "-h") {
            return Err(
                "用法：zela-verify --scripts DIR --tmp DIR --work name=WxH[:vector] [--work ...]"
                    .to_string(),
            );
        }
        let mut scripts: Option<PathBuf> = None;
        let mut tmp: Option<PathBuf> = None;
        let mut fps = 30.0f64;
        let mut works = Vec::new();
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
                other => return Err(format!("未知参数：{other}")),
            }
            i += 1;
        }
        Ok(Self {
            scripts: scripts.ok_or("缺少 --scripts")?,
            tmp: tmp.ok_or("缺少 --tmp")?,
            fps,
            works,
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

    for work in &args.works {
        let script_path = args.scripts.join(format!("{}.lua", work.name));
        let source = std::fs::read_to_string(&script_path)
            .map_err(|e| format!("[{}] 读取脚本 {} 失败：{e}", work.name, script_path.display()))?;

        // ── 编译（§4.5：也在同一进程里，避免 7 次 `zela-compile` 的启动开销）──
        let language = language_from_path(&script_path).map_err(|e| e.to_string())?;
        if language != ScriptLanguage::Lua {
            return Err(format!(
                "[{}] 批量对拍目前只接受 .lua（收到 {language:?}）",
                work.name
            ));
        }
        let payload = compile_and_write(&work.name, &source, &args.tmp.join(format!("{}.zsb", work.name)))?;

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
fn compile_and_write(name: &str, source: &str, out: &Path) -> Result<String, String> {
    let compiled = compile_script(source, ScriptLanguage::Lua)
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
    };
    run_frames(
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
    Ok(hash.hex())
}
