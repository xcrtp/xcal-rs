// 对拍工具（跨运行时判据的实现）。
//
// 用法：
//   1) 单帧数值指纹（CPU 侧）：
//      cargo run -p xcal-playback --example fingerprint -- <bundle.xsb> <frame> [fps]
//   2) 结构清单（每帧计数 + 拓扑哈希，**不含浮点**）：
//      cargo run -p xcal-playback --example fingerprint -- manifest <bundle.xsb> <out.txt> [fps]
//      stdout 另打印 `structure-summary frames=N hash=…`（可跨端比较的聚合值）。
//   3) 帧目录对拍（结构硬失败 + 数值容差）：
//      cargo run -p xcal-playback --example fingerprint -- compare <ref_dir> <cand_dir> \
//          [--max-mad 1.0] [--max-divergent-ratio 0.01]
//
// `compare` 的目录约定：
//   - `*.png`：逐帧 RGBA8（按文件名排序）；
//   - `structure.txt`（**必需**）：`manifest` 的输出；缺失即结构层失败。
//
// 判定分三层：
//   结构（帧数 / 尺寸 / 结构清单）  ⇒ 不一致立即失败（逻辑错误，不是舍入）；
//   数值（单帧单通道平均绝对差）    ⇒ 阈值默认 1.0（一个 8bit 级）；
//   分歧帧比例                      ⇒ 阈值默认 0.01（实测 1/216，见基线文档）。

use std::env;
use std::fs;
use std::fs::File;
use std::io::BufWriter;
use std::io::Write;
use std::path::{Path, PathBuf};

use xcal_playback::{build_bundle_runtime, end_frame, set_view_rect, step_frame};
use xcal_render_backends::RenderContext;
use xcal_scene_bundle::Bundle;

const DEFAULT_FPS: f64 = 30.0;
const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;

fn main() {
    if let Err(error) = run() {
        eprintln!("fingerprint: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("manifest") => {
            let bundle = args.get(1).ok_or("usage: manifest <bundle.xsb> <out.txt> [fps]")?;
            let out = args.get(2).ok_or("usage: manifest <bundle.xsb> <out.txt> [fps]")?;
            let fps = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_FPS);
            manifest(bundle, Path::new(out), fps)
        }
        Some("compare") => {
            let reference = args.get(1).ok_or("usage: compare <ref_dir> <cand_dir> [options]")?;
            let candidate = args.get(2).ok_or("usage: compare <ref_dir> <cand_dir> [options]")?;
            let mut max_mad = 1.0f64;
            let mut max_ratio = 0.01f64;
            let mut index = 3;
            while index < args.len() {
                match args[index].as_str() {
                    "--max-mad" => {
                        index += 1;
                        max_mad = args
                            .get(index)
                            .ok_or("--max-mad 需要一个数值")?
                            .parse()
                            .map_err(|_| "--max-mad 不是数值")?;
                    }
                    "--max-divergent-ratio" => {
                        index += 1;
                        max_ratio = args
                            .get(index)
                            .ok_or("--max-divergent-ratio 需要一个数值")?
                            .parse()
                            .map_err(|_| "--max-divergent-ratio 不是数值")?;
                    }
                    other => return Err(format!("未知参数：{other}")),
                }
                index += 1;
            }
            compare(Path::new(reference), Path::new(candidate), max_mad, max_ratio)
        }
        _ => {
            // 兼容原用法：<bundle.xsb> <frame> [fps]
            let bundle = args.first().ok_or(
                "usage: fingerprint <bundle.xsb> <frame> [fps]\n       fingerprint manifest <bundle> <out> [fps]\n       fingerprint compare <ref_dir> <cand_dir> [options]",
            )?;
            let target: u32 = args
                .get(1)
                .and_then(|v| v.parse().ok())
                .ok_or("缺少 frame 序号")?;
            let fps = args.get(2).and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_FPS);
            single_frame(bundle, target, fps)
        }
    }
}

fn fnv(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn build_runtime(bundle_path: &str) -> Result<xcal_playback::Runtime, String> {
    let ctx = RenderContext::headless().map_err(|e| e.to_string())?;
    let bytes = fs::read(bundle_path).map_err(|e| format!("读取 {bundle_path} 失败：{e}"))?;
    let bundle = Bundle::from_bytes(&bytes).map_err(|e| e.to_string())?;
    let runtime = build_bundle_runtime(ctx, &bundle).map_err(|e| e.to_string())?;
    set_view_rect(&runtime, WIDTH as f32, HEIGHT as f32);
    Ok(runtime)
}

fn single_frame(bundle_path: &str, target: u32, fps: f64) -> Result<(), String> {
    let runtime = build_runtime(bundle_path)?;
    let dt = 1.0 / fps as f32;
    for index in 0..=target {
        let callback = step_frame(&runtime, dt).ok_or("本帧没有可渲染回调")?;
        if index == target {
            println!("frame={index} fingerprint={}", callback.debug_fingerprint());
            println!("frame={index} structure={}", callback.debug_structure());
        }
        end_frame(&runtime);
    }
    Ok(())
}

fn manifest(bundle_path: &str, out: &Path, fps: f64) -> Result<(), String> {
    let runtime = build_runtime(bundle_path)?;
    let frames = ((runtime.duration * fps).ceil() as u32).max(1);
    let dt = 1.0 / fps as f32;

    let mut lines = Vec::with_capacity(frames as usize);
    for index in 0..frames {
        let callback = step_frame(&runtime, dt).ok_or("本帧没有可渲染回调")?;
        let objects = runtime
            .driver
            .with_scheduler(|scheduler| scheduler.world().len());
        let structure = callback.debug_structure().replace(';', " ");
        lines.push(format!("frame={index} objects={objects} {structure}"));
        end_frame(&runtime);
    }

    let text = lines.join("\n") + "\n";
    let mut file = BufWriter::new(
        File::create(out).map_err(|e| format!("创建 {} 失败：{e}", out.display()))?,
    );
    file.write_all(text.as_bytes())
        .map_err(|e| format!("写 {} 失败：{e}", out.display()))?;
    println!("structure-summary frames={frames} hash={:016x}", fnv(text.trim_end()));
    Ok(())
}

fn png_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| format!("读取目录 {} 失败：{e}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("png"))
        .collect();
    files.sort();
    Ok(files)
}

fn decode_png(path: &Path) -> Result<(u32, u32, Vec<u8>), String> {
    let file = File::open(path).map_err(|e| format!("打开 {} 失败：{e}", path.display()))?;
    let decoder = png::Decoder::new(file);
    let mut reader = decoder
        .read_info()
        .map_err(|e| format!("解析 {} 失败：{e}", path.display()))?;
    let mut buffer = vec![0; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|e| format!("解码 {} 失败：{e}", path.display()))?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!(
            "{} 不是 RGBA8（{:?}/{:?}）",
            path.display(),
            info.color_type,
            info.bit_depth
        ));
    }
    buffer.truncate(info.buffer_size());
    Ok((info.width, info.height, buffer))
}

fn compare(
    reference: &Path,
    candidate: &Path,
    max_mad: f64,
    max_divergent_ratio: f64,
) -> Result<(), String> {
    let reference_frames = png_files(reference)?;
    let candidate_frames = png_files(candidate)?;

    // ── 第一层：结构（硬失败）────────────────────────────────────────
    if reference_frames.len() != candidate_frames.len() {
        return Err(format!(
            "结构失败：帧数不同（ref={} cand={}）",
            reference_frames.len(),
            candidate_frames.len()
        ));
    }
    // 结构层不允许降级：没有清单就无法证明结构一致，直接失败。
    let reference_manifest = reference.join("structure.txt");
    let candidate_manifest = candidate.join("structure.txt");
    if !reference_manifest.is_file() || !candidate_manifest.is_file() {
        return Err(format!(
            "结构失败：缺少 structure.txt（ref={} cand={}）—— 先对两侧运行 `fingerprint manifest`",
            reference_manifest.display(),
            candidate_manifest.display()
        ));
    }
    let a = fs::read_to_string(&reference_manifest)
        .map_err(|e| format!("读取 {} 失败：{e}", reference_manifest.display()))?;
    let b = fs::read_to_string(&candidate_manifest)
        .map_err(|e| format!("读取 {} 失败：{e}", candidate_manifest.display()))?;
    if a != b {
        let first = a
            .lines()
            .zip(b.lines())
            .position(|(x, y)| x != y)
            .unwrap_or_else(|| a.lines().count().min(b.lines().count()));
        return Err(format!(
            "结构失败：structure.txt 第 {} 行不同（ref={:?} cand={:?}）—— 这是逻辑错误，不是舍入",
            first + 1,
            a.lines().nth(first),
            b.lines().nth(first)
        ));
    }
    println!(
        "structure: OK（{} 帧，manifest 完全一致，hash={:016x}）",
        reference_frames.len(),
        fnv(a.trim_end())
    );

    // ── 第二层：数值容差 ─────────────────────────────────────────────
    let mut divergent = 0usize;
    let mut out_of_tolerance = 0usize;
    let mut max_seen = 0.0f64;
    let mut max_at = 0usize;

    for (index, (ref_path, cand_path)) in reference_frames
        .iter()
        .zip(candidate_frames.iter())
        .enumerate()
    {
        let (ref_w, ref_h, ref_px) = decode_png(ref_path)?;
        let (cand_w, cand_h, cand_px) = decode_png(cand_path)?;
        if (ref_w, ref_h) != (cand_w, cand_h) {
            return Err(format!(
                "结构失败：第 {index} 帧尺寸不同（ref={ref_w}×{ref_h} cand={cand_w}×{cand_h}）"
            ));
        }
        if ref_w != WIDTH || ref_h != HEIGHT {
            return Err(format!(
                "结构失败：第 {index} 帧尺寸 {ref_w}×{ref_h} ≠ 基线 {WIDTH}×{HEIGHT}"
            ));
        }
        if ref_px.len() != cand_px.len() {
            return Err(format!("结构失败：第 {index} 帧像素字节数不同"));
        }

        let mut sums = [0u64; 4];
        let mut diff_pixels = 0usize;
        for (a, b) in ref_px.chunks_exact(4).zip(cand_px.chunks_exact(4)) {
            let mut any = false;
            for channel in 0..4 {
                let delta = i32::from(a[channel]) - i32::from(b[channel]);
                if delta != 0 {
                    any = true;
                    sums[channel] += delta.unsigned_abs() as u64;
                }
            }
            if any {
                diff_pixels += 1;
            }
        }
        let pixels = f64::from(ref_w) * f64::from(ref_h);
        let mad = [
            sums[0] as f64 / pixels,
            sums[1] as f64 / pixels,
            sums[2] as f64 / pixels,
            sums[3] as f64 / pixels,
        ];
        let mad_rgb = (mad[0] + mad[1] + mad[2]) / 3.0;
        let is_divergent = sums.iter().any(|&sum| sum > 0);
        let over = mad_rgb > max_mad;
        if is_divergent {
            divergent += 1;
        }
        if over {
            out_of_tolerance += 1;
        }
        if mad_rgb > max_seen {
            max_seen = mad_rgb;
            max_at = index;
        }
        if is_divergent || over {
            println!(
                "frame={index} mad=({:.6},{:.6},{:.6},{:.6}) mad_rgb={:.6} diff_px={} status={}",
                mad[0],
                mad[1],
                mad[2],
                mad[3],
                mad_rgb,
                diff_pixels,
                if over { "OUT_OF_TOLERANCE" } else { "OK" }
            );
        }
    }

    let ratio = divergent as f64 / reference_frames.len() as f64;
    println!(
        "summary: frames={} structure=OK divergent={} ({:.2}%) out_of_tolerance={} max_mad_rgb={:.6}@frame{} ratios(limit={:.2}%) verdict={}",
        reference_frames.len(),
        divergent,
        ratio * 100.0,
        out_of_tolerance,
        max_seen,
        max_at,
        max_divergent_ratio * 100.0,
        if out_of_tolerance == 0 && ratio <= max_divergent_ratio {
            "OK"
        } else {
            "FAIL"
        }
    );

    if out_of_tolerance > 0 {
        return Err(format!(
            "数值容差失败：{out_of_tolerance} 帧 mad_rgb > {max_mad}"
        ));
    }
    if ratio > max_divergent_ratio {
        return Err(format!(
            "分歧帧比例失败：{:.2}% > {:.2}%",
            ratio * 100.0,
            max_divergent_ratio * 100.0
        ));
    }
    Ok(())
}
