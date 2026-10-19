//! `xcal-web` — 浏览器薄壳（**唯一**允许碰 DOM / WebGPU 初始化的地方）。
//!
//! 只做四件事：
//! 1. canvas + WebGPU 初始化（格式取自 `surface.get_capabilities`，不硬编码）；
//! 2. `fetch` `.xsb` → `Bundle::from_bytes` → `xcal-playback::build_bundle_runtime`；
//! 3. rAF 驱动，**固定 dt = 1/30**（绝不用 rAF delta）；
//! 4. 无 `navigator.gpu` ⇒ 显式提示，不白屏。
//!
//! 显示与对拍是两套格式（S6）：
//! - 显示：canvas surface 格式；
//! - 对拍：独立的 `Bgra8UnormSrgb` 离屏纹理 + 异步回读 + `FrameSequenceHash`。
//!
//! URL 参数：
//! - `?xsb=./fourier.xsb`（默认）；
//! - `?compare=1` 开启对拍（结束后在页面与控制台输出序列哈希）。
#![cfg(target_arch = "wasm32")]

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Document, HtmlCanvasElement, Response, UrlSearchParams, Window};

use xcal_playback::{
    build_bundle_runtime, end_frame, set_view_rect, step_frame, to_rgba,
};
use xcal_render_backends::{
    DEFAULT_TARGET_FORMAT, RenderContext, ScreenParams, StandardBackends,
    render_frame_headless_async, render_frame_to_view,
};
use xcal_scene_bundle::Bundle;
use xcal_scene_bundle::frame_hash::FrameSequenceHash;

/// 与 native 基线一致：640×480、30fps、固定 dt。
const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const FPS: f64 = 30.0;

fn js_err(message: impl Into<String>) -> JsValue {
    JsValue::from_str(&message.into())
}

fn fnv(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn set_status(document: &Document, message: &str) {
    if let Some(element) = document.get_element_by_id("xcal-status") {
        element.set_text_content(Some(message));
    }
}

fn show_error(error: &JsValue) {
    let message = error
        .as_string()
        .unwrap_or_else(|| format!("{error:?}"));
    if let Some(document) = web_sys::window().and_then(|window| window.document()) {
        set_status(&document, &format!("错误：{message}"));
    }
    web_sys::console::error_1(&error);
}

/// 入口：由 `index.html` 调用（`wasm.start()`）。
#[wasm_bindgen]
pub async fn start() -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    match run().await {
        Ok(()) => Ok(()),
        Err(error) => {
            show_error(&error);
            Err(error)
        }
    }
}

async fn run() -> Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| js_err("没有 window"))?;
    let document = window
        .document()
        .ok_or_else(|| js_err("没有 document"))?;

    // ── 4. 能力检测：无 WebGPU ⇒ 显式提示 ──────────────────────────────
    let gpu = js_sys::Reflect::get(&window.navigator(), &JsValue::from_str("gpu"))
        .unwrap_or(JsValue::UNDEFINED);
    if gpu.is_undefined() || gpu.is_null() {
        return Err(js_err(
            "当前浏览器不支持 WebGPU（navigator.gpu 缺失）。请使用 Chrome/Edge 113+ 并启用硬件加速。",
        ));
    }

    let params = UrlSearchParams::new_with_str(&window.location().search().unwrap_or_default())?;
    let xsb_url = params
        .get("xsb")
        .unwrap_or_else(|| "./fourier.xsb".to_string());
    let compare = params.get("compare").as_deref() == Some("1");

    let canvas: HtmlCanvasElement = document
        .get_element_by_id("xcal-canvas")
        .ok_or_else(|| js_err("缺少 <canvas id=\"xcal-canvas\">"))?
        .dyn_into()
        .map_err(|_| js_err("xcal-canvas 不是 canvas"))?;
    canvas.set_width(WIDTH);
    canvas.set_height(HEIGHT);
    set_status(&document, "WebGPU 初始化…");

    // ── 1. canvas + WebGPU（格式取自 capabilities，不硬编码）────────────
    let instance = wgpu::Instance::default();
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|e| js_err(format!("create_surface 失败：{e}")))?;
    let (adapter, device, queue) = RenderContext::request_adapter_and_device(
        &instance,
        wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        },
    )
    .await
    .map_err(|e| js_err(e.to_string()))?;

    let caps = surface.get_capabilities(&adapter);
    let format = *caps
        .formats
        .first()
        .ok_or_else(|| js_err("surface 没有可用纹理格式"))?;
    let display_ctx = RenderContext::new(device, queue, format);
    surface.configure(
        &display_ctx.device,
        &wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            color_space: wgpu::SurfaceColorSpace::Auto,
            width: WIDTH,
            height: HEIGHT,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode: caps
                .alpha_modes
                .first()
                .copied()
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: Vec::new(),
        },
    );

    // ── 2. fetch .xsb → 还原 → Runtime ─────────────────────────────────
    set_status(&document, &format!("加载 {xsb_url} …"));
    let bytes = fetch_bytes(&xsb_url).await?;
    let bundle = Bundle::from_bytes(&bytes).map_err(|e| js_err(e.to_string()))?;
    let runtime = build_bundle_runtime(display_ctx.clone(), &bundle)
        .map_err(|e| js_err(e.to_string()))?;
    set_view_rect(&runtime, WIDTH as f32, HEIGHT as f32);
    set_status(
        &document,
        &format!(
            "已加载：{} 个对象 / {:.3}s；{}",
            bundle.meta().object_count,
            bundle.meta().duration,
            if compare { "对拍模式" } else { "播放" }
        ),
    );

    let mut display_backends = StandardBackends::new(&display_ctx);

    // ── S6：对拍用独立上下文（离屏 Srgb）与独立后端 ─────────────────────
    let compare_ctx = RenderContext::new(
        display_ctx.device.clone(),
        display_ctx.queue.clone(),
        DEFAULT_TARGET_FORMAT,
    );
    let mut compare_backends = StandardBackends::new(&compare_ctx);
    let mut hasher = FrameSequenceHash::new(WIDTH, HEIGHT);
    // 结构层：每帧计数 + 拓扑序（不含浮点）。compare 模式结束时输出聚合哈希，
    // 与 native `fingerprint manifest` 的 `structure-summary` 直接比较。
    let mut structure_lines: Vec<String> = Vec::new();

    let frames = ((runtime.duration * FPS).ceil() as u32).max(1);
    let screen = ScreenParams::new([WIDTH, HEIGHT], 1.0);
    let clear = wgpu::Color {
        r: 0.0,
        g: 0.0,
        b: 0.0,
        a: 1.0,
    };
    let dt = 1.0 / FPS as f32;

    // ── 3. rAF 驱动，固定 dt ───────────────────────────────────────────
    for index in 0..frames {
        next_animation_frame(&window).await?;

        let output = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => texture,
            other => {
                return Err(js_err(format!(
                    "get_current_texture 未取得可用帧：{other:?}"
                )));
            }
        };
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // 与 native 导出循环逐字一致：Advance → 构造回调 → 渲染 → end_frame。
        let Some(callback) = step_frame(&runtime, dt) else {
            end_frame(&runtime);
            display_ctx.queue.present(output);
            continue;
        };

        render_frame_to_view(
            &display_ctx,
            &mut display_backends,
            &callback,
            screen,
            clear,
            &view,
        );
        display_ctx.queue.present(output);

        if compare {
            let objects = runtime
                .driver
                .with_scheduler(|scheduler| scheduler.world().len());
            structure_lines.push(format!(
                "frame={index} objects={objects} {}",
                callback.debug_structure().replace(';', " ")
            ));
            let pixels = render_frame_headless_async(
                &compare_ctx,
                &mut compare_backends,
                &callback,
                screen,
                clear,
            )
            .await;
            hasher.push_frame(&to_rgba(&compare_ctx, &pixels));
        }

        end_frame(&runtime);

        if index % 30 == 0 {
            set_status(&document, &format!("帧 {}/{}", index + 1, frames));
        }
    }

    if compare {
        let sequence = hasher.hex();
        set_status(
            &document,
            &format!("对拍完成：{sequence}（{} 帧）", hasher.frames()),
        );
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "[frame-hash] sequence = {sequence}（{} 帧，{WIDTH}×{HEIGHT}，RGBA8）",
            hasher.frames()
        )));
        let structure = structure_lines.join("\n");
        web_sys::console::log_1(&JsValue::from_str(&format!(
            "[structure] frames={} hash={:016x}",
            structure_lines.len(),
            fnv(&structure)
        )));
    } else {
        set_status(&document, &format!("播放完成：{frames} 帧"));
    }
    Ok(())
}

async fn fetch_bytes(url: &str) -> Result<Vec<u8>, JsValue> {
    let window = web_sys::window().ok_or_else(|| js_err("没有 window"))?;
    let response_value = JsFuture::from(window.fetch_with_str(url)).await?;
    let response: Response = response_value
        .dyn_into()
        .map_err(|_| js_err("fetch 返回值不是 Response"))?;
    if !response.ok() {
        return Err(js_err(format!(
            "fetch {url} 失败：HTTP {}",
            response.status()
        )));
    }
    let buffer = JsFuture::from(response.array_buffer()?).await?;
    Ok(js_sys::Uint8Array::new(&buffer).to_vec())
}

/// 等下一帧 rAF。
async fn next_animation_frame(window: &Window) -> Result<(), JsValue> {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let callback = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });
        let _ = window.request_animation_frame(callback.unchecked_ref());
    });
    JsFuture::from(promise).await.map(|_| ())
}
