// xcal-app: 组装 + 启动层
//
// App::new 负责：
//   - 构建 Scene、初始化 World 图元
//   - 构建 Resources、挂载各个 egui Resource 类型
//   - 构建 Scheduler、按 Phase 注册 System（迁移到新阶段分配以避开冲突）
//   - 实现 eframe::App trait（ui 每帧调度）

use std::sync::Arc;

pub use xcal_core::Result;
use xcal_core::errors::Error;
use xcal_core::resource::Resources;
use xcal_core::scene::components::{Camera3D, OrbitAnim, Projection};
use xcal_core::scene::primitives::wireframe_cube;
use xcal_core::scheduler::{Phase, Scheduler};
use xcal_core::types::Color32;
use xcal_egui::resources::{AppCamera, EguiFrameContext, TimeState, UiConfig, ViewRect};
use xcal_egui::systems::{
    AnimationApplySystem, CameraInputSystem, ControlPanelSystem, GridBuildSystem,
    RenderSubmitSystem, SceneRecordSystem, TimeUpdateSystem, ViewRectUpdateSystem,
};
use xcal_render::components::{
    Circle, CircleAnim, ClockState, FunctionX2Y, LocalClock, PhaseAnim, Sampler1D, StrokeSolid,
};
use xcal_render::{DrawQueue, DrawQueue3D, scene::Scene};
use xcal_render_backends::{RenderContext, WgpuBackend3D};

mod health;
mod systems_3d;
use health::HealthOverlay;
use systems_3d::{OrbitApplySystem, RenderSubmit3DSystem, SceneRecord3DSystem};

pub struct App {
    scheduler: Scheduler,
    render_ctx: RenderContext,
    health: HealthOverlay,
}

impl App {
    pub fn new(ctx: &eframe::CreationContext) -> Result<Self> {
        Self::setup_fonts(&ctx.egui_ctx)?;

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

        // 3D 相机 + 轨道动画（eye 每帧由 OrbitApplySystem 覆盖，初始值仅占位）。
        scene.spawn((
            Camera3D::new(
                glam::Vec3::new(0.0, 0.0, 5.0),
                glam::Vec3::ZERO,
                glam::Vec3::Y,
                Projection::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0),
            ),
            OrbitAnim::new(0.5, 5.0, glam::Vec3::Y, 0.0),
        ));

        scene.spawn((
            Circle::new((0.0, 0.0), 1.0),
            StrokeSolid::new(2.0, (100, 255, 0, 255)),
            // 动画驱动：start=0, speed=1；base=1.0, amp=0.4, omega=2.0 → radius 在 0.6~1.4 间 sin 振荡
            LocalClock::new(None, 0.0, 0.0, 1.0, 0.0, ClockState::Loop),
            CircleAnim::new(1.0, 0.4, 2.0),
        ));

        scene.spawn((
            FunctionX2Y::new(|x: f32| x.sin()),
            Sampler1D::new(-10.0f32, 10.0, 0.1),
            StrokeSolid::new(2.0, (0, 200, 200, 255)),
            // 相位滑动：start=0, speed=1；omega=1.5 → sin 曲线随时间水平平移
            LocalClock::new(None, 0.0, 0.0, 1.0, 0.0, ClockState::Loop),
            PhaseAnim::new(0.0, 1.5),
        ));

        let world = scene.into_world();
        let mut resources = Resources::new();
        resources.insert(AppCamera::default());
        resources.insert(TimeState::default());
        resources.insert(UiConfig::default());
        resources.insert(ViewRect::default());
        resources.insert(DrawQueue::new());
        resources.insert(DrawQueue3D::new());
        resources.insert(render_ctx.clone());

        let mut scheduler = Scheduler::new(world, resources);

        // 新阶段分配以避开同 phase 内 read/write 冲突：
        // - Input:        TimeUpdate (W=TimeState), CameraInput (R=EguiFrameContext, W=AppCamera)
        // - LogicUpdate:  ControlPanel (R=TimeState, AppCamera, EguiFrameContext, W=UiConfig)
        // - SceneSync:    ViewRectUpdate (R=EguiFrameContext, W=ViewRect)
        //                 AnimationApply (R=TimeState, W=())
        // - Prepare:      GridBuild (R=ViewRect, AppCamera, UiConfig, W=DrawQueue)
        // - RenderCommit: SceneRecord (R=ViewRect, AppCamera, W=DrawQueue)
        // - FrameEnd:     RenderSubmit (R=ViewRect, EguiFrameContext, DrawQueue, W=())
        scheduler.add_system(Phase::Input, TimeUpdateSystem);
        scheduler.add_system(Phase::Input, CameraInputSystem);

        scheduler.add_system(Phase::LogicUpdate, ControlPanelSystem);

        // SceneSync：ViewRectUpdate 写 ViewRect，AnimationApply 只读 TimeState，
        // 两者访问集不相交（writes ∩ reads∪writes = ∅），无冲突。
        // OrbitApply 同样只读 TimeState（Camera3D/OrbitAnim 是 hecs 组件）。
        scheduler.add_system(Phase::SceneSync, ViewRectUpdateSystem);
        scheduler.add_system(Phase::SceneSync, AnimationApplySystem);
        scheduler.add_system(Phase::SceneSync, OrbitApplySystem);

        scheduler.add_system(Phase::Prepare, GridBuildSystem);

        // RenderCommit：2D SceneRecord 写 DrawQueue(2D)，
        // 3D SceneRecord3D 写 DrawQueue3D，类型不相交。
        scheduler.add_system(Phase::RenderCommit, SceneRecordSystem);
        scheduler.add_system(Phase::RenderCommit, SceneRecord3DSystem);

        // FrameEnd：2D RenderSubmit 读写 DrawQueue(2D)；
        // 3D RenderSubmit3D 只读 EguiFrameContext + DrawQueue3D（回调 GPU 提交）。
        scheduler.add_system(Phase::FrameEnd, RenderSubmitSystem);
        scheduler.add_system(Phase::FrameEnd, RenderSubmit3DSystem);

        Ok(Self {
            scheduler,
            render_ctx,
            health: HealthOverlay::new(),
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
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let dt = ui.input(|i| i.unstable_dt).min(0.1);

        self.scheduler
            .resources_mut()
            .insert(EguiFrameContext::new(ui));

        self.scheduler.run_frame(dt);

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

        self.scheduler.resources_mut().remove::<EguiFrameContext>();

        if let Some(queue) = self.scheduler.resources_mut().get_mut::<DrawQueue>() {
            queue.clear();
        }

        // egui 默认事件驱动，无鼠标/键盘事件时不调度 ui()，导致
        // TimeState.elapsed 停止增长、动画卡住。动画引擎需持续重绘，每帧请求下一帧。
        ui.ctx().request_repaint();
    }
}
