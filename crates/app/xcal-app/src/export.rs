// export.rs — 序列帧导出（B 线）。
//
// # 流程
//
// ```text
// 每帧：构造 SceneCompositorCallback → render_frame_headless → 抓像素
//       → BGRA/RGBA → PNG → 存盘
// ```
//
// **没有 seek**：Scene 不保证纯（动画/延迟命令都可能依赖真实时序）⇒ 只能边跑边抓。
// 视频编码不在本轮（序列帧交给外部工具合成）。
//
// # 与实时画面的一致性
//
// 导出的像素来自 `render_frame_headless`，它与 egui 宿主路径共用
// `SceneCompositorCallback::prepare_frame` / `paint_frame`；后端装配方式不同
// （`StandardBackends` vs `TypeMap`）。`xcal-render-backends` 的
// `headless_path_matches_egui_path_bit_for_bit` 用例锁定"逐字节一致"，
// 本模块的测试再用导出用的回调复核一次。
//
// # 像素格式与 alpha
//
// `render_frame_headless` 返回**目标纹理格式的字节序**（本项目 headless 默认
// `Bgra8UnormSrgb`）。PNG 只接受 RGBA ⇒ 导出时按 `ctx.target_format()` 做
// **通道交换**；alpha 原样保留（8-bit 直色，非预乘）。

use std::fmt;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use xcal_render_backends::{
    RenderContext, SceneCompositorCallback, ScreenParams, StandardBackends, render_frame_headless,
};
/// 导出规格。
#[derive(Debug, Clone)]
pub struct ExportSpec {
    /// 输出目录（不存在则创建）。
    pub out_dir: PathBuf,
    /// 帧宽（物理像素）。
    pub width: u32,
    /// 帧高（物理像素）。
    pub height: u32,
    /// 帧率（由调用方用来算帧时间；导出本身只按帧索引）。
    pub fps: f64,
    /// 帧数。
    pub frames: u32,
    /// 清屏色。
    pub clear: wgpu::Color,
}

impl ExportSpec {
    /// 默认规格：`out_dir`、640×480、30 fps、60 帧、不透明黑底。
    pub fn new(out_dir: impl Into<PathBuf>) -> Self {
        Self {
            out_dir: out_dir.into(),
            width: 640,
            height: 480,
            fps: 30.0,
            frames: 60,
            clear: wgpu::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            },
        }
    }
}

/// 导出错误（全部明确，不静默跳过）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    /// 文件系统错误。
    Io { path: PathBuf, message: String },
    /// PNG 编码错误。
    Png { path: PathBuf, message: String },
    /// 回读像素数与 `width * height * 4` 不符。
    BadPixelCount { expected: usize, actual: usize },
    /// 目标格式没有已知的 ffmpeg 像素格式（**不静默猜测**）。
    UnsupportedFormat { format: String },
}

impl fmt::Display for ExportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, message } => write!(f, "I/O 错误（{}）：{message}", path.display()),
            Self::Png { path, message } => {
                write!(f, "PNG 编码错误（{}）：{message}", path.display())
            }
            Self::BadPixelCount { expected, actual } => {
                write!(f, "像素数量不符：期望 {expected}，实际 {actual}")
            }
            Self::UnsupportedFormat { format } => {
                write!(f, "目标纹理格式 `{format}` 没有已知的 ffmpeg 像素格式")
            }
        }
    }
}

impl std::error::Error for ExportError {}

/// 导出结果（含逐帧耗时，供"只测不优化"的记账）。
#[derive(Debug, Clone, Default)]
pub struct ExportReport {
    /// 写出的文件（按帧序）。
    pub files: Vec<PathBuf>,
    /// 每帧 `render_frame_headless` 的耗时。
    pub render_times: Vec<Duration>,
    /// 每帧 PNG 编码 + 存盘的耗时。
    pub encode_times: Vec<Duration>,
}

impl ExportReport {
    /// 平均渲染耗时（毫秒）。
    pub fn avg_render_ms(&self) -> f64 {
        avg_ms(&self.render_times)
    }

    /// 平均编码耗时（毫秒）。
    pub fn avg_encode_ms(&self) -> f64 {
        avg_ms(&self.encode_times)
    }
}

fn avg_ms(times: &[Duration]) -> f64 {
    if times.is_empty() {
        return 0.0;
    }
    times.iter().map(|d| d.as_secs_f64()).sum::<f64>() * 1000.0 / times.len() as f64
}

/// **逐帧导出**：`frame(i)` 给出第 `i` 帧的完整渲染请求。
///
/// - 后端**只建一次**（`StandardBackends` 跨帧缓存管线/几何）；
/// - 每帧自建目标纹理（`render_frame_headless` 的既有行为，本轮不优化）；
/// - 文件名 `frame_{i:05}.png`。
pub fn export_sequence<F>(
    ctx: &RenderContext,
    spec: &ExportSpec,
    mut frame: F,
) -> Result<ExportReport, ExportError>
where
    F: FnMut(u32) -> SceneCompositorCallback,
{
    std::fs::create_dir_all(&spec.out_dir).map_err(|e| ExportError::Io {
        path: spec.out_dir.clone(),
        message: e.to_string(),
    })?;

    let mut backends = StandardBackends::new(ctx);
    let screen = ScreenParams::new([spec.width, spec.height], 1.0);
    let expected = spec.width as usize * spec.height as usize * 4;

    let mut report = ExportReport::default();
    for i in 0..spec.frames {
        let callback = frame(i);

        let t0 = Instant::now();
        let pixels = render_frame_headless(ctx, &mut backends, &callback, screen, spec.clear);
        report.render_times.push(t0.elapsed());

        if pixels.len() != expected {
            return Err(ExportError::BadPixelCount {
                expected,
                actual: pixels.len(),
            });
        }

        let rgba = to_rgba(ctx, &pixels);
        let path = spec.out_dir.join(format!("frame_{i:05}.png"));
        let t1 = Instant::now();
        write_png(&path, spec.width, spec.height, &rgba)?;
        report.encode_times.push(t1.elapsed());

        report.files.push(path);
    }

    Ok(report)
}

/// 目标纹理格式 → ffmpeg `-pix_fmt`（**查询**，不得硬编码：格式一变颜色就错且不报错）。
///
/// `Bgra8*` 可直接喂 ffmpeg（零通道交换）；`Rgba8*` 同理。
pub fn target_pix_fmt(ctx: &RenderContext) -> Result<&'static str, ExportError> {
    use wgpu::TextureFormat;

    match ctx.target_format() {
        TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb => Ok("bgra"),
        TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb => Ok("rgba"),
        other => Err(ExportError::UnsupportedFormat {
            format: format!("{other:?}"),
        }),
    }
}

/// 把回读像素转成 PNG 用的 RGBA8（按目标格式做通道交换；alpha 原样）。
pub fn to_rgba(ctx: &RenderContext, pixels: &[u8]) -> Vec<u8> {
    // 唯一实现在回放 crate（浏览器对拍共用同一口径）。
    xcal_playback::to_rgba(ctx, pixels)
}

/// 写一张 RGBA8 PNG。
pub fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), ExportError> {
    let file = std::fs::File::create(path).map_err(|e| ExportError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    let writer = BufWriter::new(file);
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| ExportError::Png {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    writer
        .write_image_data(rgba)
        .map_err(|e| ExportError::Png {
            path: path.to_path_buf(),
            message: e.to_string(),
        })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_notion::{Rect2D, Vec2};
    use xc_object::Camera2D;
    use xcal_render::{
        DrawQueue, DrawQueue3D, PostFilterChain, StrokeSolid, ZIndex, components::Line,
    };
    use xcal_render_backends::Compositor;
    use xcal_scene::domain::{LayerBlend, LayerId, LayerKind};

    const W: u32 = 32;
    const H: u32 = 24;

    fn clear() -> wgpu::Color {
        wgpu::Color {
            r: 0.05,
            g: 0.1,
            b: 0.15,
            a: 1.0,
        }
    }

    fn callback(line_x: f32) -> SceneCompositorCallback {
        let mut q = DrawQueue::new();
        q.line_stroke(
            ZIndex::new(0),
            Line::new((line_x, 3.0), (line_x, H as f32 - 3.0)),
            StrokeSolid::new(3.0, (255, 255, 255, 255)),
        );
        let camera = Camera2D::new(
            Vec2::new(W as f32 / 2.0, H as f32 / 2.0),
            1.0,
            Rect2D::new(Vec2::ZERO, Vec2::new(W as f32, H as f32)),
        );
        SceneCompositorCallback::new(
            Rect2D::new(Vec2::ZERO, Vec2::new(W as f32, H as f32)),
            vec![xcal_render_backends::SceneLayer {
                id: LayerId(0),
                kind: LayerKind::Viewport2D { camera },
                blend: LayerBlend::Alpha,
                opacity: 1.0,
            }],
            &q,
            &DrawQueue3D::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            PostFilterChain::default(),
        )
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xcal-export-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn decode_png(path: &Path) -> (u32, u32, Vec<u8>) {
        let file = std::fs::File::open(path).expect("打开 PNG");
        let decoder = png::Decoder::new(file);
        let mut reader = decoder.read_info().expect("PNG 头");
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("PNG 帧");
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    /// B1 验收：编码出的 PNG 可被标准解码器读回，像素逐位一致。
    #[test]
    fn png_roundtrip_preserves_pixels() {
        let dir = temp_dir("roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("one.png");
        let rgba = vec![255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 10, 20, 30, 40];
        write_png(&path, 2, 2, &rgba).expect("编码");
        let (w, h, decoded) = decode_png(&path);
        assert_eq!((w, h), (2, 2));
        assert_eq!(decoded, rgba, "PNG 往返必须逐位一致（含 alpha）");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// B2 验收：N 帧文件数正确、命名有序、**每帧内容确实在变**。
    #[test]
    fn export_writes_ordered_and_changing_frames() {
        let Some((_gpu, ctx)) = xcal_render_backends::require_gpu() else {
            return;
        };
        let dir = temp_dir("sequence");
        let mut spec = ExportSpec::new(&dir);
        spec.width = W;
        spec.height = H;
        spec.frames = 6;
        spec.fps = 10.0;
        spec.clear = clear();

        let report =
            export_sequence(&ctx, &spec, |i| callback(4.0 + i as f32 * 4.0)).expect("导出成功");

        assert_eq!(report.files.len(), 6);
        for (i, path) in report.files.iter().enumerate() {
            assert!(path.exists(), "第 {i} 帧文件必须存在");
            assert_eq!(
                path.file_name().unwrap().to_string_lossy(),
                format!("frame_{i:05}.png")
            );
        }

        let frames: Vec<Vec<u8>> = report.files.iter().map(|p| decode_png(p).2).collect();
        for i in 1..frames.len() {
            assert_ne!(
                frames[i],
                frames[i - 1],
                "第 {i} 帧与上一帧完全相同 —— 导出没有在推进画面"
            );
        }

        eprintln!(
            "[export] {:.2} ms/帧渲染、{:.2} ms/帧编码（{} 帧，{}×{}）",
            report.avg_render_ms(),
            report.avg_encode_ms(),
            report.files.len(),
            W,
            H
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// B4 验收：导出用的回调在 **headless 与 egui 宿主路径** 上逐字节一致。
    #[test]
    fn export_frame_matches_egui_path_bit_for_bit() {
        let Some((_gpu, ctx)) = xcal_render_backends::require_gpu() else {
            return;
        };
        let device = ctx.device();
        let queue = ctx.queue();
        let cb = callback(12.0);
        let screen = ScreenParams::new([W, H], 1.0);

        // ── B：headless（导出实际走的路径）──
        let mut backends = StandardBackends::new(&ctx);
        let px_headless = render_frame_headless(&ctx, &mut backends, &cb, screen, clear());

        // ── A：egui 路径（与生产宿主同构）──
        let mut resources = egui_wgpu::CallbackResources::new();
        StandardBackends::new(&ctx).install_into(&mut resources);
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("xcal-export-consistency-target"),
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ctx.target_format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("xcal-export-consistency"),
        });
        let egui_screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [W, H],
            pixels_per_point: 1.0,
        };
        let _ = egui_wgpu::CallbackTrait::prepare(
            &cb,
            device,
            queue,
            &egui_screen,
            &mut enc,
            &mut resources,
        );
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("xcal-export-consistency-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear()),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(0.0, 0.0, W as f32, H as f32, 0.0, 1.0);
            pass.set_scissor_rect(0, 0, W, H);
            let compositor = resources.get::<Compositor>().expect("合成器");
            let composed = resources
                .get::<xcal_render_backends::SceneCompositorFrame>()
                .expect("合成帧");
            cb.paint_frame(compositor, composed, &mut pass);
        }
        ctx.submit(enc.finish());
        let px_egui = ctx.read_texture(&target, (W, H));

        let diff = px_headless.iter().zip(&px_egui).position(|(a, b)| a != b);
        assert!(
            diff.is_none(),
            "导出帧与 egui 路径必须逐字节一致（首个不同字节：{diff:?}）"
        );
        eprintln!(
            "[export] 一致性：{} 字节逐位相同（{}×{} RGBA）",
            px_headless.len(),
            W,
            H
        );
    }
}
