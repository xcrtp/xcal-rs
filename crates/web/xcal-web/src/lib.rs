//! `xcal-web` — 浏览器薄壳（**唯一**允许碰 DOM / WebGPU 初始化的地方）。
//!
//! 只做四件事：
//! 1. canvas + WebGPU 初始化（格式取自 `surface.get_capabilities`，不硬编码）；
//! 2. `fetch` `.xsb` → `Bundle::from_bytes` → `xcal-playback::build_bundle_runtime`；
//! 3. rAF 驱动，**固定 dt = 1/30**（绝不用 rAF delta）；
//! 4. 无 `navigator.gpu` ⇒ 显式提示，不白屏；
//! 5. 调速：只改变每个 rAF 推进的仿真帧数，**dt 恒为 1/30**。
//!
//! 显示与对拍是两套格式（S6）：
//! - 显示：canvas surface 格式；
//! - 对拍：独立的 `Bgra8UnormSrgb` 离屏纹理 + 异步回读 + `FrameSequenceHash`。
//!
//! URL 参数：
//! - `?xsb=./fourier.xsb`（默认）；
//! - `?compare=1` 开启对拍（结束后在页面与控制台输出序列哈希）。
//!
//! 两个入口：
//! - [`start`]：按 URL 参数 fetch `.xsb`；
//! - [`start_with_bytes`]：上传预览（只播放，不 compare；32 MB 上限）。
//!   两者共用 [`run_with`]，忙标志保证同一时刻只有一个有限帧循环。
#![cfg(target_arch = "wasm32")]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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

/// 单个 XSB 的体积上限（32 MB）：**解析之前**校验，防止坏长度字段诱导巨量分配。
const MAX_BUNDLE_BYTES: usize = 32 * 1024 * 1024;

/// 忙标志：`run()` 是**有限**帧循环（`for index in 0..frames`），不并发；
/// 播放中拒绝第二次启动即可，不需要 generation 机制。
static PLAYING: AtomicBool = AtomicBool::new(false);

/// 播放速度（0.1×–8×）。帧循环每个 rAF 读取一次，因此播放中可即时调速。
/// compare 模式强制 1×（见 [`playback_speed`]），不参与调速。
static SPEED_BITS: AtomicU32 = AtomicU32::new(1.0f32.to_bits());

const MIN_SPEED: f32 = 0.1;
const MAX_SPEED: f32 = 8.0;

fn playback_speed(compare: bool) -> f32 {
    if compare {
        return 1.0;
    }
    f32::from_bits(SPEED_BITS.load(Ordering::Relaxed)).clamp(MIN_SPEED, MAX_SPEED)
}

struct PlayGuard;

impl PlayGuard {
    fn acquire() -> Result<Self, JsValue> {
        if PLAYING.swap(true, Ordering::SeqCst) {
            return Err(js_err("正在播放，请等待完成"));
        }
        Ok(Self)
    }
}

impl Drop for PlayGuard {
    fn drop(&mut self) {
        PLAYING.store(false, Ordering::SeqCst);
    }
}

/// 页面显示的短 hash：`991068e3…1cc4`（完整值留在控制台）。
fn short_hash(hex: &str) -> String {
    if hex.len() <= 16 {
        return hex.to_string();
    }
    format!("{}…{}", &hex[..8], &hex[hex.len() - 4..])
}

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
    let result = run().await;
    if let Err(error) = &result {
        show_error(error);
    }
    result
}

/// 上传预览入口：`index.html` 把 `File` 读成 `Uint8Array` 后传入。
///
/// **只播放，不 compare**：compare 需要 native 对侧，对上传文件无意义。
#[wasm_bindgen]
pub async fn start_with_bytes(bytes: Vec<u8>) -> Result<(), JsValue> {
    console_error_panic_hook::set_once();
    let result = async {
        let (window, document) = prepare()?;
        run_with(window, document, bytes, false, "加载上传文件 …").await
    }
    .await;
    if let Err(error) = &result {
        show_error(error);
    }
    result
}

/// 设置播放速度（0.1×–8×）。播放中调用即时生效（帧循环每个 rAF 读取一次）。
///
/// **不改变固定 dt**：只改变每个 rAF 推进的仿真帧数（`>1×` 快进、`<1×` 慢放），
/// compare 模式（`?compare=1`）强制 1×，不受此设置影响。
#[wasm_bindgen]
pub fn set_speed(speed: f32) {
    let speed = if speed.is_finite() {
        speed.clamp(MIN_SPEED, MAX_SPEED)
    } else {
        1.0
    };
    SPEED_BITS.store(speed.to_bits(), Ordering::Relaxed);
}

/// window / document / WebGPU 能力检测（两条入口共用）。
fn prepare() -> Result<(Window, Document), JsValue> {
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
    Ok((window, document))
}

/// fetch 入口：URL 参数 → 字节 → 公共播放实现。
async fn run() -> Result<(), JsValue> {
    let (window, document) = prepare()?;

    let params = UrlSearchParams::new_with_str(&window.location().search().unwrap_or_default())?;
    let xsb_url = params
        .get("xsb")
        .unwrap_or_else(|| "./fourier.xsb".to_string());
    let compare = params.get("compare").as_deref() == Some("1");

    let loading = format!("加载 {xsb_url} …");
    set_status(&document, &loading);
    let bytes = fetch_bytes(&xsb_url).await?;
    run_with(window, document, bytes, compare, &loading).await
}

/// **唯一的播放实现**：fetch 路径与上传路径共用。
async fn run_with(
    window: Window,
    document: Document,
    bytes: Vec<u8>,
    compare: bool,
    loading: &str,
) -> Result<(), JsValue> {
    let _playing = PlayGuard::acquire()?;

    // 体积上限必须在解析之前校验（坏长度字段可能诱导巨量分配）。
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(js_err(format!(
            "XSB 文件过大：{} B（上限 {MAX_BUNDLE_BYTES} B）",
            bytes.len()
        )));
    }

    set_status(&document, loading);

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

    // ── 2. 字节 → 还原 → Runtime（fetch 与上传共用同一条路）────────────
    let bundle = Bundle::from_bytes(&bytes).map_err(|e| js_err(e.to_string()))?;
    let payload = bundle.payload_hash_hex();
    let payload_short = short_hash(&payload);
    let runtime = build_bundle_runtime(display_ctx.clone(), &bundle)
        .map_err(|e| js_err(e.to_string()))?;
    set_view_rect(&runtime, WIDTH as f32, HEIGHT as f32);
    set_status(
        &document,
        &format!(
            "已加载：{} 个对象 / {:.3}s / payload {} / {:.2}×；{}",
            bundle.meta().object_count,
            bundle.meta().duration,
            payload_short,
            playback_speed(compare),
            if compare { "对拍模式" } else { "播放" }
        ),
    );
    web_sys::console::log_1(&JsValue::from_str(&format!(
        "[payload] hash = {payload}（{} 字节）",
        bundle.payload().len()
    )));

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

    // ── 3. rAF 驱动，固定 dt；速度只改变每个 rAF 推进的仿真帧数 ─────────
    //
    // - 1×：每个 rAF 推进并渲染 1 帧；
    // - >1×：每个 rAF 连续推进多帧，只渲染最后一帧（快进）；
    // - <1×：累积到 1 才推进一帧（慢放，画面保持）。
    // compare 模式强制 1×（`playback_speed`），帧序列与 native 基线逐位一致。
    let mut index: u32 = 0;
    let mut credit: f64 = 0.0;
    let mut next_status: u32 = 0;

    while index < frames {
        next_animation_frame(&window).await?;

        credit += f64::from(playback_speed(compare));
        let mut steps = credit.floor() as u32;
        if steps == 0 {
            continue;
        }
        credit -= f64::from(steps);
        steps = steps.min(frames - index);

        // 前面的步骤只推进（不渲染、不取 surface 纹理）。
        for _ in 1..steps {
            let _ = step_frame(&runtime, dt);
            end_frame(&runtime);
            index += 1;
        }

        // ── 渲染最后一步 ──
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
            index += 1;
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
        index += 1;

        if index >= next_status {
            set_status(
                &document,
                &format!("帧 {index}/{frames} · {:.2}×", playback_speed(compare)),
            );
            next_status = index.saturating_add(30);
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
        set_status(
            &document,
            &format!("播放完成：{frames} 帧 · payload {payload_short}"),
        );
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
