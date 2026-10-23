//! `zela-parity`：**作品级**双后端对拍（B13 Step 1/2）。
//!
//! ```text
//! zela-parity --scripts DIR [--fps 30] [--sample 30] [--post match|backend-default]
//!             [--out DIR] --work name=WxH [--work ...]
//! ```
//!
//! `--post` 决定差额里有多少是**后处理政策**：`backend-default` 让矢量按自身能力开后处理
//! （tone/bloom/ao），wgpu headless 则全关；`match` 两侧用同一份 `RenderParams::default()`
//! ⇒ 只比渲染算法。两种口径的结果都要落档（B13 §6.2：系统性偏移必须解释）。
//!
//! # 为什么需要它
//!
//! B13 之前 11 件矢量作品的唯一质量检查是"抽检中位数不为 0"（没有黑帧）——
//! 它画得**对不对**完全不知道。`vector_vs_wgpu.rs` 的判据（覆盖率 / 全图 MAE /
//! 内部区 MAE）只覆盖手工搓的几何，没有覆盖真实作品。
//!
//! 本工具把同一套判据用到**作品**上：同一脚本站`run_frames`（唯一帧循环）跑两遍，
//! 一遍 wgpu、一遍矢量，逐采样帧比较。它不新写渲染路径，也不改任何渲染语义。
//!
//! # 判据（不得放宽，B13 §5.2）
//!
//! - 覆盖率（非背景像素数）相对差 ≤ 10%；
//! - 逐像素 RGB MAE（全图）≤ 8/255；
//! - **内部区** MAE ≤ 2/255 —— 内部区只该受色彩管线影响，不该受抗锯齿影响。
//!
//! 另报差异 bbox、差异像素占比、平均有符号偏差（系统性偏移 vs 局部差异的判据）。
//!
//! # 输出
//!
//! `--out` 目录下每件作品一个 `<work>.json`（逐采样帧指标 + 决策日志），
//! 以及 `summary.md`（落档用的一览表）。stdout 一行一个作品：
//!
//! ```text
//! [parity] {"work":"fourier","status":"ok","frames":216,"sampled":8,...}
//! ```

use std::path::{Path, PathBuf};
use std::time::Duration;

use zela_app::frame_loop::{
    FrameLoopOptions, LoopBackend, frames_for, run_frames,
};
use zela_app::{ScriptLanguage, build_script_runtime, language_from_path};
use zela_render_backends::{HeadlessTarget, RenderContext, RoutingDecision, StandardBackends};

/// 判据（B13 §5.2）：覆盖率相对差 / 全图 MAE / 内部区 MAE。
const COVERAGE_REL_MAX: f64 = 0.10;
const MAE_MAX: f64 = 8.0;
const INTERIOR_MAE_MAX: f64 = 2.0;

/// "非背景"的判据：任一通道 > 24（与 `vector_vs_wgpu.rs` 同口径）。
fn covered(frame: &[u8], index: usize) -> bool {
    let p = &frame[index * 4..index * 4 + 3];
    p[0] > 24 || p[1] > 24 || p[2] > 24
}

fn mae_at(wgpu: &[u8], vector: &[u8], index: usize) -> f64 {
    let p = &wgpu[index * 4..index * 4 + 4];
    let q = &vector[index * 4..index * 4 + 4];
    (0..3)
        .map(|k| (p[k] as f64 - q[k] as f64).abs())
        .sum::<f64>()
        / 3.0
}

/// 一帧的差异指标。
#[derive(Clone)]
struct FrameMetrics {
    frame: u32,
    coverage_wgpu: usize,
    coverage_vector: usize,
    coverage_rel: f64,
    mae: f64,
    interior_mae: f64,
    edge_mae: f64,
    interior_px: usize,
    edge_px: usize,
    diff_px: usize,
    diff_px_pct: f64,
    /// 差异 bbox（`max|Δ| > 8` 的像素外接矩形）：`[x0, y0, x1, y1]`。
    bbox: Option<[u32; 4]>,
    /// 被覆盖像素上的**有符号**平均偏差（R/G/B）。系统性偏亮/偏暗会在这里露出来。
    bias: [f64; 3],
}

impl FrameMetrics {
    fn failures(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.coverage_rel > COVERAGE_REL_MAX {
            out.push(format!(
                "覆盖率相对差 {:.3} > {COVERAGE_REL_MAX}（wgpu={} vector={}）",
                self.coverage_rel, self.coverage_wgpu, self.coverage_vector
            ));
        }
        if self.mae > MAE_MAX {
            out.push(format!("全图 MAE {:.2} > {MAE_MAX}", self.mae));
        }
        if self.interior_mae > INTERIOR_MAE_MAX {
            out.push(format!(
                "内部区 MAE {:.2} > {INTERIOR_MAE_MAX}（{} px）",
                self.interior_mae, self.interior_px
            ));
        }
        out
    }
}

fn metrics(w: u32, h: u32, frame: u32, wgpu: &[u8], vector: &[u8]) -> FrameMetrics {
    let total = (w * h) as usize;
    let index_of = |x: u32, y: u32| (y * w + x) as usize;

    let coverage_wgpu = (0..total).filter(|i| covered(wgpu, *i)).count();
    let coverage_vector = (0..total).filter(|i| covered(vector, *i)).count();
    let relative = (coverage_wgpu as f64 - coverage_vector as f64).abs()
        / coverage_wgpu.max(coverage_vector).max(1) as f64;

    let mae: f64 = (0..total).map(|i| mae_at(wgpu, vector, i)).sum::<f64>() / total as f64;

    // 内部 / 边缘 / 差异像素：以**两帧并集**为覆盖掩码（避免系统性偏移被误判为"未覆盖"）。
    let mask: Vec<bool> = (0..total)
        .map(|i| covered(wgpu, i) || covered(vector, i))
        .collect();
    let mut interior = (0usize, 0.0f64);
    let mut edge = (0usize, 0.0f64);
    let mut diff_px = 0usize;
    let mut bbox: Option<[u32; 4]> = None;
    let mut bias = [0.0f64; 3];
    let mut covered_px = 0usize;

    for y in 0..h {
        for x in 0..w {
            let i = index_of(x, y);
            if !mask[i] {
                continue;
            }
            covered_px += 1;
            for k in 0..3 {
                bias[k] += wgpu[i * 4 + k] as f64 - vector[i * 4 + k] as f64;
            }
            let px_diff = (0..3)
                .map(|k| (wgpu[i * 4 + k] as f64 - vector[i * 4 + k] as f64).abs())
                .fold(0.0f64, f64::max);
            if px_diff > 8.0 {
                diff_px += 1;
                bbox = Some(match bbox {
                    None => [x, y, x, y],
                    Some(b) => [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)],
                });
            }
            let is_edge = [
                (x.wrapping_sub(1), y),
                (x + 1, y),
                (x, y.wrapping_sub(1)),
                (x, y + 1),
            ]
            .into_iter()
            .filter(|&(nx, ny)| nx < w && ny < h)
            .any(|(nx, ny)| !mask[index_of(nx, ny)]);
            let bucket = if is_edge { &mut edge } else { &mut interior };
            bucket.0 += 1;
            bucket.1 += mae_at(wgpu, vector, i);
        }
    }

    let mean = |(count, sum): (usize, f64)| if count == 0 { 0.0 } else { sum / count as f64 };
    if covered_px > 0 {
        for value in &mut bias {
            *value /= covered_px as f64;
        }
    }

    FrameMetrics {
        frame,
        coverage_wgpu,
        coverage_vector,
        coverage_rel: relative,
        mae,
        interior_mae: mean(interior),
        edge_mae: mean(edge),
        interior_px: interior.0,
        edge_px: edge.0,
        diff_px,
        diff_px_pct: if covered_px == 0 {
            0.0
        } else {
            diff_px as f64 * 100.0 / covered_px as f64
        },
        bbox,
        bias,
    }
}

struct Work {
    name: String,
    width: u32,
    height: u32,
}

/// 渲染参数口径（决定差异里有多少是"后处理政策"而不是"渲染算法"）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum PostMode {
    /// 两侧用**同一份** `RenderParams::default()`（后处理全关）：只比渲染算法。
    Match,
    /// 让每个后端按自己的能力取默认值（矢量 = tone/bloom/ao 全开，wgpu = 全关）：
    /// 这是生产路径的口径，差异会包含后处理政策。
    BackendDefault,
}

impl PostMode {
    fn name(self) -> &'static str {
        match self {
            PostMode::Match => "match",
            PostMode::BackendDefault => "backend-default",
        }
    }
}

struct Args {
    scripts: PathBuf,
    fps: f64,
    sample: u32,
    out: PathBuf,
    post: PostMode,
    /// 把每件作品"最差帧"的两张图落盘（PNG），用于人工解释差异。
    dump: bool,
    works: Vec<Work>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let raw: Vec<String> = std::env::args().skip(1).collect();
        if raw.is_empty() || raw.iter().any(|a| a == "--help" || a == "-h") {
            return Err(
                "用法：zela-parity --scripts DIR [--fps 30] [--sample 30] [--out DIR] \
                 --work name=WxH [--work ...]"
                    .to_string(),
            );
        }
        let mut scripts = None;
        let mut fps = 30.0f64;
        let mut sample = 30u32;
        let mut out = PathBuf::from("target/xtask/parity");
        let mut post = PostMode::Match;
        let mut dump = false;
        let mut works = Vec::new();
        let mut i = 0;
        while i < raw.len() {
            match raw[i].as_str() {
                "--scripts" => {
                    i += 1;
                    scripts = Some(PathBuf::from(raw.get(i).ok_or("--scripts 需要一个目录")?));
                }
                "--fps" => {
                    i += 1;
                    fps = raw
                        .get(i)
                        .ok_or("--fps 需要一个数字")?
                        .parse()
                        .map_err(|_| "--fps 必须是数字".to_string())?;
                }
                "--sample" => {
                    i += 1;
                    sample = raw
                        .get(i)
                        .ok_or("--sample 需要一个数字")?
                        .parse()
                        .map_err(|_| "--sample 必须是数字".to_string())?;
                }
                "--out" => {
                    i += 1;
                    out = PathBuf::from(raw.get(i).ok_or("--out 需要一个目录")?);
                }
                "--post" => {
                    i += 1;
                    post = match raw.get(i).map(String::as_str) {
                        Some("match") => PostMode::Match,
                        Some("backend-default") => PostMode::BackendDefault,
                        other => {
                            return Err(format!(
                                "--post 只接受 match | backend-default（收到 {other:?}）"
                            ));
                        }
                    };
                }
                "--dump" => dump = true,
                "--work" => {
                    i += 1;
                    works.push(parse_work(raw.get(i).ok_or("--work 需要 name=WxH")?)?);
                }
                other => return Err(format!("未知参数：{other}")),
            }
            i += 1;
        }
        if sample == 0 {
            return Err("--sample 必须 > 0".to_string());
        }
        Ok(Self {
            scripts: scripts.ok_or("缺少 --scripts")?,
            fps,
            sample,
            out,
            post,
            dump,
            works,
        })
    }
}

fn parse_work(spec: &str) -> Result<Work, String> {
    let (name, size) = spec
        .split_once('=')
        .ok_or_else(|| format!("--work 需要 name=WxH：{spec}"))?;
    let (w, h) = size
        .split_once('x')
        .ok_or_else(|| format!("--work 尺寸必须是 WxH：{size}"))?;
    Ok(Work {
        name: name.to_string(),
        width: w.parse().map_err(|_| format!("宽度不是数字：{w}"))?,
        height: h.parse().map_err(|_| format!("高度不是数字：{h}"))?,
    })
}

/// 与 `zela-verify` 同一探测顺序（显式扩展名优先）—— 诊断工具各自独立，
/// 但**顺序**必须一致，否则同一作品在两处会选到不同脚本。
fn locate_script(dir: &Path, name: &str) -> Result<(PathBuf, ScriptLanguage), String> {
    if let Some(extension) = Path::new(name).extension().and_then(|ext| ext.to_str()) {
        if matches!(extension, "lua" | "py" | "js") {
            let path = dir.join(name);
            if path.is_file() {
                return Ok((path.clone(), language_from_path(&path).map_err(|e| e.to_string())?));
            }
            return Err(format!("[{name}] 指定的脚本不存在：{}", path.display()));
        }
    }
    for ext in ["lua", "py", "js"] {
        let path = dir.join(format!("{name}.{ext}"));
        if path.is_file() {
            return Ok((path.clone(), language_from_path(&path).map_err(|e| e.to_string())?));
        }
    }
    Err(format!("[{name}] 在 {} 下找不到 .lua / .py / .js", dir.display()))
}

fn main() {
    zela_schedule::telemetry::init_tracing_headless();
    if let Err(message) = run() {
        eprintln!("zela-parity: {message}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = Args::parse()?;
    if args.works.is_empty() {
        return Err("至少要有一个 --work".to_string());
    }
    std::fs::create_dir_all(&args.out)
        .map_err(|e| format!("创建 {} 失败：{e}", args.out.display()))?;

    // 与 `zela-render --backend vector` 同一条装配：device features 按"已注册且可用"后端之并
    // （矢量要 FLOAT32_BLENDABLE），否则建管线时 R32Float 不可混合 ⇒ 直接报错。
    let mut registry = zela_render_backends::BackendRegistry::with_standard();
    registry
        .register(zela_render_backends::BackendKind::Vector)
        .map_err(|e| e.to_string())?;
    let ctx = RenderContext::headless_with_registry(
        zela_render_backends::DEFAULT_TARGET_FORMAT,
        &registry,
    )
    .map_err(|e| format!("无法创建 headless GPU 上下文：{e}"))?;
    let adapter = ctx
        .adapter()
        .cloned()
        .ok_or_else(|| "headless 上下文没有 adapter（能力校验无法进行）".to_string())?;
    registry
        .select_on(zela_render_backends::BackendKind::Vector, &adapter)
        .map_err(|e| format!("矢量后端在本机不可用：{e}"))?;
    zela_render_vector::init_gpu(ctx.device.clone(), ctx.queue.clone(), &adapter)
        .map_err(|e| format!("矢量后端注入失败：{e}"))?;

    let mut summary: Vec<(String, u32, usize, FrameMetrics, bool, String)> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for work in &args.works {
        let (script_path, language) = locate_script(&args.scripts, &work.name)?;
        let source = std::fs::read_to_string(&script_path)
            .map_err(|e| format!("[{}] 读取 {} 失败：{e}", work.name, script_path.display()))?;

        let wgpu = render_pass(&ctx, &source, language, work, &args, LoopBackend::Wgpu)?;
        println!(
            "[parity] {}：wgpu {} 帧（采样 {}）…",
            work.name,
            wgpu.frames,
            wgpu.samples.len()
        );
        let vector = match render_pass(&ctx, &source, language, work, &args, LoopBackend::Vector) {
            Ok(pass) => pass,
            Err(error) => {
                // 拒绝渲染不是"没测"：把理由原样落档（B13 §5.3 的能力边界）。
                println!(
                    "[parity] {{\"work\":\"{}\",\"status\":\"rejected\",\"reason\":\"{}\"}}",
                    work.name,
                    escape(&error)
                );
                failures.push(format!("{}: 矢量后端拒绝渲染：{error}", work.name));
                continue;
            }
        };

        let mut worst: Option<FrameMetrics> = None;
        let mut worst_pair: Option<(u32, Vec<u8>, Vec<u8>)> = None;
        let mut rows = Vec::new();
        for (frame, wgpu_px) in &wgpu.samples {
            let Some(vector_px) = vector
                .samples
                .iter()
                .find(|(index, _)| index == frame)
                .map(|(_, px)| px)
            else {
                continue;
            };
            let m = metrics(work.width, work.height, *frame, wgpu_px, vector_px);
            if worst
                .as_ref()
                .is_none_or(|current| m.mae > current.mae)
            {
                worst = Some(m.clone());
                worst_pair = Some((*frame, wgpu_px.clone(), vector_px.clone()));
            }
            rows.push(m);
        }
        let Some(worst) = worst else {
            return Err(format!("[{}] 两条路径没有可比较的采样帧", work.name));
        };
        if args.dump {
            if let Some((frame, wgpu_px, vector_px)) = &worst_pair {
                for (suffix, pixels) in [("wgpu", wgpu_px), ("vector", vector_px)] {
                    let path = args
                        .out
                        .join(format!("{}-f{frame:05}-{suffix}.png", work.name));
                    zela_app::export::write_png(&path, work.width, work.height, pixels)
                        .map_err(|e| format!("写 {} 失败：{e}", path.display()))?;
                }
            }
        }

        let problems = worst.failures();
        let status = if problems.is_empty() { "ok" } else { "fail" };
        let decisions = format!(
            "{{\"wgpu\":{}, \"vector\":{}}}",
            decisions_json(&wgpu.decisions),
            decisions_json(&vector.decisions)
        );
        println!(
            "[parity] {{\"work\":\"{}\",\"status\":\"{status}\",\"post\":\"{}\",\
             \"size\":\"{}x{}\",\
             \"frames\":{},\"sampled\":{},\"worst_frame\":{},\"coverage_rel\":{:.4},\
             \"mae\":{:.2},\"interior_mae\":{:.2},\"edge_mae\":{:.2},\
             \"diff_px_pct\":{:.2},\"bbox\":{},\"bias\":[{:.2},{:.2},{:.2}],\
             \"decisions\":{decisions}}}",
            work.name,
            args.post.name(),
            work.width,
            work.height,
            wgpu.frames,
            rows.len(),
            worst.frame,
            worst.coverage_rel,
            worst.mae,
            worst.interior_mae,
            worst.edge_mae,
            worst.diff_px_pct,
            bbox_text(worst.bbox),
            worst.bias[0],
            worst.bias[1],
            worst.bias[2],
        );

        write_work_json(
            &args.out,
            work,
            args.post.name(),
            wgpu.frames,
            &rows,
            &worst,
            status,
            &decisions,
        )?;
        let reason = if problems.is_empty() {
            String::new()
        } else {
            problems.join("；")
        };
        summary.push((
            work.name.clone(),
            wgpu.frames,
            rows.len(),
            worst.clone(),
            problems.is_empty(),
            reason.clone(),
        ));
        if !problems.is_empty() {
            failures.push(format!("{}: {}", work.name, reason));
        }
    }

    write_summary(&args.out, args.post.name(), &summary)?;
    if !failures.is_empty() {
        return Err(format!(
            "对拍未通过（阈值不得放宽，B13 §6.3）：\n  {}",
            failures.join("\n  ")
        ));
    }
    Ok(())
}

/// 一次后端渲染的采样结果。
struct Pass {
    frames: u32,
    samples: Vec<(u32, Vec<u8>)>,
    decisions: Vec<RoutingDecision>,
}

fn render_pass(
    ctx: &RenderContext,
    source: &str,
    language: ScriptLanguage,
    work: &Work,
    args: &Args,
    backend: LoopBackend,
) -> Result<Pass, String> {
    let mut runtime = build_script_runtime(ctx.clone(), source.to_string(), language)
        .map_err(|e| format!("[{}] 装配失败：{e}", work.name))?;
    let frames = frames_for(&runtime, args.fps);
    let mut backends = StandardBackends::new(ctx);
    let mut target = HeadlessTarget::new();
    let mut samples: Vec<(u32, Vec<u8>)> = Vec::new();
    let sample = args.sample;
    let options = FrameLoopOptions {
        width: work.width,
        height: work.height,
        fps: args.fps,
        backend,
        // `match`：两侧同一份参数（排掉"矢量默认开后处理、wgpu 默认全关"的政策差异）；
        // `backend-default`：生产路径口径（各按自身能力取默认）。
        vector_params: match args.post {
            PostMode::Match => Some(zela_scene_bundle::RenderParams::default()),
            PostMode::BackendDefault => None,
        },
    };
    let run = run_frames(
        ctx,
        &mut runtime,
        &mut backends,
        &mut target,
        options,
        true,
        &mut |index, view| {
            if index % sample == 0 {
                samples.push((index, view.rgba.to_vec()));
            }
            Ok(Duration::ZERO)
        },
    )
    .map_err(|e| format!("[{}] {backend:?} 渲染失败：{e}", work.name))?;
    Ok(Pass {
        frames,
        samples,
        decisions: run.decisions,
    })
}

fn decisions_json(decisions: &[RoutingDecision]) -> String {
    let items: Vec<String> = decisions
        .iter()
        .map(|decision| {
            let rejected: Vec<String> = decision
                .rejected
                .iter()
                .map(|(kind, why)| {
                    format!("{{\"backend\":\"{kind:?}\",\"reason\":\"{}\"}}", escape(why))
                })
                .collect();
            format!(
                "{{\"backend\":\"{:?}\",\"reason\":\"{}\",\"rejected\":[{}]}}",
                decision.backend,
                escape(&decision.reason),
                rejected.join(",")
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

fn bbox_text(bbox: Option<[u32; 4]>) -> String {
    match bbox {
        Some([x0, y0, x1, y1]) => format!("[{x0},{y0},{x1},{y1}]"),
        None => "null".to_string(),
    }
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn write_work_json(
    out: &Path,
    work: &Work,
    post: &str,
    frames: u32,
    rows: &[FrameMetrics],
    worst: &FrameMetrics,
    status: &str,
    decisions: &str,
) -> Result<(), String> {
    let samples: Vec<String> = rows
        .iter()
        .map(|m| {
            format!(
                "{{\"frame\":{},\"coverage_wgpu\":{},\"coverage_vector\":{},\
                 \"coverage_rel\":{:.4},\"mae\":{:.3},\"interior_mae\":{:.3},\
                 \"edge_mae\":{:.3},\"interior_px\":{},\"edge_px\":{},\
                 \"diff_px\":{},\"diff_px_pct\":{:.2},\"bbox\":{},\
                 \"bias\":[{:.3},{:.3},{:.3}]}}",
                m.frame,
                m.coverage_wgpu,
                m.coverage_vector,
                m.coverage_rel,
                m.mae,
                m.interior_mae,
                m.edge_mae,
                m.interior_px,
                m.edge_px,
                m.diff_px,
                m.diff_px_pct,
                bbox_text(m.bbox),
                m.bias[0],
                m.bias[1],
                m.bias[2],
            )
        })
        .collect();
    let json = format!(
        "{{\"work\":\"{}\",\"post\":\"{post}\",\"size\":\"{}x{}\",\"frames\":{},\
         \"status\":\"{status}\",\
         \"criteria\":{{\"coverage_rel_max\":{COVERAGE_REL_MAX},\"mae_max\":{MAE_MAX},\
         \"interior_mae_max\":{INTERIOR_MAE_MAX}}},\
         \"worst_frame\":{},\"decisions\":{decisions},\"samples\":[{}]}}",
        work.name,
        work.width,
        work.height,
        frames,
        worst.frame,
        samples.join(","),
    );
    let path = out.join(format!("{}.json", work.name));
    std::fs::write(&path, json).map_err(|e| format!("写 {} 失败：{e}", path.display()))
}

fn write_summary(
    out: &Path,
    post: &str,
    rows: &[(String, u32, usize, FrameMetrics, bool, String)],
) -> Result<(), String> {
    let mut text = format!(
        "# 矢量 vs wgpu 对拍（B13 Step 1/2）\n\n\
         判据：覆盖率相对差 ≤ 10% / 全图 MAE ≤ 8 / 内部区 MAE ≤ 2（不得放宽）。\n\
         `worst` 取采样帧里全图 MAE 最大的那一帧。post = `{post}`\n\
         （`match` = 两侧同一份 `RenderParams::default()`；`backend-default` = 各按自身能力取默认）。\n\n\
         | work | frames | sampled | worst# | cov_rel | MAE | interior | edge | diff% | bbox | bias RGB | status |\n\
         |---|---|---|---|---|---|---|---|---|---|---|---|\n",
    );
    for (name, frames, sampled, worst, ok, reason) in rows {
        let bbox = worst
            .bbox
            .map(|[x0, y0, x1, y1]| format!("{x0},{y0}..{x1},{y1}"))
            .unwrap_or_else(|| "—".to_string());
        text.push_str(&format!(
            "| {name} | {frames} | {sampled} | {} | {:.3} | {:.2} | {:.2} | {:.2} | {:.2}% | {bbox} | \
             {:.1}/{:.1}/{:.1} | {} |\n",
            worst.frame,
            worst.coverage_rel,
            worst.mae,
            worst.interior_mae,
            worst.edge_mae,
            worst.diff_px_pct,
            worst.bias[0],
            worst.bias[1],
            worst.bias[2],
            if *ok { "ok" } else { "FAIL" },
        ));
        if !ok {
            text.push_str(&format!("\n> {name}: {reason}\n"));
        }
    }
    let path = out.join("summary.md");
    std::fs::write(&path, text).map_err(|e| format!("写 {} 失败：{e}", path.display()))
}
