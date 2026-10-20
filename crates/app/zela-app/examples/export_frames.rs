//! 序列帧导出示例（与 GUI / `zela-render` 共用 **Driver pod** 推进）。
//!
//! ```text
//! cargo run -p zela-app --example export_frames -- \
//!     [--script crates/app/zela-app/scripts/storyboard.lua] \
//!     [--out export/frames] [--frames N] [--fps F] [--size WxH]
//! ```
//!
//! # 为什么不再手写帧循环
//!
//! 旧版本每帧手动 `graph.refresh(...)` / `scene.set(...)`：它能导出是因为那个场景的
//! 动画是手写的。脚本产出的动画全在 `SceneRecipe` 里，**不走调度器就一动不动**。
//! 现在每帧只发一条 `Advance(dt)` 命令，渲染帧体由 `zela_render::frame` +
//! `zela_render_backends::frame` 构造 —— 与 GUI 是同一份帧体。
//!
//! 无 GPU 适配器时明确报错退出。

use std::path::PathBuf;

use ze_notion::{Rect2D, Vec2};
use zela_app::export::{ExportSpec, export_sequence};
use zela_app::{build_script_runtime, language_from_path};
use zela_render::{DrawQueue, DrawQueue3D, LightQueue, PostFilterChain, ViewRect};
use zela_render_backends::{RenderContext, SceneCompositorCallback};
use zela_scene::DriverCommand;
use zela_scene::contract::WorldView;

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
            "用法：cargo run -p zela-app --example export_frames -- \
             [--script PATH] [--out DIR] [--frames N] [--fps F] [--size WxH]"
        );
        return;
    }

    let script = PathBuf::from(
        arg("--script").unwrap_or_else(|| "crates/app/zela-app/scripts/storyboard.lua".to_string()),
    );
    let out = arg("--out")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("export/frames"));
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

    let source = match std::fs::read_to_string(&script) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("读取脚本 {} 失败：{e}", script.display());
            std::process::exit(1);
        }
    };
    let language = match language_from_path(&script) {
        Ok(language) => language,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    let runtime = match build_script_runtime(ctx.clone(), source, language) {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("装配失败：{e}");
            std::process::exit(1);
        }
    };

    runtime.driver.with_scheduler_mut(|scheduler| {
        scheduler.resources_mut().insert(ViewRect {
            rect: Rect2D::new(Vec2::ZERO, Vec2::new(width as f32, height as f32)),
        });
    });

    let frames: u32 = arg("--frames")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| (runtime.duration * fps).ceil().max(1.0) as u32);
    let dt = 1.0 / fps as f32;

    let mut spec = ExportSpec::new(&out);
    spec.width = width;
    spec.height = height;
    spec.fps = fps;
    spec.frames = frames;

    println!(
        "导出 {frames} 帧到 {}（{}×{} @ {fps} fps，时长 {:.3}s）…",
        out.display(),
        width,
        height,
        runtime.duration
    );

    let report = export_sequence(&ctx, &spec, |_i| {
        // 与 GUI 同一条命令：固定 dt。
        runtime.driver.submit(DriverCommand::Advance { dt }).wait();

        let callback = runtime
            .driver
            .with_scheduler(|scheduler| {
                let options = zela_render::FrameOptions::default();
                let view = zela_render::frame::build_scene_view(
                    &WorldView::new(scheduler.world()),
                    scheduler.resources(),
                    options,
                );
                zela_render::frame::build_frame_inputs(&view, scheduler.resources(), options)
                    .and_then(zela_render_backends::frame::build_scene_callback)
            })
            .unwrap_or_else(|| {
                SceneCompositorCallback::new(
                    Rect2D::new(Vec2::ZERO, Vec2::new(width as f32, height as f32)),
                    Vec::new(),
                    &DrawQueue::new(),
                    &DrawQueue3D::new(),
                    &LightQueue::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    PostFilterChain::default(),
                )
            });

        // 帧末清理（与宿主 `end_frame` 同口径）。
        runtime.driver.with_scheduler_mut(|scheduler| {
            zela_render::frame::end_frame(scheduler.resources_mut());
        });
        callback
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
}
