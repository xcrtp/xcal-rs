// xcal-app: 组装层（A 线瘦身后的**壳子**）
//
// 只负责：
//   - Pod 装配（scene / content / lua / python）；
//   - 建 Resources、注册各层 System（scene / render 各自的注册函数）；
//   - 提交 `Advance(dt)` 命令、订阅 `FrameFinished` 事件；
//   - 构建 GUI / headless 共用的 `Runtime`。
//
// GUI 的可执行壳在 `xcal-gui`（它依赖本 crate + `xcal-egui`）。
// 本 crate **不依赖 xcal-egui**：headless 导出与脚本装配都可独立使用。

use std::sync::Arc;

use xc_notion::Rect2D;
use xc_object::{
    CONTENT_TEXTURE_HEIGHT, CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH, Camera2D, Camera2DHandle,
    Framing2D,
};
use xcal_render::{
    ContentTextureQueue, DrawQueue, DrawQueue3D, MorphQueue, RevealQueue, ViewRect, scene::Scene,
};
use xcal_render_backends::RenderContext;
pub use xcal_scene::Result;
use xcal_scene::animation::{Binding, BindingTable, CameraEye};
use xcal_scene::contract::{GroupVisibility, ObjectId, SceneRead};
use xcal_scene::director_state::DirectorState;
use xcal_scene::domain::{ContentTextureSpec, InputEvents, UiConfig};
use xcal_scene::errors::Error;
use xcal_scene::scene::components::TextureHandle;
use xcal_scene::{
    ClockForest, ClockNode, ComposerQueue, Curve, DeferredCommands, DriverPod, DynamicObjects,
    LifetimeEvents, LifetimeTracker, SeekKind, TimeState,
};
use xcal_schedule::resource::Resources;
use xcal_schedule::scheduler::Scheduler;

// 内容纹理（S3-3）的 id 与尺寸已归位到对象层默认配置：
// `xc_object::{CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH, CONTENT_TEXTURE_HEIGHT}`。

mod assembly;
pub mod export;
mod formula;

/// **2D 场景选择**（启动参数切换，见 `main.rs`）。
///
/// 为什么是启动参数而不是 UI 开关：场景在 `build_scene_runtime` 里**一次性装配**（spawn 决定
/// `ObjectId`、时钟树与绑定表在装配期编译），运行态重建整张场景图需要尚未具备的
/// 生命周期设施（见「运行时创建/销毁对象」的记账）。故本轮用**启动参数**切换：
///
/// - `SceneChoice::Storyboard`（默认）：与 `--script` **同一条消费路径**
///   （加载 `storyboard.lua`；A3）；
/// - `SceneChoice::CapabilityDemo`：N 线能力测试 demo
///   （`xcal-scene::authored::capability_demo`）——最小场景，四项能力
///   （Name/Group/Visibility/Opacity）各落一处；
/// - `SceneChoice::ScriptStoryboard`：`--script` —— 与默认路径完全相同；
/// - `SceneChoice::PythonStoryboard`：`--python` —— 同一分镜由 **Python 脚本**编排
///   （`xc-pod-py`，默认装配的一部分），与 Lua 版等价。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SceneChoice {
    #[default]
    Storyboard,
    CapabilityDemo,
    ScriptStoryboard,
    PythonStoryboard,
}

/// 一次装配的产物：**GUI 与 headless 共用**。
pub struct Runtime {
    /// 命令驱动的场景推进器。
    pub driver: Arc<DriverPod>,
    /// 渲染上下文（headless 由调用方创建后传入）。
    pub render_ctx: RenderContext,
    /// 动画时长（秒）：脚本取 `SceneScript::duration()`，硬编码场景取最晚绑定结束时间。
    pub duration: f64,
}

/// 脚本语言（headless 按扩展名推断）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptLanguage {
    /// `.lua`（`xc-pod-lua`，始终可用）。
    Lua,
    /// `.py`（`xc-pod-py`，需要 `python` feature）。
    Python,
}

/// 按文件扩展名推断脚本语言（`.lua` / `.py`；其他 ⇒ 明确错误）。
pub fn language_from_path(path: &std::path::Path) -> Result<ScriptLanguage> {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("lua") => Ok(ScriptLanguage::Lua),
        Some("py") => Ok(ScriptLanguage::Python),
        other => Err(Error::InitializationError(format!(
            "不支持的脚本扩展名：{other:?}（只支持 .lua / .py）"
        ))),
    }
}

/// 场景来源：启动参数选定的内置场景、headless 指定的脚本，或 XSB 产物。
enum SceneSource {
    Choice(SceneChoice),
    Script {
        source: String,
        language: ScriptLanguage,
    },
    /// XSB 回放：已装配产物（纯数据），不经过任何脚本运行时。
    Bundle(xcal_scene::scene_script::SceneScript),
}

/// **headless 装配入口**：脚本 → 与 GUI 完全相同的 Runtime。
pub fn build_script_runtime(
    render_ctx: RenderContext,
    source: impl Into<String>,
    language: ScriptLanguage,
) -> Result<Runtime> {
    build_runtime(
        render_ctx,
        SceneSource::Script {
            source: source.into(),
            language,
        },
        // headless 脚本 = 完整作品：不叠加 demo3d。
        false,
    )
}

/// 脚本编译结果（`xcal-compile` 的中间产物，全部是纯数据）。
pub struct CompiledScript {
    /// 装配产物（会写进 `.xsb` 的 payload）。
    pub script: xcal_scene::scene_script::SceneScript,
    /// 参与装配的能力版本（写入 Meta）。
    pub capabilities: Vec<xcal_scene_bundle::Capability>,
    /// 参与装配的 Pod 版本（写入 Meta）。
    pub pods: Vec<xcal_scene_bundle::Pod>,
    /// 源脚本的 SHA-256（诊断用，不进内容 hash）。
    pub source_sha256: String,
}

/// **把脚本编译成装配产物**（不渲染、不需要 GPU）。
///
/// 与 `--script` / GUI 走**同一条装配路径**（`assembly::assemble` → `run_script`），
/// 差别只在"拿到 SceneScript 后写盘"而不是立刻 commit。
pub fn compile_script(source: &str, language: ScriptLanguage) -> Result<CompiledScript> {
    let engine = Arc::new(formula::build_engine());
    let assembly = assembly::assemble(engine)
        .map_err(|e| Error::InitializationError(format!("脚本 Pod 装配失败：{e}")))?;
    let script = run_script_source(&assembly, source, language)?;

    // **最小集**（不是"编译器注册的全部"）：只记录脚本实际用到的 capability。
    // - scene：出现了对象或时间线（`wait` 也算使用）；
    // - content：`content.formula` / `content.text` 被实际调用过。
    let scene_used = script.len() > 0 || script.duration() > 0.0;
    let content_used = assembly.content_used();

    Ok(CompiledScript {
        script,
        capabilities: used_capabilities(scene_used, content_used),
        pods: runtime_pods(scene_used, content_used),
        source_sha256: xcal_scene_bundle::sha256::hex(&xcal_scene_bundle::sha256::sha256(
            source.as_bytes(),
        )),
    })
}

/// 单个 capability 的版本声明（名字取自签名，唯一真源）。
fn capability_of<C: xc_pod::Capability<'static>>() -> xcal_scene_bundle::Capability {
    let meta = C::meta();
    let name = C::signatures()
        .first()
        .map(|signature| signature.namespace)
        .unwrap_or("");
    xcal_scene_bundle::Capability::new(name, meta.version)
}

fn scene_capability() -> xcal_scene_bundle::Capability {
    capability_of::<xcal_scene::pod::SceneApi<'static>>()
}

fn content_capability() -> xcal_scene_bundle::Capability {
    capability_of::<xcal_scene::pod::ContentApi<'static>>()
}

/// 脚本**实际用到的最小 capability 集**（XSB Meta 口径）。
fn used_capabilities(scene_used: bool, content_used: bool) -> Vec<xcal_scene_bundle::Capability> {
    let mut out = Vec::new();
    if scene_used {
        out.push(scene_capability());
    }
    if content_used {
        out.push(content_capability());
    }
    out
}

/// 当前运行时注册的**全部** capability（可还原性检查的"可用集"）。
fn runtime_capabilities() -> Vec<xcal_scene_bundle::Capability> {
    vec![scene_capability(), content_capability()]
}

/// 脚本**实际用到的最小 Pod 集**（XSB Meta 口径，与 [`used_capabilities`] 同口径）。
///
/// 未用到的 Pod 不得写进 Meta：回放端（尤其浏览器）可能根本没有 content Pod，
/// `check_restorable` 会因未使用的 Pod 误报拒载 —— 而几何早已烘焙进 payload。
fn runtime_pods(scene_used: bool, content_used: bool) -> Vec<xcal_scene_bundle::Pod> {
    let mut out = Vec::new();
    if scene_used {
        out.push(xcal_scene_bundle::Pod::new(
            xcal_scene::pod::SERVICE_NAME,
            xcal_scene::pod::SERVICE_VERSION,
        ));
    }
    if content_used {
        out.push(xcal_scene_bundle::Pod::new(
            xcal_scene::pod::CONTENT_SERVICE_NAME,
            xcal_scene::pod::CONTENT_SERVICE_VERSION,
        ));
    }
    out
}

/// 当前运行时注册的**全部** Pod（可还原性检查的"可用集"）。
fn all_runtime_pods() -> Vec<xcal_scene_bundle::Pod> {
    vec![
        xcal_scene_bundle::Pod::new(
            xcal_scene::pod::SERVICE_NAME,
            xcal_scene::pod::SERVICE_VERSION,
        ),
        xcal_scene_bundle::Pod::new(
            xcal_scene::pod::CONTENT_SERVICE_NAME,
            xcal_scene::pod::CONTENT_SERVICE_VERSION,
        ),
    ]
}

/// 当前运行时**能解释什么**（XSB 可还原性检查的"可用集"）。
pub fn available_runtime_caps() -> xcal_scene_bundle::Available {
    xcal_scene_bundle::Available::new(runtime_capabilities(), all_runtime_pods())
}

/// **headless 回放入口**：XSB 产物 → 与脚本路径**同一条** Runtime。
///
/// 顺序：可还原性检查（列全缺失项）→ 还原 `SceneScript` → 提交场景 → 建调度器。
pub fn build_bundle_runtime(
    render_ctx: RenderContext,
    bundle: &xcal_scene_bundle::Bundle,
) -> Result<Runtime> {
    bundle
        .check_restorable(&available_runtime_caps())
        .map_err(|e| Error::InitializationError(e.to_string()))?;
    let script = bundle
        .script()
        .map_err(|e| Error::InitializationError(format!("XSB payload 还原失败：{e}")))?;
    build_runtime(render_ctx, SceneSource::Bundle(script), false)
}

/// **GUI 装配入口**：内置场景 → Runtime（带 app 自带的 3D demo）。
///
/// GUI 壳（`xcal-gui`）用本入口；headless 脚本用 [`build_script_runtime`]。
pub fn build_scene_runtime(render_ctx: RenderContext, choice: SceneChoice) -> Result<Runtime> {
    build_runtime(render_ctx, SceneSource::Choice(choice), true)
}

/// 2D 相机的**语义判据**：场景里唯一带 `Framing2D` 的对象。
///
/// 不能按"第几个句柄"取：脚本可以任意安排创建顺序（当前坐标轴先于相机）。
fn find_camera2d(scene: &Scene) -> Result<Camera2DHandle> {
    let mut cameras = Vec::new();
    scene.for_each::<Framing2D>(|id, _| cameras.push(id));
    cameras.first().copied().map(Camera2DHandle).ok_or_else(|| {
        Error::InitializationError("场景没有 2D 相机对象（缺少 Framing2D）".to_string())
    })
}

/// **唯一的 System 注册入口**（回归测试共用）。
///
/// 注册序列归位到各层自己的注册函数 —— app 只按层次顺序调用：
/// - `xcal-scene::register`：场景 system（Director / 相机驱动 / 绑定 / 延迟命令 /
///   坐标轴与函数图刷新 / 时间与参数面板事件消费）；
/// - `xcal-render::register_record_systems`：五个录制 System。
///
/// 抽成函数的原因不变：`Scheduler::add_system` 在 debug 下做**静态借用冲突检查**并
/// 直接 `panic!`，单测分别装单个 System 测不出来；整张图由
/// `full_system_graph_assembles_without_borrow_conflicts` 覆盖。
fn register_systems(
    scheduler: &mut Scheduler,
    cam2d_bindings: Vec<Binding>,
    orbit_binding: Binding,
) {
    xcal_scene::register::register_systems(scheduler, cam2d_bindings, orbit_binding);
    xcal_render::register_record_systems(scheduler);
}

/// **唯一的场景装配实现**：GUI 与 headless 走同一条路径。
///
/// 分叉点只允许在"渲染上下文怎么来"（`eframe` vs headless wgpu）、
/// "每帧 dt 怎么来"（墙钟 vs 固定 `1/fps`）、以及**是否叠加 app 自带的 3D demo**
/// （GUI 默认叠加；headless 脚本是完整场景，不叠加）——场景、系统图、Driver pod 完全相同。
fn build_runtime(
    render_ctx: RenderContext,
    source: SceneSource,
    with_demo3d: bool,
) -> Result<Runtime> {
    let mut scene = Scene::new();

    // 排版引擎在 App 侧建一次：内容纹理、分镜脚本与内容服务共用。
    let engine = Arc::new(formula::build_engine());

    // 3D 场景（线框立方体 + 参数曲面 + 内容纹理公式）由场景层声明（A6）。
    // headless 脚本是**独立作品**：不叠加 app 的 demo3d（否则 3D 物体会盖住脚本画面，
    // 且脚本无法关闭该图层）。
    let demo3d = with_demo3d
        .then(|| xcal_scene::authored::demo3d::build(&mut scene, engine.as_ref()));

    // ── 2D 场景：脚本 / 能力 demo（启动参数切换，见 `SceneChoice`）──────────
    // 两条来源产出**同形的三样东西**：时钟树 + 绑定表 + 主 2D 相机句柄；
    // demo 额外产出一个"待插入的组级开关"（N2 的组控制）。
    let (forest2d, bindings2d, camera2d_handle, group_visibility, script_duration) =
        build_scene_source(&mut scene, &engine, &source)?;

    let world = scene.into_world();

    // ── 时钟树 + 绑定表（动画唯一驱动链路）────────────────────────
    // 2D 场景已把时间编排编译进 `forest2d` / `bindings2d`；下面只补一条与
    // 2D 编排无关的手工绑定：3D 轨道相机（时间源 = 全局 t）。
    //
    // K3：这条绑定由 `CameraDriveSystem` **托管**——"视口操作"切到 3D 时
    // 它会从表里移出（让位给交互）、切回 2D 时放回。
    let mut forest = forest2d;
    let mut bindings = bindings2d;
    // 「2D 相机自由」开关：分镜里指向 2D 相机（`Camera2DFraming`）的绑定
    // （`center` + `scale` 各一条）也被 `CameraDriveSystem` 托管——开关打开时从表里
    // 移出（否则每帧覆盖用户的 Pan/Zoom）、关闭时放回（分镜驱动）。
    // 能力 demo 没有 2D 相机取景绑定 ⇒ 此向量为空，托管自然退化为 no-op。
    //
    // T0-②：筛选口径从"按 target 类别"改为"**按对象**"（`target_id() == 相机句柄`）
    // —— 比按类别更精确：真正想要的是"作用于**这台**相机的绑定"。
    let cam2d_bindings: Vec<Binding> = bindings
        .bindings
        .iter()
        .filter(|b| b.target.target_id() == camera2d_handle.0)
        .cloned()
        .collect();
    let orbit_clock = forest.insert(ClockNode::new(None)).unwrap();
    let orbit_binding = Binding {
        clock: orbit_clock,
        // 3D 轨道相机：eye = radius * rotate_y(speed * t)，radius=5, speed=0.5
        curve: Curve::Orbit {
            radius: 5.0,
            axis: glam::Vec3::Y,
            speed: 0.5,
            initial_angle: 0.0,
        },
        // 无 demo3d 时指向悬垂 id（不会命中任何对象）；CameraDriveSystem 仍拿到一条
        // 形状合法的绑定，不必为 headless 分叉场景层签名。
        target: Arc::new(CameraEye {
            id: demo3d
                .as_ref()
                .map_or(ObjectId::DANGLING, |demo| demo.camera3d.0),
        }),
        seek_kind: SeekKind::O1,
    };
    bindings.add(orbit_binding.clone());

    // 时长：脚本给的是显式 `duration()`；无脚本时取最晚绑定结束时间。
    let duration = if script_duration > 0.0 {
        script_duration
    } else {
        forest_duration(&forest, &bindings)
    };

    let mut resources = Resources::new();
    // P1-1：相机状态**不再是 Resource** —— 场景里有一个 2D 相机对象，
    // 这里只放它的**句柄**（引用型，P2）；K3 再加 3D 相机句柄。
    resources.insert(camera2d_handle);
    if let Some(demo) = &demo3d {
        resources.insert(demo.camera3d);
    }
    resources.insert(TimeState::default());
    resources.insert(UiConfig::default());
    // S5：父子变换累乘的派生态（每帧由 TransformHierarchySystem 重建）。
    resources.insert(xcal_scene::WorldTransforms::default());
    resources.insert(xcal_scene::TransformHierarchyError::default());
    resources.insert(ViewRect::default());
    resources.insert(InputEvents::default());
    resources.insert(DrawQueue::new());
    resources.insert(DrawQueue3D::new());
    // S3-3 内容纹理：配置（唯一真相来源）+ 当帧录制结果。
    // 仅 demo3d 需要它（参数曲面采样公式纹理）；headless 脚本没有 3D 曲面 ⇒ 不声明该图层。
    if let Some(demo) = &demo3d {
        // 内容相机缺省（公式编译失败）时退回一个"世界原点居中、无内容"的相机。
        let content_camera = demo.content_camera.unwrap_or_else(|| {
            Camera2D::new(
                (0.0, 0.0).into(),
                25.0,
                Rect2D::new(
                    xc_notion::Vec2::ZERO,
                    xc_notion::Vec2::new(
                        CONTENT_TEXTURE_WIDTH as f32,
                        CONTENT_TEXTURE_HEIGHT as f32,
                    ),
                ),
            )
        });
        resources.insert(ContentTextureSpec {
            camera: content_camera,
            size_px: [CONTENT_TEXTURE_WIDTH, CONTENT_TEXTURE_HEIGHT],
            texture: TextureHandle::new(CONTENT_TEXTURE_ID),
            composite: false,
        });
        resources.insert(ContentTextureQueue::default());
    }
    // S4.3 形变：当帧的形变项（`from`/`to` 常量 + 逐帧 `t`），由 MorphRecordSystem 写。
    resources.insert(MorphQueue::default());
    // R0 书写动画：当帧的 reveal 项（路径常量 + 区间 [start, end]）。
    resources.insert(RevealQueue::default());
    resources.insert(render_ctx.clone());
    resources.insert(forest);
    resources.insert(bindings);
    resources.insert(DirectorState::new());
    // N3：组级可见性开关（纯数据资源；默认空 ⇒ 所有组可见）。
    // 录制路径（`SceneRecordSystem` 等）以"可缺席资源"方式读取，缺席同样按空处理。
    // 分镜场景给默认空；能力 demo 也用默认空（组控制由测试填上以验证 N2）。
    resources.insert(group_visibility);
    // P6/B3：运行时延迟命令队列（帧末由 `DeferredApplySystem` 应用）。
    resources.insert(DeferredCommands::new());
    // P6/B4：运行时创建对象台账（重播清理用）。
    resources.insert(DynamicObjects::new());
    // S1：生命周期事件的差集状态与最近批次（opt-in：只有 LifetimeTracked 对象进集合）。
    resources.insert(LifetimeTracker::new());
    resources.insert(LifetimeEvents::new());
    // S3：合成器的派生态（相机无关内容索引；无 Composer 时保持空）。
    resources.insert(ComposerQueue::new());

    let mut scheduler = Scheduler::new(world, resources);
    register_systems(&mut scheduler, cam2d_bindings, orbit_binding);

    Ok(Runtime {
        driver: Arc::new(DriverPod::new(scheduler)),
        render_ctx,
        duration,
    })
}

/// 场景来源 → 时钟树 / 绑定表 / 2D 相机 / 组开关 / 脚本时长。
///
/// 抽象出来是为了让 GUI（`SceneChoice`）与 headless（任意脚本路径）**共用**
/// 同一条装配路径：两者都落到同一组 `ClockNode + Binding`。
fn build_scene_source(
    scene: &mut Scene,
    engine: &Arc<xc_prerender::InMemoryEngine>,
    source: &SceneSource,
) -> Result<(
    ClockForest,
    BindingTable,
    Camera2DHandle,
    GroupVisibility,
    f64,
)> {
    // ── 任意脚本（headless）──
    if let SceneSource::Script { source, language } = source {
        let assembly = assembly::assemble(Arc::clone(engine))
            .map_err(|e| Error::InitializationError(format!("脚本 Pod 装配失败：{e}")))?;
        let script = run_script_source(&assembly, source, *language)?;
        eprintln!(
            "[xcal-app] 场景 = 脚本分镜（{} 个对象，{}s）",
            script.len(),
            script.duration()
        );
        let duration = script.duration();
        let recipe = script
            .commit(scene)
            .map_err(|e| Error::InitializationError(format!("脚本 commit 失败：{e}")))?;
        // **不能假设第 0 个句柄是相机**：脚本先建坐标轴容器。
        let camera = find_camera2d(scene)?;
        return Ok((
            recipe.forest,
            recipe.bindings,
            camera,
            GroupVisibility::default(),
            duration,
        ));
    }

    // ── XSB 产物回放（纯数据，无脚本运行时）──
    if let SceneSource::Bundle(script) = source {
        let duration = script.duration();
        eprintln!(
            "[xcal-app] 场景 = XSB 产物（{} 个对象，{}s）",
            script.len(),
            duration
        );
        let recipe = script
            .clone()
            .commit(scene)
            .map_err(|e| Error::InitializationError(format!("XSB commit 失败：{e}")))?;
        let camera = find_camera2d(scene)?;
        return Ok((
            recipe.forest,
            recipe.bindings,
            camera,
            GroupVisibility::default(),
            duration,
        ));
    }

    let SceneSource::Choice(choice) = source else {
        unreachable!("Script / Bundle 分支已在上面返回")
    };

    match choice {
        // A3：默认启动与 `--script` **合并为同一条消费路径**（都加载
        // `storyboard.lua`）——不再有"硬编码版 / 脚本版"两条分支。
        // `storyboard.rs` 仍保留在 `xcal-scene::authored` 作等价性参照基准。
        SceneChoice::Storyboard | SceneChoice::ScriptStoryboard => {
            // 全部 6 个镜头由 **Lua 脚本**编排（装配期一次）。App 显式声明
            // 节点（scene / content / lua），Broker 负责校验、排序与注入。
            build_scene_source(
                scene,
                engine,
                &SceneSource::Script {
                    source: assembly::STORYBOARD_LUA.to_string(),
                    language: ScriptLanguage::Lua,
                },
            )
        }
        SceneChoice::CapabilityDemo => {
            // N 线能力测试 demo：最小场景（两条曲线 + 一个对照组）。
            // 默认**不隐藏任何组**（全可见）⇒ 看得见 `cos` 淡出（N4）。
            eprintln!("[xcal-app] 场景 = 能力 demo（N 线：Name / Group / Visibility / Opacity）");
            let d = xcal_scene::authored::capability_demo::build(scene);
            Ok((d.forest, d.bindings, d.camera, d.group_visibility, 0.0))
        }
        SceneChoice::PythonStoryboard => {
            #[cfg(not(feature = "python"))]
            {
                Err(Error::InitializationError(
                    "Python 适配器未启用（编译时关闭了 python feature）".to_string(),
                ))
            }
            #[cfg(feature = "python")]
            {
                // 同一分镜由 **Python 脚本**编排。与硬编码版的等价性由
                // `python_storyboard_matches_hardcoded` 锁定。
                build_scene_source(
                    scene,
                    engine,
                    &SceneSource::Script {
                        source: assembly::STORYBOARD_PY.to_string(),
                        language: ScriptLanguage::Python,
                    },
                )
            }
        }
    }
}

/// 执行脚本（语言由 [`ScriptLanguage`] 决定；Python 未启用 ⇒ 明确报错）。
fn run_script_source(
    assembly: &assembly::PodAssembly,
    source: &str,
    language: ScriptLanguage,
) -> Result<xcal_scene::scene_script::SceneScript> {
    match language {
        ScriptLanguage::Lua => assembly
            .run_script(source)
            .map_err(|e| Error::InitializationError(format!("Lua 分镜执行失败：{e}"))),
        ScriptLanguage::Python => {
            #[cfg(not(feature = "python"))]
            {
                let _ = (assembly, source);
                Err(Error::InitializationError(
                    "Python 适配器未启用（编译时关闭了 python feature）".to_string(),
                ))
            }
            #[cfg(feature = "python")]
            {
                assembly
                    .run_python_script(source)
                    .map_err(|e| Error::InitializationError(format!("Python 分镜执行失败：{e}")))
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use xc_object::{Camera2DHandle, Camera3D, Camera3DHandle, Framing2D, Projection};
    use xcal_scene::ClockForest;
    use xcal_scene::animation::{BindingTable, Camera2DFraming, CameraEye, SeekKind};
    use xcal_scene::contract::object_id_of;
    use xcal_scene::domain::CameraMode;
    use xcal_scene::{AnimValue, Interp};
    use xcal_schedule::scheduler::Phase;

    /// **A3 验收**：默认启动与 `--script` 走**同一条消费路径**（都加载
    /// `storyboard.lua`）——对象数、时长与绑定数完全一致。
    #[test]
    fn default_and_script_share_one_consumption_path() {
        use xc_prerender::placeholder_engine;

        let engine = Arc::new(placeholder_engine());
        let mut scene_default = Scene::new();
        let (_forest_a, bindings_a, camera_a, _, duration_a) = build_scene_source(
            &mut scene_default,
            &engine,
            &SceneSource::Choice(SceneChoice::Storyboard),
        )
        .expect("默认场景装配成功");

        let mut scene_script = Scene::new();
        let (_forest_b, bindings_b, camera_b, _, duration_b) = build_scene_source(
            &mut scene_script,
            &engine,
            &SceneSource::Choice(SceneChoice::ScriptStoryboard),
        )
        .expect("--script 场景装配成功");

        assert_eq!(scene_default.len(), scene_script.len(), "对象数一致");
        assert_eq!(
            bindings_a.bindings.len(),
            bindings_b.bindings.len(),
            "绑定数一致"
        );
        assert_eq!(duration_a, duration_b, "时长一致");
        assert_eq!(duration_a, 4.5, "默认分镜时长 = 脚本 duration()");
        let _ = (camera_a, camera_b);
    }

    /// **回归（实测踩过）**：整张 System 图的**注册 + 运行一帧**都不得冲突。
    ///
    /// 坑：3D 模式 System 原先与 `ParamPanelSystem` 同处 `LogicUpdate` 且都写
    /// `BindingTable` ⇒ `Scheduler::add_system` 的**静态借用冲突检查在 app 启动时
    /// panic**（`Scheduler conflict in phase LogicUpdate ...`），而"每个 System 各装
    /// 一次的"单测完全测不出来 —— 完整 GUI 装配需要 `eframe::CreationContext`，跑不到。
    ///
    /// 修法：相机驱动 System 移到 `Input`（那里没有别的 `BindingTable` 写者），并把它与
    /// `CameraInputSystem` 的先后固定为「先改模式、后解释输入」。切换开关（2D 自由 / 3D
    /// 视口目标）都会走到这条路径。
    /// 本用例走与 GUI / headless **同一个** `register_systems` ⇒ 注册序列变化会被它挡住。
    #[test]
    fn full_system_graph_assembles_without_borrow_conflicts() {
        let mut world = hecs::World::new();
        // K3 的对象形态：3D 相机对象 + 模式组件。
        let camera3d = world.spawn((
            Camera3D::new(
                glam::Vec3::new(0.0, 0.0, 5.0),
                glam::Vec3::ZERO,
                glam::Vec3::Y,
                Projection::perspective(1.0, 1.0, 0.1, 100.0),
            ),
            CameraMode::PATH,
        ));
        let camera2d = world.spawn((Framing2D::default(),));

        // 自动轨道绑定（`CameraDriveSystem` 会托管让位/复位）。
        let mut forest = ClockForest::new();
        let clock = forest.insert(ClockNode::new(None)).unwrap();
        let orbit = Binding {
            clock,
            curve: Curve::Orbit {
                radius: 5.0,
                axis: glam::Vec3::Y,
                speed: 0.5,
                initial_angle: 0.0,
            },
            target: Arc::new(CameraEye {
                id: object_id_of(camera3d),
            }),
            seek_kind: SeekKind::O1,
        };
        // 分镜的 2D 相机绑定（`free_2d_camera` 会托管让位/复位）。
        let cam2d_clock = forest.insert(ClockNode::new(None)).unwrap();
        let cam2d_framing = Binding {
            clock: cam2d_clock,
            curve: Curve::Keyframes {
                keys: vec![
                    (0.0, AnimValue::Scalar(40.0)),
                    (1.0, AnimValue::Scalar(110.0)),
                ],
                interpolation: Interp::Linear,
            },
            target: Arc::new(Camera2DFraming {
                id: object_id_of(camera2d),
            }),
            seek_kind: SeekKind::O1,
        };
        let mut bindings = BindingTable::new();
        bindings.add(orbit.clone());
        bindings.add(cam2d_framing.clone());

        let mut res = Resources::new();
        res.insert(Camera2DHandle(object_id_of(camera2d)));
        res.insert(Camera3DHandle(object_id_of(camera3d)));
        res.insert(UiConfig::default());
        res.insert(TimeState::default());
        res.insert(DirectorState::new());
        res.insert(InputEvents::default());
        res.insert(ViewRect::default());
        res.insert(DrawQueue::new());
        res.insert(DrawQueue3D::new());
        res.insert(MorphQueue::default());
        res.insert(RevealQueue::default());
        res.insert(forest);
        res.insert(bindings);

        let mut sched = Scheduler::new(world, res);
        // 注册即校验：debug 下有冲突会 panic（这就是要挡住的东西）。
        register_systems(&mut sched, vec![cam2d_framing], orbit);
        sched.init_resources();
        // 运行期借用（BorrowTracker）也不得冲突。
        sched.run_frame(0.016);
    }

    /// **B0 验收**：装配层能经 `xcal_scene` 写"用契约层 `spawn_dyn` 动态创建对象"的代码。
    ///
    /// 可证伪：`xcal_scene::contract` 若不转发写 trait `Scene`，本测试**编译失败**。
    /// 这里也顺带钉住登记表（`add_known`）齐备性：`Name` 未登记 ⇒ `spawn_dyn` 返回 `None`。
    #[test]
    fn app_layer_can_spawn_dyn_via_contract() {
        use xcal_scene::contract::{
            Components, Name, Scene as SceneTrait, SceneRead, find_by_name,
        };
        use xcal_scene::scene::Scene;

        let mut scene = Scene::new();
        let components: Components = vec![Box::new(Name::new("hero"))];
        let id = scene
            .spawn_dyn(components)
            .expect("Name 已登记 ⇒ 动态创建成功（未登记会返回 None）");
        assert!(scene.contains(id));
        assert_eq!(find_by_name(&scene, "hero"), Some(id));
        assert_eq!(scene.len(), 1);
    }

    // ─── B3 GPU 像素级：延迟命令的相位顺序陷阱（§9.2.1 / §9.2.3）────────────

    /// 离屏回读尺寸（与 [`deferred_gpu_geometry`] 的视口一致）。
    const DEF_W: u32 = 240;
    const DEF_H: u32 = 200;

    fn deferred_gpu_geometry() -> xcal_render_backends::Frame2DGeometry {
        xcal_render_backends::Frame2DGeometry {
            viewport_points: Rect2D::new(
                xc_notion::Vec2::new(100.0, 50.0),
                xc_notion::Vec2::new(340.0, 250.0),
            ),
            pixels_per_point: 1.0,
            screen_size_px: [400, 300],
        }
    }

    /// "亮像素"计数（通道序无关：背景透明/黑都为 0，红描边 ≈255）。
    fn count_lit(px: &[u8]) -> usize {
        px.chunks_exact(4)
            .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 150)
            .count()
    }

    /// 🔴 **B3 验收（GPU 像素级）**：延迟 spawn 的对象**本帧不出现、下一帧出现**。
    ///
    /// 判据：帧 N 回读的亮像素 = 0（新对象尚未创建，录制先于 FrameEnd）；
    /// 帧 N+1 回读的亮像素 > 0。若把 `DeferredApplySystem` 挪到 `RenderCommit`
    /// 之前（破坏相位顺序），帧 N 就会有像素 ⇒ 本用例 RED。
    #[test]
    fn deferred_spawn_pixels_appear_next_frame() {
        use xc_notion::Transform2D;
        use xc_object::{Circle, Framing2D, spawn_camera2d};
        use xcal_render::{DrawQueue, MeasureSpace, SceneRecordSystem};
        use xcal_render_backends::{RenderContext, WgpuBackend2D};
        use xcal_scene::contract::WorldScene;
        use xcal_scene::types::Rect2D;
        use xcal_scene::{DeferredApplySystem, DeferredCommands, DynamicObjects};

        let ctx = RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
            .expect("headless GPU 上下文");
        let mut backend = WgpuBackend2D::new(ctx.device(), ctx.queue());

        // 场景：只有相机（对象稍后经延迟命令创建）。
        let mut world = hecs::World::new();
        let camera = {
            let mut ws = WorldScene::new(&mut world);
            spawn_camera2d(&mut ws, Framing2D::new(glam::Vec2::ZERO, 25.0))
                .expect("Framing2D 已登记")
        };

        let view_rect = Rect2D::new(
            xc_notion::Vec2::new(100.0, 50.0),
            xc_notion::Vec2::new(340.0, 250.0),
        );
        let mut res = Resources::new();
        res.insert(camera);
        res.insert(ViewRect { rect: view_rect });
        res.insert(DrawQueue::new());
        res.insert(DeferredCommands::new());
        res.insert(DynamicObjects::new());

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, SceneRecordSystem);
        sched.add_system(Phase::FrameEnd, DeferredApplySystem);
        sched.init_resources();

        let render = |sched: &Scheduler,
                      backend: &mut WgpuBackend2D,
                      ctx: &RenderContext,
                      g: &xcal_render_backends::Frame2DGeometry|
         -> Vec<u8> {
            use xcal_scene::domain::LayerId;
            let queue = sched.resources().get::<DrawQueue>().expect("DrawQueue");
            backend.begin_frame_at(*g);
            for cmd in queue.commands() {
                backend.push(&cmd.shape, &Transform2D::identity(), &cmd.style, cmd.z.0);
            }
            let mut enc = ctx
                .device()
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("xcal-deferred-frame"),
                });
            backend.finish_frame(&mut enc, LayerId(0), g);
            ctx.queue().submit([enc.finish()]);
            ctx.read_texture(
                backend.offscreen_texture(LayerId(0)).unwrap(),
                (DEF_W, DEF_H),
            )
        };

        // 帧 N 前：入队一条延迟创建（半径 1 世界单位 ≈ 25px 的红色描边圆）。
        sched
            .resources_mut()
            .get_mut::<DeferredCommands>()
            .unwrap()
            .spawn(vec![
                Box::new(Circle::new((0.0, 0.0), 1.0)),
                Box::new(xcal_render::StrokeSolid::new_in(
                    8.0,
                    (255, 0, 0, 255),
                    MeasureSpace::Screen,
                )),
            ]);

        let g = deferred_gpu_geometry();
        // ⚠️ `record_scene_tagged` **不清队列**（清理由宿主 `end_frame` 负责）——
        // 测试里必须每帧前清空，否则上一帧的命令会残留在回读里。
        let clear_queue = |sched: &mut Scheduler| {
            sched
                .resources_mut()
                .get_mut::<DrawQueue>()
                .unwrap()
                .clear();
        };

        clear_queue(&mut sched);
        sched.run_frame(0.0);
        let lit_n = count_lit(&render(&sched, &mut backend, &ctx, &g));
        eprintln!("[deferred-gpu] 帧 N 亮像素 = {lit_n}（期望 0：新对象下一帧才出现）");

        clear_queue(&mut sched);
        sched.run_frame(0.0);
        let lit_n1 = count_lit(&render(&sched, &mut backend, &ctx, &g));
        eprintln!("[deferred-gpu] 帧 N+1 亮像素 = {lit_n1}（期望 > 0）");

        assert_eq!(lit_n, 0, "延迟 spawn 的对象本帧不得出现（相位顺序陷阱）");
        assert!(lit_n1 > 0, "延迟 spawn 的对象下一帧必须出现，实测 {lit_n1}");

        // ── §9.2.2：延迟 despawn 后对象消失（下一帧生效）──
        let runtime_id = sched
            .resources()
            .get::<DynamicObjects>()
            .unwrap()
            .ids
            .first()
            .copied()
            .expect("运行时创建的对象已登记");
        sched
            .resources_mut()
            .get_mut::<DeferredCommands>()
            .unwrap()
            .despawn(runtime_id);
        clear_queue(&mut sched);
        sched.run_frame(0.0);
        let lit_after_enqueue = count_lit(&render(&sched, &mut backend, &ctx, &g));
        assert!(
            lit_after_enqueue > 0,
            "延迟 despawn 本帧仍可见（FrameEnd 才删）"
        );
        clear_queue(&mut sched);
        sched.run_frame(0.0);
        let lit_gone = count_lit(&render(&sched, &mut backend, &ctx, &g));
        eprintln!("[deferred-gpu] despawn 后亮像素 = {lit_gone}（期望 0）");
        assert_eq!(lit_gone, 0, "延迟 despawn 的对象下一帧必须消失");
    }

    // ─── W3：完整脚本分镜 ⇔ 硬编码分镜 ─────────────────────────────────

    #[derive(Debug, PartialEq)]
    struct BindRow {
        object: usize,
        offset: f64,
        span: (f64, f64),
        mode: String,
        target: String,
        curve: String,
    }

    /// 浮点量化到 1e-4（计划允许 Lua f64 → f32 的 1 ulp 差异）。
    fn qf(value: f32) -> i64 {
        (value as f64 * 1e4).round() as i64
    }

    fn rgba(color: xcal_scene::types::Color32) -> String {
        format!("{},{},{},{}", color.r(), color.g(), color.b(), color.a())
    }

    fn path_bounds(path: &xc_notion::Path2D) -> String {
        match path.compute_bounds() {
            Some(bounds) => format!(
                "{},{},{},{}",
                qf(bounds.min.x),
                qf(bounds.min.y),
                qf(bounds.max.x),
                qf(bounds.max.y)
            ),
            None => "none".to_string(),
        }
    }

    /// 逐组件指纹（类型 + 值；浮点量化）。
    fn object_fingerprint(scene: &Scene, id: xcal_scene::contract::ObjectId) -> String {
        use xc_object::{Arrow, Axes2D, Framing2D, Path as ObjectPath, Transform};
        use xcal_render::{FillSolid, StrokeSolid};
        use xcal_scene::contract::SceneRead;
        use xcal_scene::scene::components::Reveal;

        let mut parts: Vec<String> = Vec::new();
        if let Some(text) = scene.with::<Axes2D, _>(id, |a| {
            format!(
                "axes(tick={},grid={}:{}:{:?},axis={}:{}:{:?},x={:?},y={:?})",
                qf(a.tick.min_px),
                qf(a.grid_stroke.width),
                rgba(a.grid_stroke.color),
                a.grid_stroke.space,
                qf(a.axis_stroke.width),
                rgba(a.axis_stroke.color),
                a.axis_stroke.space,
                a.x_range,
                a.y_range
            )
        }) {
            parts.push(text);
        }
        if let Some(text) = scene.with::<Framing2D, _>(id, |f| {
            format!(
                "frame({},{},{})",
                qf(f.center.x),
                qf(f.center.y),
                qf(f.scale)
            )
        }) {
            parts.push(text);
        }
        if let Some(text) =
            scene.with::<Reveal, _>(id, |r| format!("reveal({})", path_bounds(&r.path)))
        {
            parts.push(text);
        }
        if let Some(text) =
            scene.with::<ObjectPath, _>(id, |p| format!("path({})", path_bounds(&p.0)))
        {
            parts.push(text);
        }
        if let Some(text) = scene.with::<Arrow, _>(id, |a| {
            format!(
                "arrow({},{},{},{},{},{})",
                qf(a.0.from.x),
                qf(a.0.from.y),
                qf(a.0.to.x),
                qf(a.0.to.y),
                qf(a.0.head_length),
                qf(a.0.head_width)
            )
        }) {
            parts.push(text);
        }
        if let Some(text) = scene.with::<FillSolid, _>(id, |f| format!("fill({})", rgba(f.color))) {
            parts.push(text);
        }
        if let Some(text) = scene.with::<StrokeSolid, _>(id, |s| {
            format!("stroke({}:{}:{:?})", qf(s.width), rgba(s.color), s.space)
        }) {
            parts.push(text);
        }
        if let Some(text) = scene.with::<Transform, _>(id, |t| {
            format!(
                "transform({},{},{})",
                qf(t.position.x),
                qf(t.position.y),
                qf(t.position.z)
            )
        }) {
            parts.push(text);
        }
        parts.join("|")
    }

    /// 故事板对象（创建顺序 = `ObjectId.index` 升序；无 despawn）。
    fn storyboard_ids(scene: &Scene) -> Vec<xcal_scene::contract::ObjectId> {
        use xc_object::{Arrow, Axes2D, Framing2D, Path as ObjectPath};
        use xcal_scene::contract::{ObjectId, SceneRead};
        use xcal_scene::scene::components::Reveal;

        let mut ids: Vec<ObjectId> = Vec::new();
        scene.for_each::<Axes2D>(|id, _| ids.push(id));
        scene.for_each::<Framing2D>(|id, _| ids.push(id));
        scene.for_each::<Reveal>(|id, _| ids.push(id));
        scene.for_each::<ObjectPath>(|id, _| ids.push(id));
        scene.for_each::<Arrow>(|id, _| ids.push(id));
        ids.sort_by_key(|id| id.index);
        ids.dedup();
        ids
    }

    /// Fourier 作品的对象全集（含纯层级节点 / 圆点 / Name / Parent）。
    fn fourier_ids(scene: &Scene) -> Vec<xcal_scene::contract::ObjectId> {
        use xc_object::{Arrow, Axes2D, Circle, Framing2D, Path as ObjectPath, Transform};
        use xcal_scene::contract::{Name, ObjectId, SceneRead};
        use xcal_scene::scene::components::Reveal;

        let mut ids: Vec<ObjectId> = Vec::new();
        scene.for_each::<Axes2D>(|id, _| ids.push(id));
        scene.for_each::<Framing2D>(|id, _| ids.push(id));
        scene.for_each::<Reveal>(|id, _| ids.push(id));
        scene.for_each::<ObjectPath>(|id, _| ids.push(id));
        scene.for_each::<Arrow>(|id, _| ids.push(id));
        scene.for_each::<Circle>(|id, _| ids.push(id));
        scene.for_each::<Name>(|id, _| ids.push(id));
        scene.for_each::<Transform>(|id, _| ids.push(id));
        ids.sort_by_key(|id| id.index);
        ids.dedup();
        ids
    }

    /// Fourier 作品的逐组件指纹（在通用指纹上补 Circle / Name / Parent / Opacity）。
    fn fourier_fingerprint(
        scene: &Scene,
        id: xcal_scene::contract::ObjectId,
    ) -> String {
        use xc_object::Circle;
        use xcal_scene::contract::{Name, Parent, SceneRead};
        use xcal_render::Opacity;

        let mut parts = vec![object_fingerprint(scene, id)];
        if let Some(text) = scene.with::<Circle, _>(id, |c| {
            format!(
                "circle({},{},{})",
                qf(c.0.center.x),
                qf(c.0.center.y),
                qf(c.0.radius)
            )
        }) {
            parts.push(text);
        }
        if let Some(text) = scene.with::<Name, _>(id, |n| format!("name({})", n.0)) {
            parts.push(text);
        }
        if let Some(text) = scene.with::<Parent, _>(id, |p| {
            format!("parent({},{})", p.0.index, p.0.generation)
        }) {
            parts.push(text);
        }
        if let Some(text) = scene.with::<Opacity, _>(id, |o| format!("opacity({})", qf(o.0))) {
            parts.push(text);
        }
        parts.join("|")
    }

    fn binding_rows(
        forest: &ClockForest,
        bindings: &BindingTable,
        ids: &[xcal_scene::contract::ObjectId],
    ) -> Vec<BindRow> {
        bindings
            .bindings
            .iter()
            .map(|binding| {
                let node = forest.get(binding.clock).expect("时钟节点在");
                let span = node.span.expect("Timeline 编译的节点必有 span");
                let object = ids
                    .iter()
                    .position(|id| *id == binding.target.target_id())
                    .expect("target 必须指向本场景对象");
                BindRow {
                    object,
                    offset: node.local_offset,
                    span,
                    mode: format!("{:?}", node.mode),
                    target: binding.target.name().to_string(),
                    curve: format!("{:?}", binding.curve),
                }
            })
            .collect()
    }

    /// **W3 验收**：脚本版（全部 6 镜头）与硬编码版逐对象、逐绑定相等。
    ///
    /// 覆盖：对象数、组件类型**与值**、绑定数、offset / span / mode、曲线与 target。
    #[test]
    fn script_storyboard_matches_hardcoded() {
        use xc_prerender::placeholder_engine;

        // 硬编码参考实现。
        let mut scene_a = Scene::new();
        let engine_a = placeholder_engine();
        let sb = xcal_scene::authored::storyboard::build(&mut scene_a, &engine_a);

        // 脚本版：同一个 placeholder 引擎（App 侧真实路径用 Typst 引擎）。
        let assembly = crate::assembly::assemble(std::sync::Arc::new(placeholder_engine()))
            .expect("脚本 Pod 装配成功（scene + content + lua）");
        let script = assembly
            .run_script(crate::assembly::STORYBOARD_LUA)
            .expect("Lua 分镜执行成功");
        let mut scene_b = Scene::new();
        let recipe = script.commit(&mut scene_b).expect("commit");

        let ids_a = storyboard_ids(&scene_a);
        let ids_b = storyboard_ids(&scene_b);
        assert_eq!(ids_a.len(), ids_b.len(), "对象数必须一致");
        assert_eq!(recipe.handles.len(), ids_b.len(), "句柄数 = 对象数");

        let fingerprints_a: Vec<String> = ids_a
            .iter()
            .map(|id| object_fingerprint(&scene_a, *id))
            .collect();
        let fingerprints_b: Vec<String> = ids_b
            .iter()
            .map(|id| object_fingerprint(&scene_b, *id))
            .collect();
        assert_eq!(fingerprints_a, fingerprints_b, "组件类型与值必须逐对象相等");

        let rows_a = binding_rows(&sb.forest, &sb.bindings, &ids_a);
        let rows_b = binding_rows(&recipe.forest, &recipe.bindings, &ids_b);
        assert_eq!(rows_a, rows_b, "时序 / 曲线 / target 必须全等");

        eprintln!(
            "[W3] 对象 {} / 绑定 {}；前 3 个对象指纹:\n{:#?}",
            fingerprints_a.len(),
            rows_a.len(),
            &fingerprints_a[..fingerprints_a.len().min(3)]
        );
    }

    /// **D 线验收**：Python 版分镜与硬编码版逐对象、逐绑定相等。
    #[cfg(feature = "python")]
    #[test]
    fn python_storyboard_matches_hardcoded() {
        use xc_prerender::placeholder_engine;

        // 硬编码参考实现。
        let mut scene_a = Scene::new();
        let engine_a = placeholder_engine();
        let sb = xcal_scene::authored::storyboard::build(&mut scene_a, &engine_a);

        // Python 版：同一个 placeholder 引擎。
        let assembly = crate::assembly::assemble(std::sync::Arc::new(placeholder_engine()))
            .expect("Pod 装配成功（含 Python）");
        let script = assembly
            .run_python_script(crate::assembly::STORYBOARD_PY)
            .expect("Python 分镜执行成功");
        let mut scene_b = Scene::new();
        let recipe = script.commit(&mut scene_b).expect("commit");

        let ids_a = storyboard_ids(&scene_a);
        let ids_b = storyboard_ids(&scene_b);
        assert_eq!(ids_a.len(), ids_b.len(), "对象数必须一致");
        assert_eq!(recipe.handles.len(), ids_b.len(), "句柄数 = 对象数");

        let fingerprints_a: Vec<String> = ids_a
            .iter()
            .map(|id| object_fingerprint(&scene_a, *id))
            .collect();
        let fingerprints_b: Vec<String> = ids_b
            .iter()
            .map(|id| object_fingerprint(&scene_b, *id))
            .collect();
        assert_eq!(fingerprints_a, fingerprints_b, "组件类型与值必须逐对象相等");

        let rows_a = binding_rows(&sb.forest, &sb.bindings, &ids_a);
        let rows_b = binding_rows(&recipe.forest, &recipe.bindings, &ids_b);
        assert_eq!(rows_a, rows_b, "时序 / 曲线 / target 必须全等");

        eprintln!(
            "[D-PY] 对象 {} / 绑定 {}",
            fingerprints_a.len(),
            rows_a.len()
        );
    }

    /// **本轮验收**：`fourier.lua` 与 `fourier.py` 必须产出**同一份场景**。
    ///
    /// 与 `python_storyboard_matches_hardcoded` 同口径：对象数、逐对象组件指纹、
    /// 时序 / 曲线 / target 逐行相等；另加时长一致（两版脚本的时间编排等价）。
    #[cfg(feature = "python")]
    #[test]
    fn python_fourier_matches_lua() {
        use xc_prerender::placeholder_engine;

        let assembly = crate::assembly::assemble(std::sync::Arc::new(placeholder_engine()))
            .expect("脚本 Pod 装配成功（含 Python）");
        let lua = assembly
            .run_script(include_str!("../scripts/fourier.lua"))
            .expect("Lua 傅里叶脚本执行成功");
        let py = assembly
            .run_python_script(include_str!("../scripts/fourier.py"))
            .expect("Python 傅里叶脚本执行成功");
        assert_eq!(lua.duration(), py.duration(), "两侧时长必须一致");
        let duration = lua.duration();

        let mut scene_lua = Scene::new();
        let mut scene_py = Scene::new();
        let recipe_lua = lua.commit(&mut scene_lua).expect("Lua commit");
        let recipe_py = py.commit(&mut scene_py).expect("Python commit");

        let ids_lua = fourier_ids(&scene_lua);
        let ids_py = fourier_ids(&scene_py);
        assert_eq!(ids_lua.len(), ids_py.len(), "对象数必须一致");
        assert_eq!(
            recipe_lua.handles.len(),
            ids_lua.len(),
            "Lua 句柄数 = 对象数"
        );
        assert_eq!(
            recipe_py.handles.len(),
            ids_py.len(),
            "Python 句柄数 = 对象数"
        );

        let fingerprints_lua: Vec<String> = ids_lua
            .iter()
            .map(|id| fourier_fingerprint(&scene_lua, *id))
            .collect();
        let fingerprints_py: Vec<String> = ids_py
            .iter()
            .map(|id| fourier_fingerprint(&scene_py, *id))
            .collect();
        assert_eq!(
            fingerprints_lua, fingerprints_py,
            "组件类型与值必须逐对象相等"
        );

        let rows_lua = binding_rows(&recipe_lua.forest, &recipe_lua.bindings, &ids_lua);
        let rows_py = binding_rows(&recipe_py.forest, &recipe_py.bindings, &ids_py);
        assert_eq!(rows_lua, rows_py, "时序 / 曲线 / target 必须全等");

        eprintln!(
            "[fourier] 对象 {} / 绑定 {} / 时长 {duration:.3}s",
            fingerprints_lua.len(),
            rows_lua.len()
        );
    }

    /// **步骤 2 验收**：`CompiledScript.capabilities` 是**脚本实际用到的最小集**，
    /// 不是"编译器注册的全部 capability"（否则回放端会被要求具备用不到的 cap）。
    #[test]
    fn compiled_capabilities_are_the_minimal_used_set() {
        // 只用 scene：没有任何 content.formula / content.text 调用。
        let scene_only = r#"
scene.spawn({ Circle { x = 0.0, y = 0.0, r = 1.0 } })
"#;
        let compiled = compile_script(scene_only, ScriptLanguage::Lua).expect("编译 scene-only 脚本");
        let names: Vec<&str> = compiled
            .capabilities
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, vec!["xcal.scene"], "只用到 scene，不得含 content");

        // fourier 用 content.text 生成标题 ⇒ scene + content。
        let compiled = compile_script(
            include_str!("../scripts/fourier.lua"),
            ScriptLanguage::Lua,
        )
        .expect("编译 fourier");
        let names: Vec<&str> = compiled
            .capabilities
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert!(names.contains(&"xcal.scene"), "names = {names:?}");
        assert!(names.contains(&"xcal.content"), "names = {names:?}");
    }

    /// **S4 验收**：`CompiledScript.pods` 与 capabilities 同口径 ——
    /// 只用 `scene.spawn({Circle})` 时 Meta 的 pods **仅 scene**。
    #[test]
    fn compiled_pods_are_the_minimal_used_set() {
        let scene_only = r#"
scene.spawn({ Circle { x = 0.0, y = 0.0, r = 1.0 } })
"#;
        let compiled = compile_script(scene_only, ScriptLanguage::Lua).expect("编译 scene-only 脚本");
        let names: Vec<&str> = compiled
            .pods
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![xcal_scene::pod::SERVICE_NAME],
            "只用到 scene，Meta 的 pods 不得含 content"
        );

        // fourier 用了 content.text ⇒ scene + content 两个 Pod。
        let compiled = compile_script(
            include_str!("../scripts/fourier.lua"),
            ScriptLanguage::Lua,
        )
        .expect("编译 fourier");
        let names: Vec<&str> = compiled
            .pods
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(
            names,
            vec![
                xcal_scene::pod::SERVICE_NAME,
                xcal_scene::pod::CONTENT_SERVICE_NAME
            ],
            "用到 content 时两个 Pod 都必须在 Meta 中"
        );
    }

    /// **XSB 三路径同构**：`script` → `.xsb` → 还原，产物与直接脚本逐对象 / 逐绑定相等。
    ///
    /// 三条路径（默认 / `--script` / `--bundle`）共用 `build_runtime`；本用例锁死
    /// "装配产物"这一层等价，防止 `.xsb` 引入新的双轨真源。
    #[test]
    fn bundle_roundtrip_matches_script_path() {
        use xc_prerender::placeholder_engine;
        use xcal_scene_bundle::{Bundle, MetaInput};

        let assembly = crate::assembly::assemble(std::sync::Arc::new(placeholder_engine()))
            .expect("脚本 Pod 装配成功");
        let script = assembly
            .run_script(include_str!("../scripts/fourier.lua"))
            .expect("Lua 傅里叶脚本执行成功");

        let available = available_runtime_caps();
        let bundle = Bundle::encode(
            &script,
            MetaInput {
                capabilities: available.capabilities.clone(),
                pods: available.pods.clone(),
                downgrades: Vec::new(),
                source_sha256: None,
            },
        )
        .expect("编码 XSB");

        let bytes = bundle.to_bytes();
        let decoded = Bundle::from_bytes(&bytes).expect("解析 XSB");
        decoded.check_restorable(&available).expect("可还原");
        let restored = decoded.script().expect("还原 SceneScript");

        // 最强口径：规范 JSON 逐字节相同。
        assert_eq!(
            script.to_json().expect("serialize"),
            restored.to_json().expect("serialize"),
            "还原后的装配产物必须与原始脚本完全一致"
        );

        let mut scene_a = Scene::new();
        let mut scene_b = Scene::new();
        let recipe_a = script.clone().commit(&mut scene_a).expect("commit a");
        let recipe_b = restored.commit(&mut scene_b).expect("commit b");
        let ids_a = fourier_ids(&scene_a);
        let ids_b = fourier_ids(&scene_b);
        assert_eq!(ids_a.len(), ids_b.len(), "对象数必须一致");
        assert_eq!(
            ids_a
                .iter()
                .map(|id| fourier_fingerprint(&scene_a, *id))
                .collect::<Vec<_>>(),
            ids_b
                .iter()
                .map(|id| fourier_fingerprint(&scene_b, *id))
                .collect::<Vec<_>>(),
            "逐对象组件指纹必须一致"
        );
        assert_eq!(
            binding_rows(&recipe_a.forest, &recipe_a.bindings, &ids_a),
            binding_rows(&recipe_b.forest, &recipe_b.bindings, &ids_b),
            "时序 / 曲线 / target 必须一致"
        );
    }

    /// **S4 验收**：同一 target 的多段绑定「推近 → 停留 → 拉回」有段语义。
    #[test]
    fn fourier_camera_push_pull_uses_segment_semantics() {
        use xc_object::Framing2D;
        use xc_prerender::placeholder_engine;
        use xcal_scene::animation::BindingApplySystem;
        use xcal_schedule::resource::Resources;
        use xcal_schedule::scheduler::{Phase, Scheduler};

        let assembly = crate::assembly::assemble(std::sync::Arc::new(placeholder_engine()))
            .expect("脚本 Pod 装配成功");
        let script = assembly
            .run_script(include_str!("../scripts/fourier.lua"))
            .expect("Lua 傅里叶脚本执行成功");
        let mut scene = Scene::new();
        let recipe = script.commit(&mut scene).expect("commit");

        let world = scene.into_world();

        let mut resources = Resources::new();
        resources.insert(TimeState { elapsed: 0.0 });
        resources.insert(recipe.forest);
        resources.insert(recipe.bindings);

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::SceneSync, BindingApplySystem);
        sched.init_resources();

        let scale_at = |sched: &mut Scheduler, t: f32| -> f32 {
            sched
                .resources_mut()
                .get_mut::<TimeState>()
                .unwrap()
                .elapsed = t;
            sched.run_frame(0.0);
            sched
                .world()
                .query::<&Framing2D>()
                .iter()
                .next()
                .unwrap()
                .scale
        };

        assert!(
            (scale_at(&mut sched, 4.0) - 55.0).abs() < 1e-3,
            "推近前必须是全景"
        );
        let mid = scale_at(&mut sched, 5.2);
        assert!(mid > 55.0 && mid < 90.0, "推近段内应介于 55..90，实际 {mid}");
        assert!(
            (scale_at(&mut sched, 6.0) - 90.0).abs() < 1e-3,
            "停留期在 90"
        );
        assert!(
            (scale_at(&mut sched, 7.2) - 55.0).abs() < 1e-3,
            "拉回后回到全景"
        );
    }

    /// **S4 验收**：`Curve.Keyframes` / `ScalarKey` / `KeyframeInterp` 在两侧语言都可构造。
    #[cfg(feature = "python")]
    #[test]
    fn python_keyframes_curve_matches_lua() {
        use xc_prerender::placeholder_engine;

        let assembly = crate::assembly::assemble(std::sync::Arc::new(placeholder_engine()))
            .expect("脚本 Pod 装配成功");
        let lua_source = r#"
local o = scene.spawn({ Transform { x = 0.0, y = 0.0, z = 0.0 } })
scene.animate(o, scene.clip_transform(TransformChannel.Rotation, Curve.Keyframes {
    keys = {
        ScalarKey { t = 0.0, value = 0.0 },
        ScalarKey { t = 1.0, value = 1.5 },
    },
    interpolation = KeyframeInterp.Linear,
}), 1.0)
"#;
        let py_source = r#"
o = scene.spawn([Transform(x=0.0, y=0.0, z=0.0)])
scene.animate(o, scene.clip_transform(TransformChannel.Rotation, Curve.Keyframes(
    keys=[
        ScalarKey(t=0.0, value=0.0),
        ScalarKey(t=1.0, value=1.5),
    ],
    interpolation=KeyframeInterp.Linear,
)), 1.0)
"#;
        let lua = assembly.run_script(lua_source).expect("Lua Keyframes 脚本");
        let py = assembly
            .run_python_script(py_source)
            .expect("Python Keyframes 脚本");
        assert_eq!(lua.duration(), py.duration());

        let mut scene_lua = Scene::new();
        let mut scene_py = Scene::new();
        let recipe_lua = lua.commit(&mut scene_lua).expect("Lua commit");
        let recipe_py = py.commit(&mut scene_py).expect("Python commit");
        let ids_lua = fourier_ids(&scene_lua);
        let ids_py = fourier_ids(&scene_py);
        assert_eq!(
            ids_lua
                .iter()
                .map(|id| fourier_fingerprint(&scene_lua, *id))
                .collect::<Vec<_>>(),
            ids_py
                .iter()
                .map(|id| fourier_fingerprint(&scene_py, *id))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            binding_rows(&recipe_lua.forest, &recipe_lua.bindings, &ids_lua),
            binding_rows(&recipe_py.forest, &recipe_py.bindings, &ids_py)
        );
    }

    /// **D+ 验收**：capability 是普通 Python 模块 —— `import xcal.scene` 可用。
    #[cfg(feature = "python")]
    #[test]
    fn python_script_can_import_xcal_modules() {
        use xc_prerender::placeholder_engine;

        let assembly =
            crate::assembly::assemble(std::sync::Arc::new(placeholder_engine())).expect("装配成功");
        let script = assembly
            .run_python_script(
                r#"
import xcal
import xcal.scene as scene2
from xcal import Circle, scene
from xcal.scene import spawn

assert xcal.scene is scene2
assert scene.spawn is spawn

scene.spawn([Circle(x=0.0, y=0.0, r=1.0)])
scene2.spawn([Circle(x=1.0, y=0.0, r=1.0)])
"#,
            )
            .expect("Python import 脚本成功");

        let mut scene = Scene::new();
        let recipe = script.commit(&mut scene).expect("commit");
        assert_eq!(recipe.handles.len(), 2);
    }

    /// 回归（本次踩过）：**相机不能按"第 0 个句柄"取** —— 脚本先建坐标轴。
    ///
    /// 判据：按 `Framing2D` 组件找到唯一的 2D 相机，且它不是第 0 个对象。
    #[test]
    fn script_camera_is_found_by_framing_component() {
        use xc_prerender::placeholder_engine;
        use xcal_scene::contract::SceneRead;

        let assembly =
            crate::assembly::assemble(std::sync::Arc::new(placeholder_engine())).expect("装配成功");
        let script = assembly
            .run_script(crate::assembly::STORYBOARD_LUA)
            .expect("Lua 分镜执行成功");
        let mut scene = Scene::new();
        let recipe = script.commit(&mut scene).expect("commit");

        let camera = crate::find_camera2d(&scene).expect("能按 Framing2D 找到相机");
        assert_ne!(
            camera.0, recipe.handles[0],
            "第 0 个对象是坐标轴容器，不是相机"
        );
        assert_eq!(
            scene.with::<Framing2D, _>(camera.0, |f| *f),
            Some(Framing2D::new(glam::Vec2::ZERO, 40.0)),
            "找到的必须是初始取景的 2D 相机"
        );
    }

    // ─── B4：Python 双向对称性 ────────────────────────────────────────

    #[cfg(feature = "python")]
    #[test]
    fn python_lua_symmetry() {
        use std::sync::Arc;
        use xc_object::Circle as NativeCircle;
        use xc_prerender::placeholder_engine;
        use xcal_scene::contract::SceneRead;
        use xcal_scene::scene::components::Reveal;

        let assembly = crate::assembly::assemble(Arc::new(placeholder_engine()))
            .expect("Python + Lua 装配成功");
        let report = assembly.broker.plan().validate().expect("装配图合法");
        assert!(
            report.order.iter().any(|name| name == "python"),
            "python 节点必须参与装配"
        );

        let script = assembly
            .run_script(
                r#"
                local data = python.linspace(-1.0, 1.0, 5)
                local pts = {}
                for i = 1, #data do
                    pts[i] = Vec2 { x = (i - 1) * 0.5, y = data[i] }
                end
                scene.spawn({ Reveal { points = pts, closed = false } })
                python.make_dot(2.0, 3.0, 0.5)
                "#,
            )
            .expect("Lua 双向脚本成功");

        let mut scene = Scene::new();
        let recipe = script.commit(&mut scene).expect("commit");
        assert_eq!(recipe.handles.len(), 2, "Lua 一个 + Python 一个");

        // Lua 侧：Python 的 linspace 数据真的进了 Reveal（y 范围 [-1, 1]）。
        let mut bounds = None;
        scene.for_each::<Reveal>(|_, reveal| {
            bounds = reveal.path.compute_bounds();
        });
        let bounds = bounds.expect("Reveal 有包围盒");
        assert!((bounds.min.y + 1.0).abs() < 1e-4);
        assert!((bounds.max.y - 1.0).abs() < 1e-4);

        // Python 侧：经注入的 scene.* 建出的圆。
        let mut circles = Vec::new();
        scene.for_each::<NativeCircle>(|_, circle| {
            circles.push(*circle);
        });
        assert_eq!(circles.len(), 1);
        assert!((circles[0].0.center.x - 2.0).abs() < 1e-4);
        assert!((circles[0].0.center.y - 3.0).abs() < 1e-4);
        assert!((circles[0].0.radius - 0.5).abs() < 1e-4);
    }

    #[cfg(feature = "python")]
    #[test]
    fn python_is_optional() {
        use std::sync::Arc;
        use xc_prerender::placeholder_engine;

        // 描述层：未挂 python 节点时 optional 缺失只记账、不阻断。
        let report = crate::assembly::plan(false)
            .validate()
            .expect("optional 缺失可装配");
        assert!(
            report
                .optional_missing
                .iter()
                .all(|req| req.service == "python"),
            "缺失项必须全部来自 python"
        );
        assert!(!report.optional_missing.is_empty());

        // 默认装配（python 是默认 feature）包含 python 节点，Lua 侧能看到 python 表。
        let assembly = crate::assembly::assemble(Arc::new(placeholder_engine())).expect("装配成功");
        assert!(!assembly.lua.registered_global_names("python").is_empty());
    }
}

/// **T1 / T2 / T3 的 GPU 像素级验收**（计划 §2.4 / §3.4 / §4.4）。
///
/// 与既有 GPU 用例同一口径：headless wgpu 离屏渲染 + 像素回读 ——
/// **不得只靠 System / 队列链路断言**。
#[cfg(test)]
mod t_capabilities_gpu {
    use xc_notion::{Rect2D, Transform2D, Vec2};
    use xc_object::{Camera2D, Transform};
    use xcal_render::components::{Arrow, Circle, Rect};
    use xcal_render::{DrawQueue, FillSolid, Opacity, StrokeSolid, record_scene_tagged};
    use xcal_render_backends::{Frame2DGeometry, RenderContext, WgpuBackend2D};
    use xcal_scene::contract::{GroupId, GroupVisibility, Visibility, WorldView};
    use xcal_scene::domain::LayerId;

    /// 离屏尺寸（紧凑像素，行距 = `W * 4`；目标格式字节序，默认 BGRA）。
    const W: u32 = 160;
    const H: u32 = 120;
    /// 相机缩放：1 世界单位 = 20px；世界原点落在屏幕中心 (80, 60)。
    const SCALE: f32 = 20.0;

    fn geometry() -> Frame2DGeometry {
        Frame2DGeometry {
            viewport_points: Rect2D::new(Vec2::ZERO, Vec2::new(W as f32, H as f32)),
            pixels_per_point: 1.0,
            screen_size_px: [W, H],
        }
    }

    fn camera() -> Camera2D {
        Camera2D::new(
            Vec2::ZERO,
            SCALE,
            Rect2D::new(Vec2::ZERO, Vec2::new(W as f32, H as f32)),
        )
    }

    /// 走**完整录制路径**（`record_scene_tagged`）→ GPU 离屏渲染 → 回读像素。
    fn render(world: &hecs::World, groups: &GroupVisibility) -> Vec<u8> {
        let ctx = RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
            .expect("headless GPU 上下文");
        let mut backend = WgpuBackend2D::new(ctx.device(), ctx.queue());

        let mut q = DrawQueue::new();
        record_scene_tagged(&WorldView::new(world), &camera(), None, groups, &mut q);

        let g = geometry();
        backend.begin_frame_at(g);
        for cmd in q.commands() {
            backend.push(&cmd.shape, &Transform2D::identity(), &cmd.style, cmd.z.0);
        }
        let mut enc = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xcal-t-capabilities"),
            });
        backend.finish_frame(&mut enc, LayerId(0), &g);
        ctx.queue().submit([enc.finish()]);
        ctx.read_texture(backend.offscreen_texture(LayerId(0)).unwrap(), (W, H))
    }

    fn render_visible(world: &hecs::World) -> Vec<u8> {
        render(world, &GroupVisibility::default())
    }

    fn is_lit(p: &[u8]) -> bool {
        p[0] as u32 + p[1] as u32 + p[2] as u32 > 150
    }

    fn count_lit(px: &[u8]) -> usize {
        px.chunks_exact(4).filter(|p| is_lit(p)).count()
    }

    /// 亮像素的 x 重心（无亮像素 ⇒ `NaN`）。
    fn centroid_x(px: &[u8]) -> f32 {
        let mut sum = 0.0f64;
        let mut n = 0u32;
        for (i, p) in px.chunks_exact(4).enumerate() {
            if is_lit(p) {
                sum += ((i as u32) % W) as f64;
                n += 1;
            }
        }
        if n == 0 {
            f32::NAN
        } else {
            (sum / n as f64) as f32
        }
    }

    /// 亮像素包围盒 `(min_x, min_y, max_x, max_y)`；无亮像素 ⇒ `None`。
    fn bbox(px: &[u8]) -> Option<(u32, u32, u32, u32)> {
        let mut out: Option<(u32, u32, u32, u32)> = None;
        for (i, p) in px.chunks_exact(4).enumerate() {
            if !is_lit(p) {
                continue;
            }
            let (x, y) = ((i as u32) % W, (i as u32) / W);
            out = Some(match out {
                None => (x, y, x, y),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
            });
        }
        out
    }

    /// 亮像素最多的列（x）—— 用来判定"头在哪一端"（头底是最宽处）。
    fn widest_column_x(px: &[u8]) -> u32 {
        let mut counts = vec![0u32; W as usize];
        for (i, p) in px.chunks_exact(4).enumerate() {
            if is_lit(p) {
                counts[(i as u32 % W) as usize] += 1;
            }
        }
        let (mut best, mut best_n) = (0u32, 0u32);
        for (x, n) in counts.into_iter().enumerate() {
            if n > best_n {
                best = x as u32;
                best_n = n;
            }
        }
        best
    }

    fn white_fill() -> FillSolid {
        FillSolid::new((255, 255, 255, 255))
    }

    fn transform_at(x: f32, y: f32) -> Transform {
        Transform {
            position: glam::Vec3::new(x, y, 0.0),
            ..Default::default()
        }
    }

    // ─── T1 ─────────────────────────────────────────────────────────────

    /// **T1-2**：`position = (1, 0)` ⇒ 像素整体右移 `1 × scale = 20px`。
    #[test]
    fn t1_translation_shifts_pixels_by_scale() {
        let mut base = hecs::World::new();
        base.spawn((Circle::new((0.0, 0.0), 0.5), white_fill()));
        let mut moved = hecs::World::new();
        moved.spawn((
            Circle::new((0.0, 0.0), 0.5),
            white_fill(),
            transform_at(1.0, 0.0),
        ));

        let (a, b) = (render_visible(&base), render_visible(&moved));
        let (ca, cb) = (centroid_x(&a), centroid_x(&b));
        eprintln!("[T1-GPU] 位移：重心 x {ca:.2} → {cb:.2}（期望 +{SCALE}）");
        assert!(
            (ca - 80.0).abs() < 2.0,
            "基准圆心应在屏幕中心 80，实际 {ca}"
        );
        assert!(
            (cb - ca - SCALE).abs() < 2.0,
            "右移 1 世界单位应位移 {SCALE}px，实际 {}",
            cb - ca
        );
    }

    /// **T1-3**：`scale = 2` ⇒ 覆盖像素数显著增大（半径 ×2 ⇒ 面积 ×4）。
    #[test]
    fn t1_scale_increases_covered_pixels() {
        let mut base = hecs::World::new();
        base.spawn((Circle::new((0.0, 0.0), 0.5), white_fill()));
        let mut scaled = hecs::World::new();
        scaled.spawn((
            Circle::new((0.0, 0.0), 0.5),
            white_fill(),
            Transform {
                scale: glam::Vec3::splat(2.0),
                ..Default::default()
            },
        ));

        let (n0, n1) = (
            count_lit(&render_visible(&base)),
            count_lit(&render_visible(&scaled)),
        );
        eprintln!("[T1-GPU] 缩放：亮像素 {n0} → {n1}（期望约 ×4）");
        assert!(n0 > 0, "基准圆必须有像素");
        assert!(n1 > n0 * 2, "scale = 2 ⇒ 覆盖面积应显著增大（{n0} → {n1}）");
    }

    /// **T1-4**：绕 Z 旋转 45° ⇒ 包围盒变化正确（矩形从 40×10px 变成约 35×35px）。
    #[test]
    fn t1_rotation_changes_bounding_box() {
        let rect = Rect::new((-1.0, -0.25), (1.0, 0.25));
        let mut base = hecs::World::new();
        base.spawn((rect, white_fill()));
        let mut rotated = hecs::World::new();
        rotated.spawn((
            rect,
            white_fill(),
            Transform {
                rotation: glam::Quat::from_rotation_z(std::f32::consts::FRAC_PI_4),
                ..Default::default()
            },
        ));

        let b0 = bbox(&render_visible(&base)).expect("基准矩形有像素");
        let b1 = bbox(&render_visible(&rotated)).expect("旋转后矩形有像素");
        let (h0, h1) = (b0.3 - b0.1, b1.3 - b1.1);
        let (w0, w1) = (b0.2 - b0.0, b1.2 - b1.0);
        eprintln!("[T1-GPU] 旋转：包围盒 {w0}×{h0} → {w1}×{h1}");
        assert!((w0 as i32 - 40).abs() <= 2, "基准宽应约 40px，实际 {w0}");
        assert!((h0 as i32 - 10).abs() <= 2, "基准高应约 10px，实际 {h0}");
        assert!(
            h1 > h0 * 2 && w1 < w0,
            "旋转 45° 后应明显变高、变窄（{w0}×{h0} → {w1}×{h1}）"
        );
    }

    // ─── T2 ─────────────────────────────────────────────────────────────

    /// **T2-1/2**：`FillSolid` ⇒ 实心箭头；`StrokeSolid` ⇒ 空心（杆内部无像素）。
    #[test]
    fn t2_arrow_fill_and_stroke_both_have_pixels() {
        let arrow = Arrow::new((-1.0, 0.0), (1.0, 0.0), 0.5, 0.6);
        let mut filled = hecs::World::new();
        filled.spawn((arrow, white_fill()));
        let mut stroked = hecs::World::new();
        stroked.spawn((arrow, StrokeSolid::new(2.0, (255, 255, 255, 255))));

        let px_fill = render_visible(&filled);
        let px_stroke = render_visible(&stroked);
        let (n_fill, n_stroke) = (count_lit(&px_fill), count_lit(&px_stroke));

        // 杆内部采样点：世界 (-0.25, 0) ⇒ 屏幕 (75, 60)（杆半宽 3px，描边 2px）。
        let at = |px: &[u8], x: u32, y: u32| {
            let i = ((y * W + x) * 4) as usize;
            is_lit(&px[i..i + 4])
        };
        eprintln!(
            "[T2-GPU] 箭头：实心 {n_fill} 像素 / 空心 {n_stroke} 像素；\
             杆内部 (75,60) 实心={} 空心={}",
            at(&px_fill, 75, 60),
            at(&px_stroke, 75, 60)
        );
        assert!(n_fill > 0, "实心箭头必须有像素");
        assert!(n_stroke > 0, "空心箭头必须有像素");
        assert!(at(&px_fill, 75, 60), "实心箭头的杆内部必须是实心");
        assert!(
            !at(&px_stroke, 75, 60),
            "空心箭头的杆内部必须是空的（只有轮廓）"
        );
    }

    /// **T2-3**：方向可证伪 —— 头底（最宽列）落在 `to` 一侧。
    #[test]
    fn t2_arrow_head_is_at_to_end() {
        let mut fwd = hecs::World::new();
        fwd.spawn((Arrow::new((-1.0, 0.0), (1.0, 0.0), 0.5, 0.6), white_fill()));
        let mut rev = hecs::World::new();
        rev.spawn((Arrow::new((1.0, 0.0), (-1.0, 0.0), 0.5, 0.6), white_fill()));

        let (x_fwd, x_rev) = (
            widest_column_x(&render_visible(&fwd)),
            widest_column_x(&render_visible(&rev)),
        );
        eprintln!("[T2-GPU] 箭头方向：最宽列 forward={x_fwd} reversed={x_rev}");
        // 头底 = to - head_length：正向 x=0.5（屏幕 90），反向 x=-0.5（屏幕 70）。
        assert!(x_fwd > 85, "正向箭头最宽列应在右侧（to 侧），实际 {x_fwd}");
        assert!(x_rev < 75, "反向箭头最宽列应在左侧（to 侧），实际 {x_rev}");
    }

    /// **T2-5**：新图元同样受 N 线的 `Opacity` / `Group` 通道控制。
    #[test]
    fn t2_arrow_honors_opacity_and_group() {
        let arrow = Arrow::new((-1.0, 0.0), (1.0, 0.0), 0.5, 0.6);
        let g = GroupId::new(7);

        let mut world = hecs::World::new();
        world.spawn((arrow, white_fill()));
        let visible = count_lit(&render_visible(&world));
        assert!(visible > 0, "基准箭头必须有像素");

        // Opacity = 0 ⇒ 完全消失。
        let mut transparent = hecs::World::new();
        transparent.spawn((arrow, white_fill(), Opacity::new(0.0)));
        let n_opacity = count_lit(&render_visible(&transparent));
        assert_eq!(n_opacity, 0, "Opacity = 0 的箭头像素必须归零");

        // 组开关隐藏 ⇒ 完全消失。
        let mut grouped = hecs::World::new();
        grouped.spawn((arrow, white_fill(), Visibility::of_group(g)));
        let mut hidden = GroupVisibility::new();
        hidden.hide(g);
        let n_hidden = count_lit(&render(&grouped, &hidden));
        eprintln!("[T2-GPU] 箭头通道：可见 {visible} / opacity0 {n_opacity} / 隐藏组 {n_hidden}");
        assert_eq!(n_hidden, 0, "隐藏组后箭头像素必须归零");
    }

    // ─── T3 ─────────────────────────────────────────────────────────────

    /// **T3-2**：文字区域有像素（占位引擎的轮廓矩形；真实字形由 `cargo run` 目视）。
    #[test]
    fn t3_text_has_pixels() {
        let engine = xc_prerender::placeholder_engine();
        let text = crate::formula::TextSpec::new("abc", 1.0, glam::Vec2::ZERO);
        let spec = text.to_formula_spec();
        let ir = crate::formula::rebuild(&engine, &spec).expect("placeholder IR");
        let paths = crate::formula::compile_to_stroke_paths(&ir, &spec);
        assert!(!paths.is_empty(), "文本应编译出描边路径");

        let mut world = hecs::World::new();
        for (path, color, width, space) in paths {
            world.spawn((
                xc_object::Path::new(path),
                StrokeSolid::new_in(width, color, space),
            ));
        }
        let lit = count_lit(&render_visible(&world));
        eprintln!("[T3-GPU] 文字区域亮像素 = {lit}");
        assert!(lit > 0, "文字区域必须有像素");
    }
}
