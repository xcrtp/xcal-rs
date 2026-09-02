// app.rs
//
// App 只负责初始化：构建 World、注册 Resources、按阶段挂载 System，
// 然后每帧调用 Scheduler。业务逻辑全部下沉到各个 System 实现中。
//
// 借用规则：ExecutorContext 所有字段平铺，Rust split borrow 允许
// 同时 &mut ctx.ui + &mut ctx.resources + &mut ctx.world（不同字段）。
// System 内必须直接访问平铺字段（如 ctx.resources.borrow_mut::<T>()），
// 不能通过 &mut self 方法间接访问，否则编译器无法追踪到具体字段。

use std::sync::Arc;

use egui::{Panel, Pos2};

use crate::{
    executor::{ExecutorContext, Resources},
    render::{
        egui_support::Egui2dRender, record_scene, DrawQueue, Transform2D, ZIndex,
    },
    render_context::RenderContext,
    scheduler::{Phase, Scheduler},
    scene::{self, comp},
    scene::components::{Circle, FillSolid, FunctionX2Y, Line, Path, Sampler1D, StrokeSolid},
    types::{Camera2D, Rect, Vec3},
    Error, Result,
};

// ─── 全局 Resource 类型 ────────────────────────────────────────────────────

/// 逻辑相机参数（center + scale），由相机输入 System 更新。
#[derive(Debug, Clone)]
pub struct AppCamera {
    pub center: Pos2,
    pub scale: f32,
}

impl Default for AppCamera {
    fn default() -> Self {
        Self {
            center: Pos2::new(0.0, 0.0),
            scale: 40.0,
        }
    }
}

impl AppCamera {
    pub fn handle_input(&mut self, ui: &egui::Ui) {
        let input = ui.input(|i| i.clone());
        let scroll = input.smooth_scroll_delta.y;
        if scroll != 0.0 {
            let zoom_factor = 1.0 + scroll * 0.001;
            self.scale = (self.scale * zoom_factor).clamp(5.0, 500.0);
        }
        if input.pointer.secondary_down() || input.pointer.middle_down() {
            let delta = input.pointer.delta();
            self.center.x -= delta.x / self.scale;
            self.center.y += delta.y / self.scale;
        }
    }

    pub fn to_camera_2d(&self, rect: Rect) -> Camera2D {
        Camera2D::new(
            (self.center.x, self.center.y).into(),
            self.scale,
            rect,
        )
    }
}

/// 累计运行时间（秒）。
#[derive(Debug, Default)]
pub struct TimeState {
    pub elapsed: f32,
}

/// UI 配置开关。
#[derive(Debug, Clone)]
pub struct UiConfig {
    pub show_grid: bool,
    pub show_field: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self { show_grid: true, show_field: false }
    }
}

/// 本帧视口矩形（屏幕坐标）。
#[derive(Debug, Clone)]
pub struct ViewRect {
    pub rect: Rect,
}

impl Default for ViewRect {
    fn default() -> Self {
        Self {
            rect: Rect::new((0.0, 0.0).into(), (800.0, 600.0).into()),
        }
    }
}

// ─── System 实现 ───────────────────────────────────────────────────────────

/// 时间累加 System。
struct TimeUpdateSystem;
impl crate::executor::System for TimeUpdateSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let dt = ctx.dt.min(0.1);
        // 直接字段访问 ctx.resources，split borrow 允许同时操作其他字段
        if let Some(state) = ctx.resources.borrow_mut::<TimeState>() {
            state.elapsed += dt;
        }
    }
}

/// 相机输入 System：ctx.ui + ctx.resources —— 不同字段，split borrow 允许。
struct CameraInputSystem;
impl crate::executor::System for CameraInputSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let Some(ui) = ctx.ui.as_deref_mut() else { return };
        // 直接字段访问，编译器知道只借用 self.resources
        let Some(camera) = ctx.resources.borrow_mut::<AppCamera>() else { return };
        camera.handle_input(ui);
    }
}

/// 控制面板绘制 System。
/// 闭包内不能使用 ctx（会触发 &mut ctx.ui 和 &mut ctx.resources 冲突），
/// 所以在闭包外预先 clone 所有需要的值，闭包结束后再写回 Resource。
struct ControlPanelSystem;
impl crate::executor::System for ControlPanelSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let Some(ui) = ctx.ui.as_deref_mut() else { return };

        // ── Phase 1：闭包外预先提取/clone 所有只读值 ──
        let time = ctx
            .resources
            .borrow::<TimeState>()
            .map(|t| t.elapsed)
            .unwrap_or(0.0);
        let camera = ctx
            .resources
            .borrow::<AppCamera>()
            .cloned()
            .unwrap_or_default();
        let mut ui_config = ctx
            .resources
            .borrow::<UiConfig>()
            .cloned()
            .unwrap_or_default();

        // ── Phase 2：Panel show 闭包内只操作 clone 的值 ──
        Panel::left("controls")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.heading("数学可视化");
                ui.label(format!("t = {:.3}", time));
                ui.separator();

                ui.checkbox(&mut ui_config.show_grid, "显示网格");
                ui.checkbox(&mut ui_config.show_field, "显示向量场");

                ui.separator();
                ui.label("Camera");
                ui.label(format!(
                    "center: ({:.2}, {:.2})",
                    camera.center.x, camera.center.y
                ));
                ui.label(format!("scale: {:.1}", camera.scale));
            });

        // ── Phase 3：闭包结束后写回 UiConfig ──
        if let Some(cfg) = ctx.resources.borrow_mut::<UiConfig>() {
            *cfg = ui_config;
        }
    }
}

/// 视口矩形提取 System。
/// 必须在 ControlPanelSystem 之后运行：SidePanel 显示后 cursor 右移，
/// available_rect_before_wrap() 返回面板右侧的剩余区域（即 2D 渲染区）。
/// 用 max_rect() 会返回整个窗口区域，导致网格背景覆盖面板。
struct ViewRectUpdateSystem;
impl crate::executor::System for ViewRectUpdateSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let Some(ui) = ctx.ui.as_deref_mut() else { return };
        // 直接字段访问 resources，split borrow 不冲突
        if let Some(vr) = ctx.resources.borrow_mut::<ViewRect>() {
            vr.rect = Rect::from(ui.available_rect_before_wrap());
        }
    }
}

/// 网格构建 System：先 clone 只读 Resource，再 mutably borrow DrawQueue。
struct GridBuildSystem;
impl GridBuildSystem {
    fn push_grid(queue: &mut DrawQueue, camera: &Camera2D, rect: Rect) {
        let center = camera.center;
        let scale = camera.scale;
        let half_w = rect.size().x / 2.0 / scale;
        let half_h = rect.size().y / 2.0 / scale;
        let x0 = center.x - half_w;
        let x1 = center.x + half_w;
        let y0 = center.y - half_h;
        let y1 = center.y + half_h;

        let grid = StrokeSolid::new(1.0, (50, 50, 50, 255));
        let axis = StrokeSolid::new(2.0, (255, 255, 255, 255));
        let z_grid = ZIndex::new(-100);
        let z_axis = ZIndex::new(-90);
        let unit = scale.max(10.0) / scale;

        let mut x = (x0 / unit).floor() * unit;
        while x <= x1 {
            queue.line_stroke(
                z_grid,
                Line::new((x, y0), (x, y1)).to_screen(camera),
                grid.clone(),
            );
            x += unit;
        }

        let mut y = (y0 / unit).floor() * unit;
        while y <= y1 {
            queue.line_stroke(
                z_grid,
                Line::new((x0, y), (x1, y)).to_screen(camera),
                grid.clone(),
            );
            y += unit;
        }

        queue.line_stroke(
            z_axis,
            Line::new((x0, 0.0), (x1, 0.0)).to_screen(camera),
            axis.clone(),
        );
        queue.line_stroke(
            z_axis,
            Line::new((0.0, y0), (0.0, y1)).to_screen(camera),
            axis,
        );
    }
}

impl crate::executor::System for GridBuildSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // Phase 1：提取只读值（先全部读完，再可变借用 DrawQueue）
        let rect = ctx
            .resources
            .borrow::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let cam = ctx
            .resources
            .borrow::<AppCamera>()
            .cloned()
            .unwrap_or_default();
        let show_grid = ctx
            .resources
            .borrow::<UiConfig>()
            .map(|c| c.show_grid)
            .unwrap_or(true);

        let camera = cam.to_camera_2d(rect);

        // Phase 2：可变借用 DrawQueue
        let Some(queue) = ctx.resources.borrow_mut::<DrawQueue>() else { return };

        queue.rect_fill(
            ZIndex::new(-1000),
            rect,
            FillSolid::new((20, 20, 20, 255)),
        );

        if show_grid {
            Self::push_grid(queue, &camera, rect);
        }
    }
}

/// 场景录制 System：World 图元 + 函数曲线采样 → DrawQueue。
struct SceneRecordSystem;
impl crate::executor::System for SceneRecordSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // Phase 1：只读 + clone（全部从 ctx.resources 读，不碰 ctx.world）
        let rect = ctx
            .resources
            .borrow::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let cam = ctx
            .resources
            .borrow::<AppCamera>()
            .cloned()
            .unwrap_or_default();
        let camera = cam.to_camera_2d(rect);

        // Phase 2：可变借用 DrawQueue + 不可变借用 World
        // 直接用 &*ctx.world 避免经过 &self 方法，编译器知道只是 &self.world
        let Some(queue) = ctx.resources.borrow_mut::<DrawQueue>() else { return };
        let world: &hecs::World = &*ctx.world;

        record_scene(world, &camera, queue);

        for (func, sampler, stroke) in world
            .query::<(&FunctionX2Y<f32, f32>, &Sampler1D<f32>, &StrokeSolid)>()
            .iter()
        {
            let mut pts: Vec<Vec3> = Vec::new();
            let mut x = sampler.start;
            while x <= sampler.end {
                pts.push((x, (func.f)(x), 0.0).into());
                x += sampler.step;
            }
            queue.path_stroke(
                ZIndex::new(0),
                Path::new(pts, false).to_screen(&camera),
                stroke.clone(),
            );
        }
    }
}

/// 渲染提交 System：DrawQueue.execute() → Egui2dRender。
/// 必须用带 clip 的 painter_at(rect) 替换 ui.painter()，
/// 否则函数曲线/circle 等图元会画到视口外部覆盖面板。
/// 借用顺序：先取 ViewRect（Copy，立即释放），再 borrow_mut DrawQueue。
struct RenderSubmitSystem;
impl crate::executor::System for RenderSubmitSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let Some(ui) = ctx.ui.as_deref_mut() else { return };

        // Phase 1：只读提取 ViewRect 值（Copy 类型，immutable borrow 立即释放）
        let view_rect = ctx
            .resources
            .borrow::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);

        // Phase 2：可变借用 DrawQueue + 用带 clip 的 Painter 绘制
        let Some(queue) = ctx.resources.borrow_mut::<DrawQueue>() else { return };
        let painter = ui.painter_at(view_rect.into_egui_rect());
        queue.execute(&Egui2dRender::new(&painter));
    }
}

// ─── App 入口 ──────────────────────────────────────────────────────────────

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

        // 构建初始 Scene
        let mut scene = scene::Scene::new();

        scene.spawn((
            Circle::new((0.0, 0.0), 1.0),
            comp::StrokeSolid::new(2.0, (100, 255, 0, 255)),
        ));

        scene.spawn((
            comp::FunctionX2Y::new(|x: f32| x.sin()),
            comp::Sampler1D::new(-10.0f32, 10.0, 0.1),
            comp::StrokeSolid::new(2.0, (0, 200, 200, 255)),
        ));

        // 从 Scene 取出 World，构造 Resources
        let world = scene.into_world();
        let mut resources = Resources::new();
        resources.insert(AppCamera::default());
        resources.insert(TimeState::default());
        resources.insert(UiConfig::default());
        resources.insert(ViewRect::default());
        resources.insert(DrawQueue::new());
        resources.insert(render_ctx.clone());

        // 构造 Scheduler，按阶段注册 System
        let mut scheduler = Scheduler::new(world, resources);

        scheduler.add_system(Phase::Input, TimeUpdateSystem);
        scheduler.add_system(Phase::Input, CameraInputSystem);

        // RenderCommit 阶段（顺序：ControlPanel → ViewRect → Grid → Record → Submit）
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
        self.scheduler.run_frame(dt, Some(ui));

        // 帧末清空 DrawQueue
        if let Some(queue) = self.scheduler.resources_mut().borrow_mut::<DrawQueue>() {
            queue.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_camera_default() {
        let c = AppCamera::default();
        assert_eq!(c.center, Pos2::new(0.0, 0.0));
        assert_eq!(c.scale, 40.0);
    }

    #[test]
    fn test_ui_config_default() {
        let cfg = UiConfig::default();
        assert!(cfg.show_grid);
        assert!(!cfg.show_field);
    }
}
