// xcal-app: 组装 + 启动层
//
// App::new 负责：
//   - 构建 Scene、初始化 World 图元
//   - 构建 Resources、挂载各个 egui Resource 类型
//   - 构建 Scheduler、按 Phase 注册 System（迁移到新阶段分配以避开冲突）
//   - 实现 eframe::App trait（ui 每帧调度）

use std::sync::Arc;
use std::time::Instant;

pub use xcal_core::Result;
use xcal_core::animation::{BindingApplySystem, Binding, BindingTarget, BindingTable, SeekKind};
use xcal_core::director_state::{DirectorAdvanceSystem, DirectorState};
use xcal_core::errors::Error;
use xcal_core::resource::Resources;
use xcal_core::domain::InputEvents;
use xcal_core::scene::components::{Camera3D, Projection};
use xcal_core::scene::primitives::wireframe_cube;
use xcal_core::scheduler::{Phase, Scheduler};
use xcal_core::TimeState;
use xcal_core::types::Color32;
use xcal_core::{ClockForest, ClockNode, Curve};
use xcal_egui::resources::{AppCamera, UiConfig, ViewRect};
use xcal_egui::systems::{CameraInputSystem, GridBuildSystem, SceneRecordSystem};
use xcal_egui::{EguiSceneHost, TimeControlSystem};
use xcal_render::components::{Circle, FunctionX2Y, PhaseAnim, Sampler1D, StrokeSolid};
use xcal_render::{DrawQueue, DrawQueue3D, scene::Scene};
use xcal_render_backends::{RenderContext, WgpuBackend3D};

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
        Self::setup_fonts(&ctx.egui_ctx)?;
        Self::setup_visuals(&ctx.egui_ctx);

        let render_ctx = {
            let rs = ctx
                .wgpu_render_state
                .clone()
                .ok_or(Error::InitializationError(
                    "Wgpu render state not available".to_string(),
                ))?;

            // 3D 后端：pipeline 等 GPU 资源存入 egui-wgpu Renderer 的
            // callback_resources（TypeMap），Layer3DCallback 在 prepare/paint
            // 阶段按需取用（回调本身为 &self，不能持有重建中的 GPU 资源）。
            let backend_3d =
                WgpuBackend3D::with_default_shader(&rs.device, &rs.queue, rs.target_format);
            rs.renderer.write().callback_resources.insert(backend_3d);

            RenderContext::from_render_state(&rs)
        };

        let mut scene = Scene::new();

        // ── 3D 场景：白色线框立方体 ──────────────────────────────
        scene.spawn((wireframe_cube(1.0, Color32::new(255, 255, 255, 255)),));

        // 3D 相机（eye 由 BindingApplySystem 驱动，旧 OrbitAnim 组件已移除）。
        let camera_entity = scene.spawn((
            Camera3D::new(
                glam::Vec3::new(0.0, 0.0, 5.0),
                glam::Vec3::ZERO,
                glam::Vec3::Y,
                Projection::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0),
            ),
        ));

        let circle_entity = scene.spawn((
            Circle::new((0.0, 0.0), 1.0),
            StrokeSolid::new(2.0, (100, 255, 0, 255)),
            // 半径振荡由 BindingApplySystem 驱动（Binding 表：base=1.0, amp=0.4, omega=2.0）
        ));

        let func_entity = scene.spawn((
            FunctionX2Y::new(|x: f32| x.sin()),
            Sampler1D::new(-10.0f32, 10.0, 0.1),
            StrokeSolid::new(2.0, (0, 200, 200, 255)),
            // 相位滑动由 BindingApplySystem 驱动（Binding 表：omega=1.5）
            PhaseAnim::new(0.0, 1.5),
        ));

        let world = scene.into_world();

        // ── 时钟树 + 绑定表（动画唯一驱动链路）────────────────────────
        let mut forest = ClockForest::new();
        let root_circle = forest.insert(ClockNode::new(None)).unwrap();
        let root_phase = forest.insert(ClockNode::new(None)).unwrap();
        let root_orbit = forest.insert(ClockNode::new(None)).unwrap();

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

        let mut resources = Resources::new();
        resources.insert(AppCamera::default());
        resources.insert(TimeState::default());
        resources.insert(UiConfig::default());
        resources.insert(ViewRect::default());
        resources.insert(InputEvents::default());
        resources.insert(DrawQueue::new());
        resources.insert(DrawQueue3D::new());
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
        // - Prepare:      GridBuild
        // - RenderCommit: SceneRecord（2D）+ SceneRecord3D
        // - FrameEnd:     无——内容提交由 host.present 承担（原 RenderSubmit 职责）。
        //                 面板 chrome 与 ViewRect 由 host.begin_frame 承担（run_frame 前）。
        scheduler.add_system(Phase::Input, DirectorAdvanceSystem);
        scheduler.add_system(Phase::Input, CameraInputSystem);

        scheduler.add_system(Phase::LogicUpdate, TimeControlSystem);
        scheduler.add_system(Phase::LogicUpdate, ParamPanelSystem);

        scheduler.add_system(Phase::SceneSync, BindingApplySystem);

        scheduler.add_system(Phase::Prepare, GridBuildSystem);

        // RenderCommit：2D SceneRecord 写 DrawQueue(2D)，
        // 3D SceneRecord3D 写 DrawQueue3D，类型不相交。
        scheduler.add_system(Phase::RenderCommit, SceneRecordSystem);
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

    fn setup_fonts(ctx: &egui::Context) -> Result<()> {
        let mut fonts = egui::FontDefinitions::default();
        let font_path = "C:\\Windows\\Fonts\\msyh.ttc";
        if let Ok(font_data) = std::fs::read(font_path) {
            fonts.font_data.insert(
                "msyh".to_owned(),
                Arc::new(egui::FontData::from_owned(font_data)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "msyh".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("msyh".to_owned());
            ctx.set_fonts(fonts);
        } else {
            eprintln!("警告：未找到微软雅黑字体，中文可能显示为方框。");
        }
        Ok(())
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
        //    必须在 run_frame 之前：System 消费 InputEvents，GridBuild 当帧读 ViewRect。
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
        let draw_calls = self
            .scheduler
            .resources()
            .get::<DrawQueue>()
            .map_or(0, |q| q.len())
            + self
                .scheduler
                .resources()
                .get::<DrawQueue3D>()
                .map_or(0, |q| q.mesh_count() + q.line_count());
        self.health.update(
            dt,
            self.scheduler.frame_report(),
            self.scheduler.world().len() as usize,
            draw_calls,
        );
        self.health.show(ui);

        // 每帧清空 2D 队列（present 已消费其内容；SceneRecord 下帧重新录制）。
        if let Some(queue) = self.scheduler.resources_mut().get_mut::<DrawQueue>() {
            queue.clear();
        }

        // egui 默认事件驱动，无鼠标/键盘事件时不调度 ui()，导致
        // TimeState.elapsed 停止增长、动画卡住。动画引擎需持续重绘，每帧请求下一帧。
        ui.ctx().request_repaint();
    }
}
