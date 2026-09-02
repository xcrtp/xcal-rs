// xcal-app: 组装 + 启动层
//
// App::new 负责：
//   - 构建 Scene、初始化 World 图元
//   - 构建 Resources、挂载各个 egui Resource 类型
//   - 构建 Scheduler、按 Phase 注册 System
//   - 实现 eframe::App trait（ui 每帧调度）

use std::sync::Arc;

use xcal_core::errors::Error;
use xcal_core::executor::Resources;
use xcal_core::scheduler::{Phase, Scheduler};
pub use xcal_core::Result;
use xcal_egui::resources::{
    AppCamera, EguiFrameContext, TimeState, UiConfig, ViewRect,
};
use xcal_egui::systems::{
    CameraInputSystem, ControlPanelSystem, GridBuildSystem, RenderSubmitSystem,
    SceneRecordSystem, TimeUpdateSystem, ViewRectUpdateSystem,
};
use xcal_render::components::{Circle, FunctionX2Y, Sampler1D, StrokeSolid};
use xcal_render::{DrawQueue, RenderContext, scene::Scene};

pub struct App {
    scheduler: Scheduler,
    render_ctx: RenderContext,
}

impl App {
    pub fn new(ctx: &eframe::CreationContext) -> Result<Self> {
        Self::setup_fonts(&ctx.egui_ctx)?;

        let render_ctx = {
            let rs = ctx.wgpu_render_state.clone().ok_or(Error::InitializationError(
                "Wgpu render state not available".to_string(),
            ))?;
            RenderContext::from_render_state(&rs)
        };

        let mut scene = Scene::new();

        scene.spawn((
            Circle::new((0.0, 0.0), 1.0),
            StrokeSolid::new(2.0, (100, 255, 0, 255)),
        ));

        scene.spawn((
            FunctionX2Y::new(|x: f32| x.sin()),
            Sampler1D::new(-10.0f32, 10.0, 0.1),
            StrokeSolid::new(2.0, (0, 200, 200, 255)),
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

        scheduler.add_system(Phase::Input, TimeUpdateSystem);
        scheduler.add_system(Phase::Input, CameraInputSystem);

        scheduler.add_system(Phase::RenderCommit, ControlPanelSystem);
        scheduler.add_system(Phase::RenderCommit, ViewRectUpdateSystem);
        scheduler.add_system(Phase::RenderCommit, GridBuildSystem);
        scheduler.add_system(Phase::RenderCommit, SceneRecordSystem);
        scheduler.add_system(Phase::RenderCommit, RenderSubmitSystem);

        Ok(Self { scheduler, render_ctx })
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

        self.scheduler.resources_mut().insert(EguiFrameContext::new(ui));

        self.scheduler.run_frame(dt);

        self.scheduler.resources_mut().remove::<EguiFrameContext>();

        if let Some(queue) = self.scheduler.resources_mut().borrow_mut::<DrawQueue>() {
            queue.clear();
        }
    }
}
