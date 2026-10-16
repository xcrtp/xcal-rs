// xcal-app: 组装 + 启动层
//
// App::new 负责：
//   - 构建 Scene、初始化 World 图元
//   - 构建 Resources、挂载各个 egui Resource 类型
//   - 构建 Scheduler、按 Phase 注册 System（迁移到新阶段分配以避开冲突）
//   - 实现 eframe::App trait（ui 每帧调度）

use std::sync::Arc;
use std::time::Instant;

pub use xcal_scene::Result;
use xcal_scene::animation::{
    Binding, BindingApplySystem, BindingTable, BindingTarget, SeekKind,
};
use xcal_scene::director_state::{DirectorAdvanceSystem, DirectorState};
use xcal_scene::errors::Error;
use xcal_schedule::resource::Resources;
use xcal_scene::domain::{ContentTextureSpec, InputEvents};
use xcal_scene::scene::components::{
    ContentTextureItem, Material3D, MorphShape, TextureHandle,
};
use xcal_scene::scene::primitives::{parametric_surface, wireframe_cube};
use xcal_schedule::scheduler::{Phase, Scheduler};
use xcal_scene::TimeState;
use xcal_scene::types::Color32;
use xcal_scene::{ClockForest, ClockNode, Curve, PlayMode};
use xcal_egui::resources::{AppCamera, UiConfig, ViewRect};
use xcal_egui::systems::{
    AxesRefreshSystem, CameraInputSystem, ContentTextureRecordSystem, FunctionGraphRefreshSystem,
    MorphRecordSystem, SceneRecordSystem,
};
use xcal_egui::{EguiSceneHost, TimeControlSystem};
use xcal_render::components::{Circle, FunctionX2Y, Path, PhaseAnim, Sampler1D, StrokeSolid};
use xcal_render::{
    ContentTextureQueue, DrawQueue, DrawQueue3D, MeasureSpace, MorphQueue, scene::Scene,
};
use xcal_render_backends::RenderContext;
use xc_notion::{Path2D, Rect2D, SurfaceFn};
use xc_object::{
    Axes2D, CONTENT_TEXTURE_HEIGHT, CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH, Camera2D, Camera3D,
    FunctionGraph, Projection, Transform,
};

// 内容纹理（S3-3）的 id 与尺寸已归位到对象层默认配置：
// `xc_object::{CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH, CONTENT_TEXTURE_HEIGHT}`。

mod formula;
mod health;
mod param_panel;
mod systems_3d;
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
        let camera_entity = scene.spawn((
            Camera3D::new(
                glam::Vec3::new(0.0, 0.0, 5.0),
                SCENE_LOOK_AT,
                glam::Vec3::Y,
                Projection::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0),
            ),
        ));

        let circle_entity = scene.spawn((
            Circle::new((0.0, 0.0), 1.0),
            // 屏幕空间：圆粗细是"观测工具"，不随缩放变化。
            StrokeSolid::new_in(2.0, (100, 255, 0, 255), MeasureSpace::Screen),
            // 半径振荡由 BindingApplySystem 驱动（Binding 表：base=1.0, amp=0.4, omega=2.0）
        ));

        // ── 坐标轴 / 网格（M4-2）：规格与装配都在场景对象层 `Axes2D` ──────
        // 原先由 UI 层网格构建 System（M4 已退役）每帧直写 DrawQueue 的几何
        // （相机裁剪 / 步长自适应 / 刻度线 / 坐标轴）已整体归位；
        // 这里只装配一次，逐帧刷新由 `AxesRefreshSystem` 接线。
        let axes = Axes2D::unbounded();
        let _axes_handle = axes.build(&mut scene).expect("Axes2D 已登记");

        // ── 函数图像（M4-1）：采样归对象层 `FunctionGraph` ────────────────
        // 相位仍由 BindingApplySystem 写 `PhaseAnim`；`FunctionGraphRefreshSystem`
        // 读该相位并按对象自己的采样规格重采样（写路径 = 契约层 `Scene::set`）。
        let graph = FunctionGraph::new(
            FunctionX2Y::new(|x: f32| x.sin()),
            Sampler1D::new(-10.0f32, 10.0, 0.1),
        );
        let func_handle = graph
            .build(
                &mut scene,
                StrokeSolid::new_in(2.0, (0, 200, 200, 255), MeasureSpace::Screen),
            )
            .expect("FunctionGraph 已登记");
        let func_entity = xcal_scene::contract::entity_of(func_handle).expect("刚创建的句柄有效");

        // ── 公式接入：Typst 内容管线 → 旧 2D 描边路径（空心字）──────────
        // 静态一条公式，启动时烘焙一次并 spawn 实体；rebuild() 已留口，
        // 将来接交互只需加 UI 工作量。零 System 改动：公式作为普通
        // (Path, StrokeSolid) 实体参与现有 record_scene 录制。
        // 位置选在上方空白区域，避开 sin 曲线 / 圆 / 网格轴线；
        // 字号按"窄窗口也能完整看到"取值（默认视口 x∈[-10,10]，但面板会压缩视口）。
        let engine = formula::build_engine();
        let spec = formula::FormulaSpec::new("x^2 + 1", 1.3, glam::Vec2::new(-2.8, 3.0));
        // 内容纹理相机（S3-3）：由公式的世界包围盒导出——`rect` 即纹理矩形
        // （0,0–512,512），让包围盒适配它（margin 0.8 ⇒ 公式宽度 ≈ 纹理宽 × 0.8），
        // `center` 取包围盒中心。这样录进纹理的点恰好落在 [0,512]×[0,512] 内
        // （见 xcal-egui 单测 `content_record_maps_into_texture_rect`）。
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

        // ── S1 形变 demo：圆 → 方（弧长对齐的 Path 形变）────────────────
        // S4.3：形变**上 GPU** —— 实体只带 `MorphShape`（`from`/`to` 常量）+ `StrokeSolid`，
        // `t` 由 Binding 逐帧写入；`MorphRecordSystem` 把它录成 `MorphItem`，
        // 由 2D 后端的形变管线做 `mix(from, to, t)`（每帧只写一个 `t` uniform）。
        // 因此实体**不再带 `Path` 组件**（即不再走 CPU 插值 + 逐帧重三角化）。
        let morph_center = glam::Vec2::new(-6.0, -2.5);
        let morph_radius = 1.2;
        // 起点对齐（原型约定，见 S1 计划已知限制"旋转对齐"）：圆的起笔方向与
        // 方的首个顶点方向一致（均指向 -45° 的右下角），避免形变过程中"绕圈"旋转。
        let start_angle = -std::f32::consts::FRAC_PI_4;
        // 圆采样下沉到理念层 `Circle2D::sample`（原为 app 内的手写三角函数循环）。
        let circle_pts: Vec<glam::Vec2> =
            xc_notion::Circle2D::new(morph_center, morph_radius).sample(64, start_angle);
        let square_pts: Vec<glam::Vec2> = [
            glam::Vec2::new(morph_radius, -morph_radius),
            glam::Vec2::new(morph_radius, morph_radius),
            glam::Vec2::new(-morph_radius, morph_radius),
            glam::Vec2::new(-morph_radius, -morph_radius),
        ]
        .iter()
        .map(|p| morph_center + *p)
        .collect();
        let morph_entity = scene.spawn((
            MorphShape::new(
                Path2D::from_polyline(&circle_pts, true),
                Path2D::from_polyline(&square_pts, true),
            ),
            StrokeSolid::new_in(2.0, (255, 180, 0, 255), MeasureSpace::Screen),
        ));

        // ── S1b 语义形变 demo：x^2 ↔ x^3（按源码锚点配对）──────────────
        // `pair_by_anchor` 保证 `x` 配对到 `x`（而非按位置配到 `2`/`3`）。
        // 每对路径 spawn 一个 MorphShape 实体，由同一个形变时钟同步驱动。
        let anchor_spec = formula::FormulaSpec::new("x^2", 1.0, glam::Vec2::new(1.5, 3.0));
        let mut semantic_morph_entities: Vec<hecs::Entity> = Vec::new();
        let from_ir = formula::rebuild(&engine, &anchor_spec);
        let to_ir = formula::rebuild(
            &engine,
            &formula::FormulaSpec::new("x^3", 1.0, glam::Vec2::new(1.5, 3.0)),
        );
        if let (Some(f), Some(t)) = (from_ir, to_ir) {
            for (from_path, to_path) in formula::compile_morph_pairs(&f, &t, &anchor_spec) {
                semantic_morph_entities.push(scene.spawn((
                    MorphShape::new(from_path, to_path),
                    StrokeSolid::new_in(1.0, (255, 120, 220, 255), MeasureSpace::Screen),
                )));
            }
        }

        let mut world = scene.into_world();

        // 函数图像的相位组件：`FunctionGraphRefreshSystem` 读它、`BindingApplySystem`
        // 写它（`BindingTarget::Phase(func_entity)`）。装配层不认识动画组件，
        // 故它在 `FunctionGraph::build` 之后由 App 侧补上。
        world
            .insert_one(func_entity, PhaseAnim::new(0.0, 1.5))
            .expect("实体刚创建");

        // ── 时钟树 + 绑定表（动画唯一驱动链路）────────────────────────
        let mut forest = ClockForest::new();
        let root_circle = forest.insert(ClockNode::new(None)).unwrap();
        let root_phase = forest.insert(ClockNode::new(None)).unwrap();
        let root_orbit = forest.insert(ClockNode::new(None)).unwrap();
        // 形变时钟：PingPong 在 [0,1] 来回 → t 平滑往返。
        let mut morph_clock = ClockNode::new(None);
        morph_clock.span = Some((0.0, 1.0));
        morph_clock.mode = PlayMode::PingPong;
        let root_morph = forest.insert(morph_clock).unwrap();

        let mut bindings = BindingTable::new();
        // 振动圆半径：radius = base + amp * sin(t * omega)，base=1, amp=0.4, omega=2
        bindings.add(Binding {
            clock: root_circle,
            curve: Curve::Sin { base: 1.0, amp: 0.4, omega: 2.0 },
            target: BindingTarget::CircleRadius(circle_entity),
            seek_kind: SeekKind::O1,
        });
        // sin 曲线相位滑动：phase = t * omega，omega=1.5
        bindings.add(Binding {
            clock: root_phase,
            curve: Curve::Phase { omega: 1.5 },
            target: BindingTarget::Phase(func_entity),
            seek_kind: SeekKind::O1,
        });
        // 3D 轨道相机：eye = radius * rotate_y(speed * t)，radius=5, speed=0.5
        bindings.add(Binding {
            clock: root_orbit,
            curve: Curve::Orbit {
                radius: 5.0,
                axis: glam::Vec3::Y,
                speed: 0.5,
                initial_angle: 0.0,
            },
            target: BindingTarget::CameraEye(camera_entity),
            seek_kind: SeekKind::O1,
        });
        // 形变参数：t = 局部时间 ∈ [0,1]（PingPong 来回）→ MorphShape.t
        bindings.add(Binding {
            clock: root_morph,
            curve: Curve::Phase { omega: 1.0 },
            target: BindingTarget::MorphT(morph_entity),
            seek_kind: SeekKind::O1,
        });
        // S1b 语义形变：每个配对实体一条 MorphT 绑定（共用同一形变时钟）。
        for &entity in &semantic_morph_entities {
            bindings.add(Binding {
                clock: root_morph,
                curve: Curve::Phase { omega: 1.0 },
                target: BindingTarget::MorphT(entity),
                seek_kind: SeekKind::O1,
            });
        }

        let mut resources = Resources::new();
        resources.insert(AppCamera::default());
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
        resources.insert(render_ctx.clone());
        resources.insert(forest);
        resources.insert(bindings);
        resources.insert(DirectorState::new());

        let mut scheduler = Scheduler::new(world, resources);

        // P3：Director 是唯一时间源。B 阶段（UI 解耦）后的 Phase 分配：
        // - Input:        DirectorAdvance（W=DirectorState, TimeState）
        //                 CameraInput（R=InputEvents, W=AppCamera，消费视口事件）
        // - LogicUpdate:  TimeControl（R=InputEvents, W=DirectorState）
        //                 ParamPanel（R=InputEvents, W=BindingTable）
        // - SceneSync:    BindingApply（统一驱动 2D+3D 动画）
        // - Prepare:      AxesRefresh + FunctionGraphRefresh（M4：几何在场景对象层）
        // - RenderCommit: SceneRecord（2D）+ SceneRecord3D
        // - FrameEnd:     无——内容提交由 host.present 承担（原 RenderSubmit 职责）。
        //                 面板 chrome 与 ViewRect 由 host.begin_frame 承担（run_frame 前）。
        scheduler.add_system(Phase::Input, DirectorAdvanceSystem);
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

        // RenderCommit：2D SceneRecord 写 DrawQueue(2D)，
        // 3D SceneRecord3D 写 DrawQueue3D，类型不相交。
        scheduler.add_system(Phase::RenderCommit, SceneRecordSystem);
        // S3-3：内容纹理录制（R=ContentTextureSpec, W=ContentTextureQueue）——
        // 与 SceneRecord 同相，但读/写资源不相交，可同相注册。
        scheduler.add_system(Phase::RenderCommit, ContentTextureRecordSystem);
        // S4.3：形变录制（R=ViewRect/AppCamera, W=MorphQueue）——同样与上面不相交。
        scheduler.add_system(Phase::RenderCommit, MorphRecordSystem);
        scheduler.add_system(Phase::RenderCommit, SceneRecord3DSystem);

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
