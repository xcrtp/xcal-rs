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

pub use xcal_scene::Result;
use xcal_egui::resources::UiConfig;
use xcal_egui::systems::{AxesRefreshSystem, FunctionGraphRefreshSystem};
use xcal_egui::{EguiSceneHost, TimeControlSystem};
use xcal_render::components::{Path, StrokeSolid};
use xcal_render::{
    ContentTextureQueue, ContentTextureRecordSystem, DrawQueue, DrawQueue3D, MorphQueue,
    MorphRecordSystem, RevealQueue, RevealRecordSystem, SceneRecordSystem, ViewRect, scene::Scene,
};
use xcal_render_backends::RenderContext;
use xcal_scene::animation::CameraEye;
use xcal_scene::animation::{Binding, BindingApplySystem, TargetKind};
use xcal_scene::camera_input::CameraInputSystem;
use xcal_scene::contract::{GroupVisibility, object_id_of};
use xcal_scene::director_state::{DirectorAdvanceSystem, DirectorState};
use xcal_scene::domain::{CameraMode, ContentTextureSpec, InputEvents};
use xcal_scene::errors::Error;
use xcal_scene::scene::components::{ContentTextureItem, Material3D, TextureHandle};
use xcal_scene::scene::primitives::{parametric_surface, wireframe_cube};
use xcal_scene::{ClockNode, Curve, SeekKind, TimeState};
use xcal_schedule::resource::Resources;
use xcal_schedule::scheduler::{Phase, Scheduler};
use xcal_scene::types::Color32;
use xc_notion::{Rect2D, SurfaceFn};
use xc_object::{
    CONTENT_TEXTURE_HEIGHT, CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH, Camera2D, Camera3D,
    Camera3DHandle, Projection, Transform,
};

// 内容纹理（S3-3）的 id 与尺寸已归位到对象层默认配置：
// `xc_object::{CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH, CONTENT_TEXTURE_HEIGHT}`。

mod camera_drive;
mod formula;
mod health;
mod param_panel;
mod storyboard;
mod systems_3d;
use camera_drive::CameraDriveSystem;
use health::HealthOverlay;
use param_panel::ParamPanelSystem;
use systems_3d::SceneRecord3DSystem;

pub struct App {
    scheduler: Scheduler,
    render_ctx: RenderContext,
    health: HealthOverlay,
    /// 墙钟时间基准：每帧用 Instant::now() - last_frame 算 dt，
    /// 不依赖渲染帧间隔（帧率波动、卡顿都不影响 Director 时间流速）。
    last_frame: Instant,
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
}

impl App {
    pub fn new(ctx: &eframe::CreationContext) -> Result<Self> {
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
        let engine = formula::build_engine();
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

        // ── 首个数学动画原型（§1.1 六条分镜）─────────────────────────────
        // 分镜脚本在 `storyboard.rs`：声明式地表达"先…再…"，编译成 `ClockNode + Binding`
        // （求值层零改动）。返回：时钟树 + 绑定表 + 主 2D 相机句柄。
        let sb = storyboard::build(&mut scene, &engine);

        let world = scene.into_world();

        // ── 时钟树 + 绑定表（动画唯一驱动链路）────────────────────────
        // 分镜已把时间编排编译进 `sb.forest` / `sb.bindings`；下面只补一条与
        // 分镜编排无关的手工绑定：3D 轨道相机（时间源 = 全局 t）。
        //
        // K3：这条绑定由 `CameraDriveSystem` **托管**——"视口操作"切到 3D 时
        // 它会从表里移出（让位给交互）、切回 2D 时放回。
        let mut forest = sb.forest;
        let mut bindings = sb.bindings;
        // 「2D 相机自由」开关：分镜里指向 2D 相机（`Camera2DFraming`）的绑定
        // （`center` + `scale` 各一条）也被 `CameraDriveSystem` 托管——开关打开时从表里
        // 移出（否则每帧覆盖用户的 Pan/Zoom）、关闭时放回（分镜驱动）。
        let cam2d_bindings: Vec<Binding> = bindings
            .bindings
            .iter()
            .filter(|b| b.target.kind() == TargetKind::Camera2DFraming)
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
        resources.insert(sb.camera);
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
        resources.insert(GroupVisibility::default());

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
    use xcal_scene::animation::{BindingTable, Camera2DFraming, CameraEye, SeekKind};
    use xcal_scene::contract::object_id_of;
    use xcal_scene::domain::CameraMode;
    use xcal_scene::ClockForest;
    use xcal_scene::{AnimValue, Interp};
    use xc_object::{Camera2DHandle, Framing2D};

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
}
