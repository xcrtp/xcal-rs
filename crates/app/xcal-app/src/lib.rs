// xcal-app: 组装 + 启动层
//
// App::new 负责：
//   - 构建 Scene（3D demo + 首个数学动画原型的分镜脚本）
//   - 构建 Resources、挂载各个 Resource 类型
//   - 构建 Scheduler、按 Phase 注册 System
//   - 实现 eframe::App trait（ui 每帧调度）
//
// P5（本轮内容目标）：**首个数学动画原型**的六条分镜在
// [`storyboard::build`] 里被**声明**出来（而不是手工 spawn 一堆实体 + 手写 offset）。

use std::sync::Arc;
use std::time::Instant;

use xc_notion::{Rect2D, SurfaceFn};
use xc_object::{
    CONTENT_TEXTURE_HEIGHT, CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH, Camera2D, Camera2DHandle,
    Camera3D, Camera3DHandle, Framing2D, Projection, Transform,
};
use xcal_egui::resources::UiConfig;
use xcal_egui::systems::{AxesRefreshSystem, FunctionGraphRefreshSystem};
use xcal_egui::{EguiSceneHost, TimeControlSystem};
use xcal_render::components::{Path, StrokeSolid};
use xcal_render::{
    ContentTextureQueue, ContentTextureRecordSystem, DrawQueue, DrawQueue3D, MorphQueue,
    MorphRecordSystem, RevealQueue, RevealRecordSystem, SceneRecordSystem, ViewRect, scene::Scene,
};
use xcal_render_backends::RenderContext;
pub use xcal_scene::Result;
use xcal_scene::animation::CameraEye;
use xcal_scene::animation::{Binding, BindingApplySystem};
use xcal_scene::camera_input::CameraInputSystem;
use xcal_scene::contract::{GroupVisibility, SceneRead, object_id_of};
use xcal_scene::director_state::{DirectorAdvanceSystem, DirectorState};
use xcal_scene::domain::{CameraMode, ContentTextureSpec, InputEvents};
use xcal_scene::errors::Error;
use xcal_scene::scene::components::{ContentTextureItem, Material3D, TextureHandle};
use xcal_scene::scene::primitives::{parametric_surface, wireframe_cube};
use xcal_scene::types::Color32;
use xcal_scene::{
    ClockNode, Curve, DeferredApplySystem, DeferredCommands, DynamicObjects, ReplaySystem,
    SeekKind, TimeState,
};
use xcal_schedule::resource::Resources;
use xcal_schedule::scheduler::{Phase, Scheduler};

// 内容纹理（S3-3）的 id 与尺寸已归位到对象层默认配置：
// `xc_object::{CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH, CONTENT_TEXTURE_HEIGHT}`。

mod assembly;
mod camera_drive;
mod capability_demo;
pub mod export;
mod formula;
mod health;
mod param_panel;
mod storyboard;
mod systems_3d;
use camera_drive::CameraDriveSystem;
use health::HealthOverlay;
use param_panel::ParamPanelSystem;
use systems_3d::SceneRecord3DSystem;

/// **2D 场景选择**（启动参数切换，见 `main.rs`）。
///
/// 为什么是启动参数而不是 UI 开关：场景在 `App::new` 里**一次性装配**（spawn 决定
/// `ObjectId`、时钟树与绑定表在装配期编译），运行态重建整张场景图需要尚未具备的
/// 生命周期设施（见「运行时创建/销毁对象」的记账）。故本轮用**启动参数**切换：
///
/// - `SceneChoice::Storyboard`（默认）：§1.1 六条分镜（`storyboard.rs`）；
/// - `SceneChoice::CapabilityDemo`：N 线能力测试 demo（`capability_demo.rs`）——
///   最小场景，四项能力（Name/Group/Visibility/Opacity）各落一处；
/// - `SceneChoice::ScriptStoryboard`：`--script` —— 镜头 2–5 由 **Lua 脚本**编排
///   （`xc-pod-lua`，装配期产出 `SceneScript`），与手写版等价；
/// - `SceneChoice::PythonStoryboard`：`--python` —— 同一分镜由 **Python 脚本**编排
///   （`xc-pod-py`，默认装配的一部分），与手写版 / Lua 版等价。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SceneChoice {
    #[default]
    Storyboard,
    CapabilityDemo,
    ScriptStoryboard,
    PythonStoryboard,
}

pub struct App {
    scheduler: Scheduler,
    render_ctx: RenderContext,
    health: HealthOverlay,
    /// 墙钟时间基准：每帧用 Instant::now() - last_frame 算 dt，
    /// 不依赖渲染帧间隔（帧率波动、卡顿都不影响 Director 时间流速）。
    last_frame: Instant,
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

/// **唯一的 System 注册点**（`App::new` 与回归测试共用）。
///
/// 为什么抽出来：`Scheduler::add_system` 在 debug 下做**静态借用冲突检查**并直接
/// `panic!`（同 phase 内 write/write 与 read/write 冲突）。这类冲突只在真正的
/// 注册序列上暴露 —— 单测分别装单个 System 是**测不出来**的（实测踩过：
/// 当时的 3D 模式 System（今 `CameraDriveSystem` 的前身）与 `ParamPanelSystem` 同处
/// `LogicUpdate` 且都写 `BindingTable` ⇒ app 启动即 panic）。抽成函数后由
/// `full_system_graph_assembles_without_borrow_conflicts` 覆盖整张图。
///
/// Phase 分配（Director 是唯一时间源）：
/// - `Input`：DirectorAdvance（W=DirectorState/TimeState）、
///   **CameraDrive（R=UiConfig/Camera2DHandle/Camera3DHandle，W=BindingTable）**、
///   CameraInput（R=InputEvents/Camera*Handle，写**场景对象**组件）；
///   相机驱动必须在 `CameraInput` **之前** ⇒ 同帧生效。
/// - `LogicUpdate`：TimeControl（R=InputEvents，W=DirectorState）、
///   ParamPanel（R=InputEvents，W=BindingTable）—— 故相机驱动**不能**放这里。
/// - `SceneSync`：BindingApply（R=BindingTable）。
/// - `Prepare`：AxesRefresh + FunctionGraphRefresh（几何在场景对象层）。
/// - `RenderCommit`：SceneRecord / ContentTextureRecord / MorphRecord（xcal-render）
///   + SceneRecord3D（app）。
///
/// `CameraDriveSystem` 是**唯一**的 `BindingTable` 写者之一（另一个是 ParamPanel），
/// 且它同时托管两台相机的让位 —— 因为两台相机让位的是同一张表，必须由**一个**
/// System 原子完成（同相位两个写者会被 `add_system` 的静态检查拦下）。
fn register_systems(
    scheduler: &mut Scheduler,
    cam2d_bindings: Vec<Binding>,
    orbit_binding: Binding,
) {
    scheduler.add_system(Phase::Input, DirectorAdvanceSystem);
    // K3 + 2D 自由开关：按「2D 相机自由」与「视口操作目标」决定两台相机由分镜还是交互
    // 驱动，并让对应绑定让位/复位。放在 `CameraInputSystem` 之前：同帧内先改模式，
    // 交互才开始按新模式解释。
    scheduler.add_system(
        Phase::Input,
        CameraDriveSystem::new(cam2d_bindings, orbit_binding),
    );
    // P1：相机输入由场景层消费（相机状态归场景对象，UI 只发命令）。
    scheduler.add_system(Phase::Input, CameraInputSystem);

    scheduler.add_system(Phase::LogicUpdate, TimeControlSystem);
    scheduler.add_system(Phase::LogicUpdate, ParamPanelSystem);

    scheduler.add_system(Phase::SceneSync, BindingApplySystem);
    // S4.3：形变改由 GPU 插值（`from`/`to` 常量化 + `t` 走 uniform），
    // `MorphApplySystem`（CPU 插值写 `Path`）不再注册——形变实体已不带 `Path`
    // 组件，故它即使注册也不会命中。CPU 路径保留在 xcal-scene 备查。

    // M4：坐标轴 / 网格与函数图像的几何在场景对象层（`Axes2D` / `FunctionGraph`），
    // 这两个 System 只做「取相机 / 取相位 → 刷新对象」的接线。
    scheduler.add_system(Phase::Prepare, AxesRefreshSystem);
    scheduler.add_system(Phase::Prepare, FunctionGraphRefreshSystem);

    // P4：三个录制 System 已归位到 `xcal-render`（渲染动作住渲染层，
    // 不再读 UI 定义的 `ViewRect` / 已删除的 `AppCamera`）。
    scheduler.add_system(Phase::RenderCommit, SceneRecordSystem);
    scheduler.add_system(Phase::RenderCommit, ContentTextureRecordSystem);
    scheduler.add_system(Phase::RenderCommit, MorphRecordSystem);
    // R0：书写动画录制（同相位第四个写者，写集 `RevealQueue` 与前三者不相交）。
    scheduler.add_system(Phase::RenderCommit, RevealRecordSystem);
    scheduler.add_system(Phase::RenderCommit, SceneRecord3DSystem);

    // P6/B3：延迟命令应用 —— `Phase::FrameEnd` 此前为空（架构文档记"有意留空"），
    // 正好用作"帧末统一应用延迟命令"（避免录制遍历中删对象）。
    // 写集 `(DeferredCommands, DynamicObjects)` 与任何相位都不相交。
    scheduler.add_system(Phase::FrameEnd, DeferredApplySystem);
    // P6/B4：重播清理 —— 运行时创建的对象在重播时销毁（`Prepare`，在录制之前；
    // 该相位当前无其它 Resource 写者）。
    scheduler.add_system(Phase::Prepare, ReplaySystem);
}

impl App {
    /// 默认场景（§1.1 分镜）。保留此入口是为了不改既有调用点（`main.rs` / 测试）。
    pub fn new(ctx: &eframe::CreationContext) -> Result<Self> {
        Self::new_with_scene(ctx, SceneChoice::Storyboard)
    }

    /// 按 [`SceneChoice`] 装配 App（**唯一的装配入口**）。
    // 体检第 9 项（观察，暂不改）：本函数约 311 行，后续可按
    // 「字体/主题 → 渲染后端 → 场景系统 → 面板」四段抽成私有方法。
    pub fn new_with_scene(ctx: &eframe::CreationContext, choice: SceneChoice) -> Result<Self> {
        // 宿主字体（平台相关）：候选路径 + `XCAL_CJK_FONT`；找不到只提示、不中止。
        if EguiSceneHost::install_cjk_font(&ctx.egui_ctx).is_none() {
            eprintln!(
                "警告：未找到可用的中文字体（可用环境变量 XCAL_CJK_FONT 指定字体路径），中文可能显示为方框。"
            );
        }
        Self::setup_visuals(&ctx.egui_ctx);

        // 渲染环境（后端 + 合成器 + RenderContext）由 backends 侧的"标准安装"负责：
        // app 不需要知道要装哪几个后端、以什么顺序装。
        let render_ctx = {
            let rs = ctx
                .wgpu_render_state
                .clone()
                .ok_or(Error::InitializationError(
                    "Wgpu render state not available".to_string(),
                ))?;
            xcal_render_backends::install_standard_backends(&rs)
        };

        let mut scene = Scene::new();

        // ── 3D 场景：白色线框立方体 ──────────────────────────────
        // 几何以 `Arc` 持有：SceneRecord3DSystem 直接复用引用（Arc::clone），
        // 避免「先深拷贝再包 Arc」的每帧无效开销。
        scene.spawn((
            Arc::new(wireframe_cube(1.0)),
            Material3D::color(Color32::WHITE),
        ));

        // ── 3D 场景：参数曲面 z = 0.35·sin(3x)·cos(3y)，域 [-1.5, 1.5]² ──
        // 曲面自带真实法线，经 cube.wgsl 的头顶 Lambert 呈现明暗起伏（非纯色剪影）。
        // 平移到立方体侧下方（部分重叠），以体现深度测试决定的前后遮挡。
        let surface = parametric_surface(
            SurfaceFn::new((-1.5, 1.5), (-1.5, 1.5), |x, y| {
                0.35 * (3.0 * x).sin() * (3.0 * y).cos()
            }),
            48,
            48,
        );
        // S3-3：曲面材质引用内容纹理（同一 TextureHandle）。
        // 着色式为**墨迹 over 已受光材质色**（见 `cube.wgsl` F2）：
        // `rgb = tex.rgb + color.rgb * lit * (1 - tex.a)`。内容纹理透明底 ⇒ 未覆盖像素
        // 退化为 `材质色 × lit`（蓝色保留明暗起伏）；墨迹恒为纯白、不被光照压暗。
        let mut surface_material = Material3D::color(Color32::new(120, 200, 255, 255));
        surface_material.texture = Some(TextureHandle::new(CONTENT_TEXTURE_ID));
        scene.spawn((
            Arc::new(surface),
            surface_material,
            Transform {
                position: glam::Vec3::new(0.0, -1.35, 0.0),
                ..Default::default()
            },
        ));

        // 3D 相机（eye 由 BindingApplySystem 驱动，旧 OrbitAnim 组件已移除）。
        //
        // target（注视点）取"立方体 + 曲面"合并包围盒的中心，而非世界原点：曲面整体
        // 平移到 y = -1.35（见上），其几何 y ∈ [-2.85, 0.15]，与立方体 y ∈ [-1, 1] 合并后
        // 的中心是 y ≈ -0.9。若仍注视原点，视锥会整体上移，曲面下缘（y = -2.85）会贴着
        // 视口最底部。注视合并中心即可让两件物体在竖直方向居中。
        //
        // 注：eye 由 Orbit 绑定直接写入（半径 5、绕原点、eye.y ≡ 0），改变 target 只影响
        // 朝向，不影响该旋转轨道。
        const SCENE_LOOK_AT: glam::Vec3 = glam::Vec3::new(0.0, -0.9, 0.0);
        // K0/K2：3D 相机的交互模式是**组件**；这里取 `Path`（动画驱动）——
        // 与改动前逐位一致（`eye` 由下面的 `Curve::Orbit` 绑定驱动、不接受交互命令）。
        // `CameraDriveSystem` 会在用户把"视口操作"切到 3D 时把它改成 `Orbit` / `Fps`，
        // 并让自动轨道绑定**让位**（否则每帧覆盖 `eye`，交互看不见效果）。
        let camera3d_entity = scene.spawn((
            Camera3D::new(
                glam::Vec3::new(0.0, 0.0, 5.0),
                SCENE_LOOK_AT,
                glam::Vec3::Y,
                Projection::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0),
            ),
            CameraMode::PATH,
        ));
        let camera3d = Camera3DHandle(object_id_of(camera3d_entity));

        // ── 公式接入（内容纹理源）：Typst 内容管线 → 2D 描边路径 ──────────
        // 静态一条公式，启动时烘焙一次并 spawn 实体；它同时是 3D 曲面材质的纹理源
        // （带 `ContentTextureItem` 标记）。零 System 改动：公式作为普通
        // `(Path, StrokeSolid)` 对象参与既有录制。
        // 排版引擎在 App 侧建一次：内容纹理、硬编码分镜与脚本内容服务共用。
        let engine = Arc::new(formula::build_engine());
        let spec = formula::FormulaSpec::new("x^2 + 1", 1.3, glam::Vec2::new(-2.8, 3.0));
        // 内容纹理相机（S3-3）：由公式的世界包围盒导出——`rect` 即纹理矩形
        // （0,0–512,512），让包围盒适配它（margin 0.8 ⇒ 公式宽度 ≈ 纹理宽 × 0.8）。
        let texture_rect = Rect2D::new(
            xc_notion::Vec2::ZERO,
            xc_notion::Vec2::new(CONTENT_TEXTURE_WIDTH as f32, CONTENT_TEXTURE_HEIGHT as f32),
        );
        let mut content_camera: Option<Camera2D> = None;
        match formula::rebuild(&engine, &spec) {
            Some(ir) => {
                // 包围盒由理念层 `Path2D::bounds`（懒计算）给出 —— app 不再遍历命令。
                let mut bounds: Option<Rect2D> = None;
                for (mut path, color, width, space) in formula::compile_to_stroke_paths(&ir, &spec)
                {
                    bounds = Some(match bounds {
                        Some(acc) => acc.union(&path.bounds()),
                        None => path.bounds(),
                    });
                    scene.spawn((
                        Path::new(path),
                        // 公式描边是世界空间：与字形一同随缩放成比例变化。
                        StrokeSolid::new_in(width, color, space),
                        // S3-3：标记该实体进内容纹理（D5：只录带标记的实体）。
                        ContentTextureItem,
                    ));
                }
                if let Some(bbox) = bounds {
                    // `fit_bounds` 自带退化保护（空盒 / 零宽 ⇒ 保持基底 scale）；
                    // 基底取 40，与原实现里退化分支的兜底值同值。
                    let base = Camera2D::new(xc_notion::Vec2::ZERO, 40.0, texture_rect);
                    content_camera = Some(base.fit_bounds(bbox, 0.8));
                }
            }
            None => eprintln!("[xcal-app] 公式编译失败，跳过公式实体"),
        }

        // ── 2D 场景：分镜 / 能力 demo（启动参数切换，见 `SceneChoice`）────────
        // 两个场景产出**同形的三样东西**：时钟树 + 绑定表 + 主 2D 相机句柄；
        // demo 额外产出一个"待插入的组级开关"（N2 的组控制）。
        let (forest2d, bindings2d, camera2d_handle, group_visibility) = match choice {
            SceneChoice::Storyboard => {
                // 分镜脚本在 `storyboard.rs`：声明式地表达"先…再…"，编译成
                // `ClockNode + Binding`（求值层零改动）。
                let sb = storyboard::build(&mut scene, &engine);
                (
                    sb.forest,
                    sb.bindings,
                    sb.camera,
                    GroupVisibility::default(),
                )
            }
            SceneChoice::CapabilityDemo => {
                // N 线能力测试 demo：最小场景（两条曲线 + 一个对照组）。
                // 默认**不隐藏任何组**（全可见）⇒ 看得见 `cos` 淡出（N4）；
                // 组控制（N2）由 `capability_demo` 的测试与 GPU 用例证明。
                eprintln!(
                    "[xcal-app] 场景 = 能力 demo（N 线：Name / Group / Visibility / Opacity）"
                );
                let d = capability_demo::build(&mut scene);
                (d.forest, d.bindings, d.camera, d.group_visibility)
            }
            SceneChoice::ScriptStoryboard => {
                // 全部 6 个镜头由 **Lua 脚本**编排（装配期一次）。App 显式声明
                // 节点（scene / content / lua），Broker 负责校验、排序与注入。
                // 与硬编码版的等价性由 `xcal-app` 的 `script_storyboard_matches_hardcoded` 锁定。
                let assembly = assembly::assemble(Arc::clone(&engine))
                    .map_err(|e| Error::InitializationError(format!("脚本 Pod 装配失败：{e}")))?;
                let script = assembly
                    .run_script(assembly::STORYBOARD_LUA)
                    .map_err(|e| Error::InitializationError(format!("Lua 分镜执行失败：{e}")))?;
                eprintln!(
                    "[xcal-app] 场景 = Lua 脚本分镜（{} 个对象，{}s）",
                    script.len(),
                    script.duration()
                );
                let recipe = script
                    .commit(&mut scene)
                    .map_err(|e| Error::InitializationError(format!("脚本 commit 失败：{e}")))?;
                // **不能假设第 0 个句柄是相机**：脚本先建坐标轴容器。
                let camera = find_camera2d(&scene)?;
                (
                    recipe.forest,
                    recipe.bindings,
                    camera,
                    GroupVisibility::default(),
                )
            }
            SceneChoice::PythonStoryboard => {
                #[cfg(not(feature = "python"))]
                {
                    return Err(Error::InitializationError(
                        "Python 适配器未启用（编译时关闭了 python feature）".to_string(),
                    ));
                }
                #[cfg(feature = "python")]
                {
                    // 同一分镜由 **Python 脚本**编排。与硬编码版的等价性由
                    // `python_storyboard_matches_hardcoded` 锁定。
                    let assembly = assembly::assemble(Arc::clone(&engine)).map_err(|e| {
                        Error::InitializationError(format!("脚本 Pod 装配失败：{e}"))
                    })?;
                    let script = assembly
                        .run_python_script(assembly::STORYBOARD_PY)
                        .map_err(|e| {
                            Error::InitializationError(format!("Python 分镜执行失败：{e}"))
                        })?;
                    eprintln!(
                        "[xcal-app] 场景 = Python 脚本分镜（{} 个对象，{}s）",
                        script.len(),
                        script.duration()
                    );
                    let recipe = script.commit(&mut scene).map_err(|e| {
                        Error::InitializationError(format!("脚本 commit 失败：{e}"))
                    })?;
                    // 与 Lua 版相同：不能假设第 0 个句柄是相机。
                    let camera = find_camera2d(&scene)?;
                    (
                        recipe.forest,
                        recipe.bindings,
                        camera,
                        GroupVisibility::default(),
                    )
                }
            }
        };

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
            target: Arc::new(CameraEye {
                id: object_id_of(camera3d_entity),
            }),
            seek_kind: SeekKind::O1,
        };
        bindings.add(orbit_binding.clone());

        let mut resources = Resources::new();
        // P1-1：相机状态**不再是 Resource** —— 场景里有一个 2D 相机对象，
        // 这里只放它的**句柄**（引用型，P2）；K3 再加 3D 相机句柄。
        resources.insert(camera2d_handle);
        resources.insert(camera3d);
        resources.insert(TimeState::default());
        resources.insert(UiConfig::default());
        resources.insert(ViewRect::default());
        resources.insert(InputEvents::default());
        resources.insert(DrawQueue::new());
        resources.insert(DrawQueue3D::new());
        // S3-3 内容纹理：配置（唯一真相来源）+ 当帧录制结果。
        // 内容相机缺省（公式编译失败）时退回一个"世界原点居中、无内容"的相机。
        let content_camera = content_camera.unwrap_or_else(|| {
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

        let mut scheduler = Scheduler::new(world, resources);
        register_systems(&mut scheduler, cam2d_bindings, orbit_binding);

        Ok(Self {
            scheduler,
            render_ctx,
            health: HealthOverlay::new(),
            last_frame: Instant::now(),
        })
    }

    pub fn render_context(&self) -> RenderContext {
        self.render_ctx.clone()
    }

    /// 调亮 UI 文字颜色：默认 dark theme 的 weak_text_color 偏灰，
    /// 把 hint / inactive 都提亮。
    fn setup_visuals(ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.weak_text_color = Some(egui::Color32::from_rgb(210, 210, 210));
        visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(200, 200, 200);
        ctx.set_visuals(visuals);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 墙钟 dt：用单调时钟算，不随渲染帧率波动。
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // ── B 阶段帧流程（UI 与场景只经 SceneView / SceneEvent 契约交互）──
        //
        // 1. 场景 → UI：构建本帧视图声明（2D/3D 图层 + 相机）。
        //    相机取上一帧末状态——视口事件换算对该滞后不敏感
        //    （缩放因子/平移量只依赖 scale，不依赖 rect）。
        let view =
            EguiSceneHost::build_scene_view(self.scheduler.world(), self.scheduler.resources());

        // 2. UI → 场景：收集输入事件、绘制面板 chrome、写 ViewRect。
        //    必须在 run_frame 之前：System 消费 InputEvents，Axes/FunctionGraph
        //    刷新当帧读 ViewRect。
        EguiSceneHost::begin_frame(ui, &view, self.scheduler.resources_mut());

        self.scheduler.run_frame(dt);

        // 3. 内容提交：2D 队列 + 3D 队列 → painter（原 RenderSubmit 职责）。
        EguiSceneHost::present(ui, &view, self.scheduler.resources_mut());

        // ── 可观测性：F1 切换 health overlay，F10 导出 World 快照 ──
        if ui.input(|i| i.key_pressed(egui::Key::F1)) {
            self.health.toggle();
        }
        if ui.input(|i| i.key_pressed(egui::Key::F10)) {
            let tick = self.scheduler.tick();
            match self.health.dump_snapshot(
                self.scheduler.world(),
                self.scheduler.resources(),
                tick,
            ) {
                Ok(path) => tracing::info!(tick, path = %path.display(), "world snapshot exported"),
                Err(e) => tracing::error!(%e, tick, "snapshot export failed"),
            }
        }
        let draw_calls = EguiSceneHost::draw_call_count(self.scheduler.resources());
        self.health.update(
            dt,
            self.scheduler.frame_report(),
            self.scheduler.world().len() as usize,
            draw_calls,
        );
        self.health.show(ui);

        // 帧末：清空已被 present 消费的 2D 队列（帧管理归 host）。
        EguiSceneHost::end_frame(self.scheduler.resources_mut());

        // egui 默认事件驱动，无鼠标/键盘事件时不调度 ui()，导致
        // TimeState.elapsed 停止增长、动画卡住。动画引擎需持续重绘，每帧请求下一帧。
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_object::{Camera2DHandle, Framing2D};
    use xcal_scene::ClockForest;
    use xcal_scene::animation::{BindingTable, Camera2DFraming, CameraEye, SeekKind};
    use xcal_scene::contract::object_id_of;
    use xcal_scene::domain::CameraMode;
    use xcal_scene::{AnimValue, Interp};

    /// **回归（实测踩过）**：整张 System 图的**注册 + 运行一帧**都不得冲突。
    ///
    /// 坑：3D 模式 System 原先与 `ParamPanelSystem` 同处 `LogicUpdate` 且都写
    /// `BindingTable` ⇒ `Scheduler::add_system` 的**静态借用冲突检查在 app 启动时
    /// panic**（`Scheduler conflict in phase LogicUpdate ...`），而"每个 System 各装
    /// 一次的"单测完全测不出来 —— `App::new` 需要 `eframe::CreationContext`，跑不到。
    ///
    /// 修法：相机驱动 System 移到 `Input`（那里没有别的 `BindingTable` 写者），并把它与
    /// `CameraInputSystem` 的先后固定为「先改模式、后解释输入」。切换开关（2D 自由 / 3D
    /// 视口目标）都会走到这条路径。
    /// 本用例走与 `App::new` **同一个** `register_systems` ⇒ 注册序列变化会被它挡住。
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
        let sb = crate::storyboard::build(&mut scene_a, &engine_a);

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
        let sb = crate::storyboard::build(&mut scene_a, &engine_a);

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
