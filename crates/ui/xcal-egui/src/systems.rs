// systems.rs: egui 层 System 实现（新架构）
//
// 每个 System 显式声明 `Reads` / `Writes` 关联类型，使用
// `ctx.get` / `ctx.get_mut` 访问 Resource。旧 `ctx.resources.borrow*`
// 已废弃。
//
// Phase 分配（避开同 phase 内 read/write 冲突）：
// - Input:        CameraInput  (R=EguiFrameContext, W=AppCamera)
// - LogicUpdate:  ControlPanel (R=TimeState, AppCamera, EguiFrameContext, W=UiConfig)
// - SceneSync:    ViewRectUpdate (R=EguiFrameContext, W=ViewRect)
// - Prepare:      GridBuild    (R=ViewRect, AppCamera, UiConfig, W=DrawQueue)
// - RenderCommit: SceneRecord  (R=ViewRect, AppCamera, W=DrawQueue)
// - FrameEnd:     RenderSubmit (R=ViewRect, EguiFrameContext, DrawQueue, W=())
//
// 注：时间推进由 DirectorAdvanceSystem（xcal-core）负责，动画由 BindingApplySystem
// （xcal-core::animation）驱动；旧的 TimeUpdateSystem / AnimationApplySystem 已删除。

use egui::Panel;

use xcal_core::context::ExecutorContext;
use xcal_core::system::System;
use xcal_core::transform::Transform2D;
use xcal_core::types::{Camera2D, Rect, Vec3};
use xcal_render::components::{
    FillSolid, FunctionX2Y, Line, Path, PhaseAnim, Sampler1D, StrokeSolid,
};
use xcal_render::{DrawQueue, ZIndex, record_scene};
use xcal_render_backends::EguiPainter;

use crate::resources::{AppCamera, EguiFrameContext, TimeState, UiConfig, ViewRect};

// ─── 从 Resources 提取 EguiFrameContext ────────────────────────────────────

fn extract_ui_ptr(ctx: &mut ExecutorContext<'_>) -> *mut egui::Ui {
    ctx.get::<EguiFrameContext>()
        .map(|c| c.ui_ptr())
        .unwrap_or(std::ptr::null_mut())
}

// ─── CameraInputSystem ─────────────────────────────────────────────────────

pub struct CameraInputSystem;
impl System for CameraInputSystem {
    type Reads = (EguiFrameContext,);
    type Writes = (AppCamera,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let ui_ptr = extract_ui_ptr(ctx);
        if ui_ptr.is_null() {
            return;
        }

        let Some(camera) = ctx.get_mut::<AppCamera>() else {
            return;
        };
        let ui = unsafe { &mut *ui_ptr };
        camera.handle_input(ui);
    }
}

// ─── ControlPanelSystem ────────────────────────────────────────────────────

pub struct ControlPanelSystem;
impl System for ControlPanelSystem {
    type Reads = (TimeState, AppCamera, EguiFrameContext);
    type Writes = (UiConfig,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let ui_ptr = extract_ui_ptr(ctx);
        if ui_ptr.is_null() {
            return;
        }
        let ui = unsafe { &mut *ui_ptr };

        // 三阶段闭包模式：先克隆只读值，再在闭包内修改本地副本，
        // 闭包结束后写回 Resources。避免 BorrowTracker 同时持有多份 &mut。
        let time = ctx.get::<TimeState>().map(|t| t.elapsed).unwrap_or(0.0);
        let camera = ctx.get::<AppCamera>().cloned().unwrap_or_default();
        let mut ui_config = ctx.get::<UiConfig>().cloned().unwrap_or_default();

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

        if let Some(cfg) = ctx.get_mut::<UiConfig>() {
            *cfg = ui_config;
        }
    }
}

// ─── ViewRectUpdateSystem ──────────────────────────────────────────────────

pub struct ViewRectUpdateSystem;
impl System for ViewRectUpdateSystem {
    type Reads = (EguiFrameContext,);
    type Writes = (ViewRect,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let ui_ptr = extract_ui_ptr(ctx);
        if ui_ptr.is_null() {
            return;
        }
        let ui = unsafe { &mut *ui_ptr };

        if let Some(vr) = ctx.get_mut::<ViewRect>() {
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
    type Reads = (ViewRect, AppCamera, UiConfig);
    type Writes = (DrawQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 三阶段闭包：先克隆只读值，再写回。
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let cam = ctx.get::<AppCamera>().cloned().unwrap_or_default();
        let show_grid = ctx.get::<UiConfig>().map(|c| c.show_grid).unwrap_or(true);

        let camera = cam.to_camera_2d(rect);

        let Some(queue) = ctx.get_mut::<DrawQueue>() else {
            return;
        };

        queue.rect_fill(ZIndex::new(-1000), rect, FillSolid::new((20, 20, 20, 255)));

        if show_grid {
            Self::push_grid(queue, &camera, rect);
        }
    }
}

// ─── SceneRecordSystem ─────────────────────────────────────────────────────

pub struct SceneRecordSystem;
impl System for SceneRecordSystem {
    type Reads = (ViewRect, AppCamera);
    type Writes = (DrawQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let cam = ctx.get::<AppCamera>().cloned().unwrap_or_default();
        let camera = cam.to_camera_2d(rect);

        // 同时需要 &hecs::World（不可变）与 &mut DrawQueue；
        // 通过 split-borrow 辅助 API 在闭包内同时持有两个引用。
        ctx.with_world_and_resource_mut::<DrawQueue, _>(|world, queue| {
            record_scene(world, &camera, queue);

            for (func, sampler, stroke, phase) in world
                .query::<(
                    &FunctionX2Y<f32, f32>,
                    &Sampler1D<f32>,
                    &StrokeSolid,
                    Option<&PhaseAnim>,
                )>()
                .iter()
            {
                // BindingApplySystem 写入的相位偏移：让 sin 曲线随时间水平滑动。
                let phase = phase.map(|p| p.phase).unwrap_or(0.0);
                let mut pts: Vec<Vec3> = Vec::new();
                let mut x = sampler.start;
                while x <= sampler.end {
                    pts.push((x, (func.f)(x + phase), 0.0).into());
                    x += sampler.step;
                }
                queue.path_stroke(
                    ZIndex::new(0),
                    Path::new(pts, false).to_screen(&camera),
                    stroke.clone(),
                );
            }
        });
    }
}

// ─── RenderSubmitSystem ────────────────────────────────────────────────────

pub struct RenderSubmitSystem;
impl System for RenderSubmitSystem {
    type Reads = (ViewRect, EguiFrameContext);
    type Writes = (DrawQueue,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let ui_ptr = extract_ui_ptr(ctx);
        if ui_ptr.is_null() {
            return;
        }
        let ui = unsafe { &mut *ui_ptr };

        let view_rect = ctx
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);

        let Some(queue) = ctx.get_mut::<DrawQueue>() else {
            return;
        };
        let painter = ui.painter_at(view_rect.into_egui_rect());
        queue.execute(&EguiPainter::new(&painter));
    }
}
