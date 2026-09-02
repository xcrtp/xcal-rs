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
use xcal_core::scheduler::{Phase, Scheduler};
use xcal_egui::resources::{AppCamera, EguiFrameContext, TimeState, UiConfig, ViewRect};
use xcal_egui::systems::{
    AnimationApplySystem, CameraInputSystem, ControlPanelSystem, GridBuildSystem,
    RenderSubmitSystem, SceneRecordSystem, TimeUpdateSystem, ViewRectUpdateSystem,
};
use xcal_render::components::{
    Circle, CircleAnim, ClockState, FunctionX2Y, LocalClock, PhaseAnim, Sampler1D, StrokeSolid,
};
use xcal_render::{DrawQueue, RenderContext, scene::Scene};

pub struct App {
    scheduler: Scheduler,
    render_ctx: RenderContext,
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
            RenderContext::from_render_state(&rs)
        };

        let mut scene = Scene::new();

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
        scheduler.add_system(Phase::SceneSync, ViewRectUpdateSystem);
        scheduler.add_system(Phase::SceneSync, AnimationApplySystem);

        scheduler.add_system(Phase::Prepare, GridBuildSystem);

        scheduler.add_system(Phase::RenderCommit, SceneRecordSystem);

        scheduler.add_system(Phase::FrameEnd, RenderSubmitSystem);

        Ok(Self {
            scheduler,
            render_ctx,
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

        self.scheduler.resources_mut().remove::<EguiFrameContext>();

        if let Some(queue) = self.scheduler.resources_mut().get_mut::<DrawQueue>() {
            queue.clear();
        }

        // egui 默认事件驱动，无鼠标/键盘事件时不调度 ui()，导致
        // TimeState.elapsed 停止增长、动画卡住。动画引擎需持续重绘，每帧请求下一帧。
        ui.ctx().request_repaint();
    }
}
