//! `zela-web` — 浏览器薄壳（**唯一**允许碰 DOM / WebGPU 初始化的地方）。
//!
//! 只做四件事：
//! 1. canvas + WebGPU 初始化（格式取自 `surface.get_capabilities`，不硬编码）；
//! 2. `fetch` `.zsb` → `Bundle::from_bytes` → `zela-playback::build_bundle_runtime`；
//! 3. rAF 驱动，**固定 dt = 1/30**（绝不用 rAF delta）；
//! 4. 无 `navigator.gpu` ⇒ 显式提示，不白屏；
//! 5. 调速：只改变每个 rAF 推进的仿真帧数，**dt 恒为 1/30**。
//!
//! 显示与对拍是两套格式（S6）：
//! - 显示：canvas surface 格式；
//! - 对拍：独立的 `Bgra8UnormSrgb` 离屏纹理 + 异步回读 + `FrameSequenceHash`。
//!
//! URL 参数：
//! - `?zsb=./fourier.zsb`（默认）；
//! - `?compare=1` 开启对拍（结束后在页面与控制台输出序列哈希）。
//!
//! 两个入口：
//! - [`start`]：按 URL 参数 fetch `.zsb`；
//! - [`start_with_bytes`]：上传预览（只播放，不 compare；32 MB 上限）。
//!   两者共用 [`run_with`]，忙标志保证同一时刻只有一个有限帧循环。
#![cfg(target_arch = "wasm32")]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::Mutex;

use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Document, HtmlCanvasElement, Response, UrlSearchParams, Window};

use zela_playback::{
    build_bundle_runtime, end_frame, set_view_rect, step_frame, to_rgba,
};
use zela_playback::vector_frame::VectorFrame;
use zela_render_backends::{
    AdapterFacts, BackendKind, BackendRegistry, DEFAULT_TARGET_FORMAT, RenderContext,
    RoutingDecision, ScreenParams, StandardBackends, render_frame_headless_async,
    render_frame_to_view, routing::{LoopBackend, route},
};
use zela_scene_bundle::Bundle;
use zela_scene_bundle::frame_hash::FrameSequenceHash;

/// 与 native 基线一致：640×480、30fps、固定 dt。
const WIDTH: u32 = 640;
const HEIGHT: u32 = 480;
const FPS: f64 = 30.0;

/// 单个 ZSB 的体积上限（32 MB）：**解析之前**校验，防止坏长度字段诱导巨量分配。
const MAX_BUNDLE_BYTES: usize = 32 * 1024 * 1024;

/// 播放代次（B11 Step 5）：每次 start / 切后端 +1。
///
/// 旧循环在下一个 rAF 发现代次变了就**安静退出**（不再渲染），因此切后端 =
/// 「重播」而不是整页重载；同一时刻最多一条循环在画。
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// 最近一次加载的产物：切后端直接重播它，不重新 fetch。
static LAST_BUNDLE: Mutex<Option<Vec<u8>>> = Mutex::new(None);

/// 后端偏好：0 = auto（按能力），1 = wgpu，2 = vector。
static BACKEND_MODE: AtomicU8 = AtomicU8::new(0);

fn backend_preference() -> LoopBackend {
    match BACKEND_MODE.load(Ordering::SeqCst) {
        1 => LoopBackend::Wgpu,
        2 => LoopBackend::Vector,
        _ => LoopBackend::Auto,
    }
}

/// 页面工具栏切换后端（B11 §7.3）：**重播**，不整页重载、不重新申请 device。
#[wasm_bindgen]
pub fn set_backend(kind: &str) -> Result<(), JsValue> {
    let mode = match kind {
        "auto" => 0u8,
        "wgpu" => 1,
        "vector" => 2,
        other => {
            return Err(js_err(format!(
                "未知后端：{other}（只接受 auto | wgpu | vector）"
            )));
        }
    };
    BACKEND_MODE.store(mode, Ordering::SeqCst);
    let bytes = LAST_BUNDLE.lock().ok().and_then(|bundle| bundle.clone());
    match bytes {
        Some(bytes) => {
            wasm_bindgen_futures::spawn_local(async move {
                if let Err(error) = start_playback(bytes, false).await {
                    show_error(&error);
                }
            });
            Ok(())
        }
        // 尚未加载作品：偏好已记下，下次 start 生效。
        None => Ok(()),
    }
}

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

/// 重新播放一份已加载的产物（`set_backend` 用；不 compare）。
async fn start_playback(bytes: Vec<u8>, compare: bool) -> Result<(), JsValue> {
    let (window, document) = prepare()?;
    run_with(window, document, bytes, compare, "重新播放 …").await
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
    if let Some(element) = document.get_element_by_id("zela-status") {
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

/// 渲染环境（surface / device / adapter）：整个页面生命周期**只建一次**。
///
/// 切后端（B11 §7.3）只重建 Runtime 并重播 ⇒ 同一块 device 复用，
/// 矢量后端的注入也只发生一次（`init_gpu` 本来就只能设一次）。
struct Display {
    surface: wgpu::Surface<'static>,
    ctx: RenderContext,
}

thread_local! {
    static DISPLAY: std::cell::RefCell<Option<std::rc::Rc<Display>>> =
        const { std::cell::RefCell::new(None) };
}

async fn display_for(document: &Document) -> Result<std::rc::Rc<Display>, JsValue> {
    if let Some(display) = DISPLAY.with(|slot| slot.borrow().clone()) {
        return Ok(display);
    }
    let display = std::rc::Rc::new(create_display(document).await?);
    DISPLAY.with(|slot| *slot.borrow_mut() = Some(display.clone()));
    Ok(display)
}

async fn create_display(document: &Document) -> Result<Display, JsValue> {
    let canvas: HtmlCanvasElement = document
        .get_element_by_id("zela-canvas")
        .ok_or_else(|| js_err("缺少 <canvas id=\"zela-canvas\">"))?
        .dyn_into()
        .map_err(|_| js_err("zela-canvas 不是 canvas"))?;
    canvas.set_width(WIDTH);
    canvas.set_height(HEIGHT);
    set_status(document, "WebGPU 初始化…");

    let instance = wgpu::Instance::default();
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas))
        .map_err(|e| js_err(format!("create_surface 失败：{e}")))?;
    // B11 Step 1：device features = 已注册且可用后端之并（矢量要求 float32 覆盖混合 +
    // float32 深度带模板）。不可用的后端不会被算进去 ⇒ 不会拒绝本来能起的机器。
    let mut registry = BackendRegistry::with_standard();
    registry
        .register(BackendKind::Vector)
        .map_err(|e| js_err(e.to_string()))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
            apply_limit_buckets: false,
        })
        .await
        .map_err(|e| js_err(format!("No suitable GPU adapter: {e}")))?;
    let caps = surface.get_capabilities(&adapter);
    let format = *caps
        .formats
        .first()
        .ok_or_else(|| js_err("surface 没有可用纹理格式"))?;
    let (device, queue) = RenderContext::request_device_with_registry(&adapter, &registry)
        .await
        .map_err(|e| js_err(e.to_string()))?;
    // B11 Step 5.3：adapter 必须随注入一起带上（否则能力校验静默消失）。
    let ctx = RenderContext::new(device.clone(), queue.clone(), format).with_adapter(adapter.clone());
    if registry.select_on(BackendKind::Vector, &adapter).is_ok() {
        zela_render_vector::init_gpu(device, queue, &adapter)
            .map_err(|e| js_err(format!("矢量后端注入失败：{e}")))?;
    }
    surface.configure(
        &ctx.device,
        &wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
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
    Ok(Display { surface, ctx })
}

/// fetch 入口：URL 参数 → 字节 → 公共播放实现。
async fn run() -> Result<(), JsValue> {
    let (window, document) = prepare()?;

    let params = UrlSearchParams::new_with_str(&window.location().search().unwrap_or_default())?;
    let zsb_url = params
        .get("zsb")
        .unwrap_or_else(|| "./fourier.zsb".to_string());
    let compare = params.get("compare").as_deref() == Some("1");

    let loading = format!("加载 {zsb_url} …");
    set_status(&document, &loading);
    let bytes = fetch_bytes(&zsb_url).await?;
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
    // B11 Step 5：新一代播放；旧循环在下一个 rAF 退出。
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    if let Ok(mut slot) = LAST_BUNDLE.lock() {
        *slot = Some(bytes.clone());
    }

    // B12 Step 1.1：浏览器入口也接上订阅器（默认 warn，不淹没控制台）。
    zela_playback::init_tracing();

    // 体积上限必须在解析之前校验（坏长度字段可能诱导巨量分配）。
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(js_err(format!(
            "ZSB 文件过大：{} B（上限 {MAX_BUNDLE_BYTES} B）",
            bytes.len()
        )));
    }

    set_status(&document, loading);

    // ── 1. canvas + WebGPU（只建一次；切后端重播时复用同一块 device）──────
    let display = display_for(&document).await?;
    let display_ctx = display.ctx.clone();
    let surface = &display.surface;

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
    // B11：作品级路由（第一帧决定；`set_backend` 会重新播放 ⇒ 每次播放重新决策）。
    let mut pinned: Option<RoutingDecision> = None;
    let mut scratch: Option<wgpu::Texture> = None;
    // 状态栏常驻的后端决策行（B11 §7.4：选了谁、为什么、谁被拒）。
    let mut backend_line = String::new();
    // 矢量后端的默认渲染参数（`PostCaps::all()` ⇒ 着色栈默认开，与 native 同口径）。
    let mut vector_params = zela_scene_bundle::RenderParams::default_for(
        &ze_render_ir::Capabilities {
            max_sample_count: 4,
            color_format: ze_render_ir::ColorFormat::Rgba8UnormSrgb,
            depth_format: ze_render_ir::DepthFormat::Depth32Float,
            post: BackendKind::Vector.post_caps(),
        },
    );
    vector_params.size = (WIDTH, HEIGHT);

    while index < frames {
        next_animation_frame(&window).await?;
        // 切后端 / 重新开始时，旧循环安静退出（不再抢 surface、不再画）。
        if GENERATION.load(Ordering::SeqCst) != generation {
            return Ok(());
        }

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

        // ── B11 Step 4：作品级路由（帧内容 × 后端自述能力）──────────────
        let preference = backend_preference();
        let mut pending: Option<VectorFrame> = None;
        let wanted = if preference == LoopBackend::Wgpu {
            backend_line = "后端：wgpu（显式） · ".to_string();
            BackendKind::Wgpu
        } else {
            let snapshot = VectorFrame::take(&runtime).map_err(js_err)?;
            let decision = match &pinned {
                Some(decision) if preference != LoopBackend::AutoFrame => decision.clone(),
                _ => route(
                    preference,
                    snapshot.content(),
                    display_ctx.adapter().map(AdapterFacts::of),
                )
                .map_err(js_err)?,
            };
            if pinned.as_ref() != Some(&decision) {
                // 决策与理由常驻状态栏（B11 §7.4）：进度 / 完成文案都把它带上，
                // 否则第一帧之后就被"帧 x/216"覆盖掉。
                backend_line = format!("后端：{} · ", decision.report());
                web_sys::console::log_1(&JsValue::from_str(backend_line.trim_end_matches(" · ")));
                pinned = Some(decision.clone());
            }
            pending = Some(snapshot);
            decision.backend
        };

        if wanted == BackendKind::Vector {
            let snapshot = pending.take().expect("矢量路径已有快照");
            let rgba = snapshot
                .render(
                    &runtime,
                    WIDTH,
                    HEIGHT,
                    &vector_params,
                    [clear.r as f32, clear.g as f32, clear.b as f32, clear.a as f32],
                )
                .map_err(js_err)?;
            present_vector_frame(&display_ctx, &mut scratch, &output, &rgba)?;
            display_ctx.queue.present(output);
            if compare {
                hasher.push_frame(&rgba);
            }
        } else {
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
        }

        end_frame(&runtime);
        index += 1;

        if index >= next_status {
            set_status(
                &document,
                &format!(
                    "{backend_line}帧 {index}/{frames} · {:.2}×",
                    playback_speed(compare)
                ),
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
            &format!("{backend_line}播放完成：{frames} 帧 · payload {payload_short}"),
        );
    }
    Ok(())
}

/// 矢量帧（RGBA8）→ surface 纹理：写进一张目标格式的纹理，再拷到 surface。
///
/// 为什么这样而不是一条全屏绘制管线：两者的**格式一致**（surface 配置已加 `COPY_DST`），
/// 一次 `copy_texture_to_texture` 就够，且不引入新的 shader / 顶点格式。
fn present_vector_frame(
    ctx: &RenderContext,
    scratch: &mut Option<wgpu::Texture>,
    output: &wgpu::SurfaceTexture,
    rgba: &[u8],
) -> Result<(), JsValue> {
    let target = zela_playback::rgba_to_target(ctx, rgba);
    let extent = wgpu::Extent3d {
        width: WIDTH,
        height: HEIGHT,
        depth_or_array_layers: 1,
    };
    let texture = scratch.get_or_insert_with(|| {
        ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("zela-vector-frame"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: ctx.target_format(),
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    });
    ctx.queue.write_texture(
        texture.as_image_copy(),
        &target,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(WIDTH * 4),
            rows_per_image: Some(HEIGHT),
        },
        extent,
    );
    let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("zela-vector-copy"),
    });
    encoder.copy_texture_to_texture(texture.as_image_copy(), output.texture.as_image_copy(), extent);
    ctx.queue.submit([encoder.finish()]);
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
