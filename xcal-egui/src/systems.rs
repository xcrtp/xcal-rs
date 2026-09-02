// systems.rs: egui 层 System 实现

use egui::Panel;

use xcal_core::executor::{ExecutorContext, System};
use xcal_core::transform::Transform2D;
use xcal_core::types::{Camera2D, Rect, Vec3};
use xcal_render::components::{
    FillSolid, FunctionX2Y, Line, Path, Sampler1D, StrokeSolid,
};
use xcal_render::{DrawQueue, ZIndex, record_scene};

use crate::egui_support::Egui2dRender;
use crate::resources::{AppCamera, EguiFrameContext, TimeState, UiConfig, ViewRect};

// ─── 从 Resources 提取 EguiFrameContext ────────────────────────────────────

fn extract_ui_ptr(ctx: &ExecutorContext<'_>) -> *mut egui::Ui {
    ctx.resources
        .borrow::<EguiFrameContext>()
        .map(|c| c.ui_ptr())
        .unwrap_or(std::ptr::null_mut())
}

// ─── TimeUpdateSystem ──────────────────────────────────────────────────────

pub struct TimeUpdateSystem;
impl System for TimeUpdateSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let dt = ctx.dt.min(0.1);
        if let Some(state) = ctx.resources.borrow_mut::<TimeState>() {
            state.elapsed += dt;
        }
    }
}

// ─── CameraInputSystem ─────────────────────────────────────────────────────

pub struct CameraInputSystem;
impl System for CameraInputSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let ui_ptr = extract_ui_ptr(ctx);
        if ui_ptr.is_null() { return; }

        let Some(camera) = ctx.resources.borrow_mut::<AppCamera>() else { return };
        let ui = unsafe { &mut *ui_ptr };
        camera.handle_input(ui);
    }
}

// ─── ControlPanelSystem ────────────────────────────────────────────────────

pub struct ControlPanelSystem;
impl System for ControlPanelSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let ui_ptr = extract_ui_ptr(ctx);
        if ui_ptr.is_null() { return; }
        let ui = unsafe { &mut *ui_ptr };

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

        if let Some(cfg) = ctx.resources.borrow_mut::<UiConfig>() {
            *cfg = ui_config;
        }
    }
}

// ─── ViewRectUpdateSystem ──────────────────────────────────────────────────

pub struct ViewRectUpdateSystem;
impl System for ViewRectUpdateSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let ui_ptr = extract_ui_ptr(ctx);
        if ui_ptr.is_null() { return; }
        let ui = unsafe { &mut *ui_ptr };

        if let Some(vr) = ctx.resources.borrow_mut::<ViewRect>() {
            vr.rect = Rect::from(ui.available_rect_before_wrap());
        }
    }
}

// ─── GridBuildSystem ───────────────────────────────────────────────────────

pub struct GridBuildSystem;
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

impl System for GridBuildSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
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

// ─── SceneRecordSystem ─────────────────────────────────────────────────────

pub struct SceneRecordSystem;
impl System for SceneRecordSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
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

// ─── RenderSubmitSystem ────────────────────────────────────────────────────

pub struct RenderSubmitSystem;
impl System for RenderSubmitSystem {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let ui_ptr = extract_ui_ptr(ctx);
        if ui_ptr.is_null() { return; }
        let ui = unsafe { &mut *ui_ptr };

        let view_rect = ctx
            .resources
            .borrow::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);

        let Some(queue) = ctx.resources.borrow_mut::<DrawQueue>() else { return };
        let painter = ui.painter_at(view_rect.into_egui_rect());
        queue.execute(&Egui2dRender::new(&painter));
    }
}
