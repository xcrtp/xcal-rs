// app.rs

use std::sync::Arc;

use egui::{Panel, Pos2};
use egui_wgpu::RenderState;

use scene::comp as cm;

use crate::{
    render::{egui_support::Egui2dRender, record_scene, DrawQueue, Transform2D, ZIndex},
    scene::components::{Circle, FillSolid, FunctionX2Y, Line, Path, Sampler1D, StrokeSolid},
    types::{Camera2D, Rect, Vec3},
    *,
};

/// ─────────────────────────────────────────────
/// 摄像机（仅负责交互：拖拽平移 + 滚轮缩放）
/// ─────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct Camera {
    /// 屏幕中心对应的数学坐标
    pub center: Pos2,
    /// 每单位数学坐标对应的像素数
    pub scale: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            center: Pos2::new(0.0, 0.0),
            scale: 40.0,
        }
    }
}

impl Camera {
    /// 处理鼠标输入（拖拽平移 + 滚轮缩放）
    pub fn handle_input(&mut self, ui: &egui::Ui) {
        let input = ui.input(|i| i.clone());

        // 滚轮缩放
        let scroll = input.smooth_scroll_delta.y;
        if scroll != 0.0 {
            let zoom_factor = 1.0 + scroll * 0.001;
            self.scale = (self.scale * zoom_factor).clamp(5.0, 500.0);
        }

        // 右键 / 中键拖拽平移
        if input.pointer.secondary_down() || input.pointer.middle_down() {
            let delta = input.pointer.delta();
            self.center.x -= delta.x / self.scale;
            self.center.y += delta.y / self.scale;
        }
    }
}

/// ─────────────────────────────────────────────
/// App
/// ─────────────────────────────────────────────
pub struct App {
    _rs: RenderState,
    time: f32,
    show_grid: bool,
    show_field: bool,
    scene: scene::Scene,
    camera: Camera,
}

impl App {
    pub fn new(ctx: &eframe::CreationContext) -> Result<Self> {
        Self::setup_fonts(&ctx.egui_ctx)?;

        let mut app = Self {
            _rs: ctx
                .wgpu_render_state
                .clone()
                .ok_or(Error::InitializationError(
                    "Wgpu render state not available".to_string(),
                ))?,
            time: 0.0,
            show_grid: true,
            show_field: false,
            scene: scene::Scene::new(),
            camera: Camera::default(),
        };

        app.scene.spawn((
            Circle::new((0.0, 0.0), 1.0),
            cm::StrokeSolid::new(2.0, (100, 255, 0, 255)),
        ));

        app.scene.spawn((
            cm::FunctionX2Y::new(|x: f32| x.sin()),
            cm::Sampler1D::new(-10.0f32, 10.0, 0.1),
            cm::StrokeSolid::new(2.0, (0, 200, 200, 255)),
        ));

        app.scene.render(|w, _| {
            println!(
                "Circle count {}",
                w.query::<(&Circle, &scene::components::render::StrokeSolid)>()
                    .iter()
                    .count()
            )
        });

        Ok(app)
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

    /// 生成网格与坐标轴线段命令（逻辑坐标 → 屏幕坐标后压入队列）。
    fn push_grid(&self, queue: &mut DrawQueue, camera: &Camera2D, rect: Rect) {
        let center = camera.center;
        let scale = camera.scale;
        // 视口覆盖的逻辑范围
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

        // 网格间距（逻辑单位）
        let unit = scale.max(10.0) / scale;

        // 竖线
        let mut x = (x0 / unit).floor() * unit;
        while x <= x1 {
            queue.line_stroke(
                z_grid,
                Line::new((x, y0), (x, y1)).to_screen(camera),
                grid.clone(),
            );
            x += unit;
        }

        // 横线
        let mut y = (y0 / unit).floor() * unit;
        while y <= y1 {
            queue.line_stroke(
                z_grid,
                Line::new((x0, y), (x1, y)).to_screen(camera),
                grid.clone(),
            );
            y += unit;
        }

        // 坐标轴
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.time += ui.input(|i| i.unstable_dt).min(0.1);
        self.camera.handle_input(ui);

        Panel::left("controls")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.heading("数学可视化");
                ui.label(format!("t = {:.3}", self.time));
                ui.separator();
                ui.checkbox(&mut self.show_grid, "显示网格");
                ui.checkbox(&mut self.show_field, "显示向量场");

                ui.separator();
                ui.label("Camera");
                ui.label(format!(
                    "center: ({:.2}, {:.2})",
                    self.camera.center.x, self.camera.center.y
                ));
                ui.label(format!("scale: {:.1}", self.camera.scale));
            });

        egui::CentralPanel::default().show(ui, |ui| {
            let rect = Rect::from(ui.max_rect());
            let camera = Camera2D::new(
                (self.camera.center.x, self.camera.center.y).into(),
                self.camera.scale,
                rect,
            );

            let mut queue = DrawQueue::new();

            // 背景（最底层）
            queue.rect_fill(ZIndex::new(-1000), rect, FillSolid::new((20, 20, 20, 255)));

            if self.show_grid {
                self.push_grid(&mut queue, &camera, rect);
            }

            // 场景实体（Circle 等）录制进队列；函数曲线在此采样为 Path。
            self.scene.render(|w, _| {
                record_scene(w, &camera, &mut queue);

                for (func, sampler, stroke) in w
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
            });

            // 按 ZIndex 排序后一次性交给 EGUI 绘制
            queue.execute(&Egui2dRender::new(ui.painter()));
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_camera_default() {
        let c = Camera::default();
        assert_eq!(c.center, Pos2::new(0.0, 0.0));
        assert_eq!(c.scale, 40.0);
    }
}
