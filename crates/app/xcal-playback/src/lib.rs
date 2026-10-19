//! `xcal-playback` — **回放运行时**（浏览器与 native `--bundle` 共用）。
//!
//! # 为什么单独成 crate
//!
//! `xcal-app` 无条件依赖 Lua / Typst，**不能**进浏览器。但回放只需要
//! 「XSB payload → Scene → 调度器 → 帧回调」，与脚本运行时无关。
//!
//! # 防双轨
//!
//! 装配尾只有一份：[`assemble_runtime`]。
//! - `xcal-app`：脚本 / 内置场景 → 建 `Scene` → 调 [`assemble_runtime`]；
//! - 本 crate：`Bundle` → 还原 `SceneScript` → commit → 调 [`assemble_runtime`]。
//!
//! # 时钟
//!
//! 默认沿用 `xcal-schedule` 的安全零时钟（wasm 不注入也不 panic）；
//! native 宿主（`xcal-app`）在拿到 [`Runtime`] 后自行注入真实时钟，
//! 浏览器注入 `performance.now()` 实现。

use std::sync::Arc;

use glam::Vec3;
use xcal_render::{
    ContentTextureQueue, DrawQueue, DrawQueue3D, MorphQueue, RevealQueue, ViewRect,
};
use xcal_render_backends::{
    RenderContext, SceneCompositorCallback, ScreenParams, StandardBackends,
    render_frame_headless_async, render_frame_to_view,
};
#[cfg(feature = "blocking")]
use xcal_render_backends::render_frame_headless;
use xcal_scene::animation::{Binding, BindingTable, CameraEye};
use xcal_scene::contract::{GroupVisibility, ObjectId, SceneRead, WorldView};
use xcal_scene::director_state::DirectorState;
use xcal_scene::domain::{ContentTextureSpec, InputEvents, UiConfig};
use xcal_scene::scene::Scene;
use xcal_scene::{
    ClockForest, ClockNode, ComposerQueue, Curve, DeferredCommands, DriverCommand, DriverPod,
    DynamicObjects, LifetimeEvents, LifetimeTracker, SeekKind, TimeState,
};
use xcal_scene_bundle::{
    Available, Bundle, BundleError, Capability, Pod, RestoreError,
};
use xcal_schedule::resource::Resources;
use xcal_schedule::scheduler::Scheduler;
use xc_object::{Camera2DHandle, Camera3DHandle};

/// 回放装配失败（全部显式，不静默降级）。
#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    /// bundle 的可还原性检查失败（缺能力 / 版本不匹配）。
    #[error("{0}")]
    Restore(#[from] RestoreError),
    /// payload 解析 / 还原失败。
    #[error("XSB payload 还原失败：{0}")]
    Bundle(#[from] BundleError),
    /// 场景装配失败（例如没有 2D 相机）。
    #[error("{0}")]
    Assembly(String),
}

pub type Result<T> = std::result::Result<T, PlaybackError>;

/// 一次装配的产物：GUI / headless / 浏览器共用。
pub struct Runtime {
    /// 命令驱动的场景推进器。
    pub driver: Arc<DriverPod>,
    /// 渲染上下文（由宿主创建后传入）。
    pub render_ctx: RenderContext,
    /// 动画时长（秒）。
    pub duration: f64,
}

/// [`assemble_runtime`] 的输入：**场景已经建好**，只差资源 / 系统图 / Driver。
///
/// 脚本路径与 bundle 路径都先把 `Scene` 准备好，再走同一条尾部装配。
pub struct SceneAssembly {
    pub world: hecs::World,
    pub forest: ClockForest,
    pub bindings: BindingTable,
    pub camera2d: Camera2DHandle,
    pub group_visibility: GroupVisibility,
    /// 脚本时长；`<= 0` 时按最晚绑定结束时间推断。
    pub duration: f64,
    /// 可选 3D 相机（GUI demo 用）。
    pub camera3d: Option<Camera3DHandle>,
    /// 可选内容纹理规格（GUI demo 用）。
    pub content_texture: Option<ContentTextureSpec>,
}

/// **回放端能解释什么**：payload 是已烘焙的纯数据 ⇒ 只需要 scene 能力与 scene Pod。
///
/// 装配期用到的 content Pod 不在这里（见 XSB Meta 的 `required_pods` 语义）。
pub fn available_playback_caps() -> Available {
    Available::new(vec![scene_capability()], vec![scene_pod()])
}

fn scene_pod() -> Pod {
    Pod::new(
        xcal_scene::pod::SERVICE_NAME,
        xcal_scene::pod::SERVICE_VERSION,
    )
}

fn scene_capability() -> Capability {
    let meta = <xcal_scene::pod::SceneApi<'static> as xc_pod::Capability<'static>>::meta();
    let name = <xcal_scene::pod::SceneApi<'static> as xc_pod::Capability<'static>>::signatures()
        .first()
        .map(|signature| signature.namespace)
        .unwrap_or("");
    Capability::new(name, meta.version)
}

/// **XSB → Runtime**：可还原性检查 → 还原 `SceneScript` → commit → 公共尾装配。
pub fn build_bundle_runtime(render_ctx: RenderContext, bundle: &Bundle) -> Result<Runtime> {
    bundle.check_restorable(&available_playback_caps())?;

    let script = bundle.script()?;
    let duration = script.duration();

    let mut scene = Scene::new();
    let recipe = script
        .clone()
        .commit(&mut scene)
        .map_err(|e| PlaybackError::Assembly(format!("XSB commit 失败：{e}")))?;
    let camera2d = find_camera2d(&scene)?;
    let world = scene.into_world();

    Ok(assemble_runtime(
        render_ctx,
        SceneAssembly {
            world,
            forest: recipe.forest,
            bindings: recipe.bindings,
            camera2d,
            group_visibility: GroupVisibility::default(),
            duration,
            camera3d: None,
            content_texture: None,
        },
    ))
}

/// **唯一的装配尾**：资源插入序列 + System 注册 + Driver pod。
pub fn assemble_runtime(render_ctx: RenderContext, assembly: SceneAssembly) -> Runtime {
    let SceneAssembly {
        world,
        mut forest,
        mut bindings,
        camera2d,
        group_visibility,
        duration,
        camera3d,
        content_texture,
    } = assembly;

    // 2D 相机取景绑定（`center` + `scale`）交给 `CameraDriveSystem` 托管。
    let cam2d_bindings: Vec<Binding> = bindings
        .bindings
        .iter()
        .filter(|binding| binding.target.target_id() == camera2d.0)
        .cloned()
        .collect();

    // 3D 轨道相机：eye = radius * rotate_y(speed * t)，radius=5, speed=0.5。
    let orbit_clock = forest.insert(ClockNode::new(None)).unwrap();
    let orbit_binding = Binding {
        clock: orbit_clock,
        curve: Curve::Orbit {
            radius: 5.0,
            axis: Vec3::Y,
            speed: 0.5,
            initial_angle: 0.0,
        },
        // 无 3D 相机时指向悬垂 id（不会命中任何对象）。
        target: Arc::new(CameraEye {
            id: camera3d.map_or(ObjectId::DANGLING, |handle| handle.0),
        }),
        seek_kind: SeekKind::O1,
    };
    bindings.add(orbit_binding.clone());

    let duration = if duration > 0.0 {
        duration
    } else {
        forest_duration(&forest, &bindings)
    };

    let mut resources = Resources::new();
    resources.insert(camera2d);
    if let Some(camera3d) = camera3d {
        resources.insert(camera3d);
    }
    resources.insert(TimeState::default());
    resources.insert(UiConfig::default());
    resources.insert(xcal_scene::WorldTransforms::default());
    resources.insert(xcal_scene::TransformHierarchyError::default());
    resources.insert(ViewRect::default());
    resources.insert(InputEvents::default());
    resources.insert(DrawQueue::new());
    resources.insert(DrawQueue3D::new());
    if let Some(spec) = content_texture {
        resources.insert(spec);
        resources.insert(ContentTextureQueue::default());
    }
    resources.insert(MorphQueue::default());
    resources.insert(RevealQueue::default());
    resources.insert(forest);
    resources.insert(bindings);
    resources.insert(DirectorState::new());
    resources.insert(group_visibility);
    resources.insert(DeferredCommands::new());
    resources.insert(DynamicObjects::new());
    resources.insert(LifetimeTracker::new());
    resources.insert(LifetimeEvents::new());
    resources.insert(ComposerQueue::new());

    let mut scheduler = Scheduler::new(world, resources);
    xcal_scene::register::register_systems(&mut scheduler, cam2d_bindings, orbit_binding);
    xcal_render::register_record_systems(&mut scheduler);

    Runtime {
        driver: Arc::new(DriverPod::new(scheduler)),
        render_ctx,
        duration,
    }
}

/// 2D 相机的语义判据：场景里唯一带 `Framing2D` 的对象。
pub fn find_camera2d(scene: &Scene) -> Result<Camera2DHandle> {
    let mut cameras = Vec::new();
    scene.for_each::<xc_object::Framing2D>(|id, _| cameras.push(id));
    cameras
        .first()
        .copied()
        .map(Camera2DHandle)
        .ok_or_else(|| PlaybackError::Assembly("场景没有 2D 相机对象（缺少 Framing2D）".to_string()))
}

/// 动画时长 = 最晚结束的绑定（`local_offset + span.1`）；无绑定 ⇒ 0。
fn forest_duration(forest: &ClockForest, bindings: &BindingTable) -> f64 {
    bindings
        .bindings
        .iter()
        .filter_map(|binding| {
            let node = forest.get(binding.clock)?;
            let (_, end) = node.span?;
            Some(node.local_offset + end)
        })
        .fold(0.0, f64::max)
}

/// 设置画布/视口尺寸（物理像素）。宿主在每帧推进前调用一次。
pub fn set_view_rect(runtime: &Runtime, width: f32, height: f32) {
    runtime.driver.with_scheduler_mut(|scheduler| {
        scheduler.resources_mut().insert(ViewRect {
            rect: xc_notion::Rect2D::new(
                xc_notion::Vec2::ZERO,
                xc_notion::Vec2::new(width, height),
            ),
        });
    });
}

/// **帧循环一步**：推进一帧并构造本帧回调；无图层时返回 `None`。
pub fn step_frame(runtime: &Runtime, dt: f32) -> Option<SceneCompositorCallback> {
    runtime
        .driver
        .submit(DriverCommand::Advance { dt })
        .wait();

    runtime.driver.with_scheduler(|scheduler| {
        let options = xcal_render::FrameOptions::default();
        let view = xcal_render::frame::build_scene_view(
            &WorldView::new(scheduler.world()),
            scheduler.resources(),
            options,
        );
        xcal_render::frame::build_frame_inputs(&view, scheduler.resources(), options)
            .and_then(xcal_render_backends::frame::build_scene_callback)
    })
}

/// 帧末清理（**不能漏**：与 native 导出循环逐字一致）。
pub fn end_frame(runtime: &Runtime) {
    runtime
        .driver
        .with_scheduler_mut(|scheduler| xcal_render::frame::end_frame(scheduler.resources_mut()));
}

/// 推进一步并离屏渲染，返回**紧凑 RGBA8**（跨运行时对拍的规范字节）。
///
/// 同步版：仅 native `blocking` 构建可用（浏览器用 [`advance_and_render_async`]）。
#[cfg(feature = "blocking")]
pub fn advance_and_render(
    runtime: &Runtime,
    dt: f32,
    ctx: &RenderContext,
    backends: &mut StandardBackends,
    screen: ScreenParams,
    clear: wgpu::Color,
) -> Option<Vec<u8>> {
    let Some(callback) = step_frame(runtime, dt) else {
        end_frame(runtime);
        return None;
    };
    let pixels = render_frame_headless(ctx, backends, &callback, screen, clear);
    let rgba = to_rgba(ctx, &pixels);
    end_frame(runtime);
    Some(rgba)
}

/// 推进一步并离屏渲染（异步回读版，浏览器对拍用）。
pub async fn advance_and_render_async(
    runtime: &Runtime,
    dt: f32,
    ctx: &RenderContext,
    backends: &mut StandardBackends,
    screen: ScreenParams,
    clear: wgpu::Color,
) -> Option<Vec<u8>> {
    let Some(callback) = step_frame(runtime, dt) else {
        end_frame(runtime);
        return None;
    };
    let pixels = render_frame_headless_async(ctx, backends, &callback, screen, clear).await;
    let rgba = to_rgba(ctx, &pixels);
    end_frame(runtime);
    Some(rgba)
}

/// 直接把一帧渲染进给定 view（浏览器**显示**路径）。
pub fn render_step_to_view(
    runtime: &Runtime,
    dt: f32,
    ctx: &RenderContext,
    backends: &mut StandardBackends,
    screen: ScreenParams,
    clear: wgpu::Color,
    view: &wgpu::TextureView,
) -> bool {
    let Some(callback) = step_frame(runtime, dt) else {
        end_frame(runtime);
        return false;
    };
    render_frame_to_view(ctx, backends, &callback, screen, clear, view);
    end_frame(runtime);
    true
}

/// 把目标格式的紧凑像素转换为 **RGBA8**（仅通道顺序，不做域转换）。
///
/// 与 `xcal_app::export::to_rgba` 同一实现（后者改为委托本函数）。
pub fn to_rgba(ctx: &RenderContext, pixels: &[u8]) -> Vec<u8> {
    use wgpu::TextureFormat;

    let mut out = pixels.to_vec();
    match ctx.target_format() {
        TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb => {
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
        }
        _ => {}
    }
    out
}
