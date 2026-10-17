//! 序列帧导出示例（B 线）。
//!
//! ```text
//! cargo run -p xcal-app --example export_frames -- \
//!     --out export/frames --frames 60 --fps 30 --size 640x480
//! ```
//!
//! 场景里的函数**来自 `xc-vm`**（`xcal_scene::builtin_f32`），这是 A 线的
//! 生产消费方；导出的每一帧都经 `render_frame_headless`（与实时画面同一份帧体）。
//!
//! 无 GPU 适配器时明确报错退出（导出无法在无 GPU 环境完成）。

use std::path::PathBuf;

use xcal_app::export::{ExportSpec, export_sequence};
use xcal_render::{DrawQueue, DrawQueue3D, PostFilterChain, StrokeSolid, record_scene};
use xcal_render_backends::{RenderContext, SceneCompositorCallback, SceneLayer};
use xcal_scene::contract::{Scene as _, WorldView};
use xcal_scene::domain::{LayerBlend, LayerId, LayerKind};
use xcal_scene::scene::Scene;
use xc_notion::{Rect2D, Sampler1D, Vec2};
use xc_object::{Camera2D, Circle, FunctionGraph};

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    if std::env::args().any(|a| a == "--help" || a == "-h") {
        println!(
            "用法：cargo run -p xcal-app --example export_frames -- \
             [--out DIR] [--frames N] [--fps F] [--size WxH]"
        );
        return;
    }

    let out = arg("--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("export/frames"));
    let frames: u32 = arg("--frames")
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let fps: f64 = arg("--fps").and_then(|v| v.parse().ok()).unwrap_or(30.0);
    let (width, height) = arg("--size")
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((640u32, 480u32));

    let ctx = match RenderContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("无法创建 headless GPU 上下文：{e}");
            std::process::exit(1);
        }
    };

    // ── 场景：一条函数图像（来自 VM）+ 一个运动的圆 + 固定相机 ──────────
    let mut scene = Scene::new();
    let camera = Camera2D::new(
        Vec2::ZERO,
        60.0,
        Rect2D::new(Vec2::ZERO, Vec2::new(width as f32, height as f32)),
    );

    // **A 线**：函数体由 `xc-vm` 提供（`xcal_scene::builtin_f32`），
    // 不是 `xc-notion` 里那份"不依赖 VM"的静态兜底。
    let func = xcal_scene::builtin_f32("xcal.math.sin").expect("内置函数已注册进 math VM");
    let graph = FunctionGraph::new(func, Sampler1D::new(-3.0f32, 3.0, 0.02));
    let graph_id = graph
        .build(&mut scene, StrokeSolid::new(2.0, (255, 255, 255, 255)))
        .expect("FunctionGraph 的组件均已登记");

    let circle_id = scene
        .spawn_dyn(vec![
            Box::new(Circle::new((0.0, 0.0), 0.8)),
            Box::new(StrokeSolid::new(3.0, (80, 220, 255, 255))),
        ])
        .expect("Circle / StrokeSolid 均已登记");

    let mut spec = ExportSpec::new(&out);
    spec.width = width;
    spec.height = height;
    spec.fps = fps;
    spec.frames = frames;

    println!(
        "导出 {frames} 帧到 {}（{}×{} @ {fps} fps）…",
        out.display(),
        width,
        height
    );

    let report = export_sequence(&ctx, &spec, |i| {
        let t = i as f64 / fps;

        // 每帧推进画面：函数相位 + 圆的轨迹（保证帧内容确实在变）。
        graph.refresh(&mut scene, graph_id, (t * 2.0) as f32);
        let cx = 2.5 * (t as f32 * 1.7).cos();
        let cy = 1.2 * (t as f32 * 2.3).sin();
        scene.set(circle_id, Circle::new((cx, cy), 0.8));

        let mut queue = DrawQueue::new();
        record_scene(&WorldView::new(scene.world()), &camera, &mut queue);

        SceneCompositorCallback::new(
            Rect2D::new(Vec2::ZERO, Vec2::new(width as f32, height as f32)),
            vec![SceneLayer {
                id: LayerId(0),
                kind: LayerKind::Viewport2D { camera },
                blend: LayerBlend::Alpha,
                opacity: 1.0,
            }],
            &queue,
            &DrawQueue3D::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            PostFilterChain::default(),
        )
    })
    .unwrap_or_else(|e| {
        eprintln!("导出失败：{e}");
        std::process::exit(1);
    });

    println!(
        "完成：{} 个文件；渲染 {:.2} ms/帧，编码 {:.2} ms/帧",
        report.files.len(),
        report.avg_render_ms(),
        report.avg_encode_ms()
    );
    if let Some(first) = report.files.first() {
        println!("示例帧：{}", first.display());
    }
}
