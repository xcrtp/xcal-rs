// app.rs

use std::sync::Arc;

use egui::{Color32, Panel, Pos2, Rect, Stroke};
use egui_wgpu::RenderState;

use scene::comp as cm;

use crate::{
    scene::components::{Circle, CircleBuilder, FunctionX2Y, Sampler1D, Sampler1DBuilder},
    *,
};

/// ─────────────────────────────────────────────
/// 坐标变换工具模块
/// ─────────────────────────────────────────────
mod utils {
    use egui::{Pos2, Rect, pos2};

    /// 数学坐标 → 屏幕坐标
    #[inline]
    pub fn math_to_screen(mx: f32, my: f32, center: Pos2, scale: f32, rect: Rect) -> Pos2 {
        pos2(
            rect.center().x + (mx - center.x) * scale,
            rect.center().y - (my - center.y) * scale,
        )
    }

    /// 屏幕坐标 → 数学坐标
    #[inline]
    pub fn screen_to_math(px: f32, py: f32, center: Pos2, scale: f32, rect: Rect) -> (f32, f32) {
        let c = rect.center();
        (center.x + (px - c.x) / scale, center.y - (py - c.y) / scale)
    }

    /// 构造线段端点对（避免到处写 `.into()`）
    #[inline]
    pub fn seg(a: Pos2, b: Pos2) -> [Pos2; 2] {
        [a, b]
    }
}

/// ─────────────────────────────────────────────
/// 摄像机
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
    #[inline]
    pub fn math_to_screen(&self, mx: f32, my: f32, rect: Rect) -> Pos2 {
        utils::math_to_screen(mx, my, self.center, self.scale, rect)
    }

    #[inline]
    pub fn screen_to_math(&self, px: f32, py: f32, rect: Rect) -> (f32, f32) {
        utils::screen_to_math(px, py, self.center, self.scale, rect)
    }

    #[inline]
    pub fn length_to_screen(&self, length: f32) -> f32 {
        length * self.scale
    }

    #[inline]
    pub fn screen_to_length(&self, length: f32) -> f32 {
        length / self.scale
    }

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

    fn draw_grid(&self, painter: &egui::Painter, rect: Rect) {
        let step = self.camera.scale.max(10.0);
        let grid_unit = step / self.camera.scale;

        // 竖线
        let mut x =
            (self.camera.center.x - rect.center().x / self.camera.scale).floor() * grid_unit;

        while let Some(sx) = {
            let p = self.camera.math_to_screen(x, 0.0, rect);
            if p.x > rect.max.x { None } else { Some(p.x) }
        } {
            let a = Pos2::new(sx, rect.min.y);
            let b = Pos2::new(sx, rect.max.y);
            painter.line_segment(utils::seg(a, b), Stroke::new(1.0, Color32::from_gray(50)));
            x += grid_unit;
        }

        // 横线
        let mut y =
            (self.camera.center.y - rect.center().y / self.camera.scale).floor() * grid_unit;

        while let Some(sy) = {
            let p = self.camera.math_to_screen(0.0, y, rect);
            if p.y > rect.max.y { None } else { Some(p.y) }
        } {
            let a = Pos2::new(rect.min.x, sy);
            let b = Pos2::new(rect.max.x, sy);
            painter.line_segment(utils::seg(a, b), Stroke::new(1.0, Color32::from_gray(50)));
            y += grid_unit;
        }

        // 坐标轴
        let origin = self.camera.math_to_screen(0.0, 0.0, rect);
        painter.line_segment(
            utils::seg(
                Pos2::new(rect.min.x, origin.y),
                Pos2::new(rect.max.x, origin.y),
            ),
            Stroke::new(2.0, Color32::WHITE),
        );
        painter.line_segment(
            utils::seg(
                Pos2::new(origin.x, rect.min.y),
                Pos2::new(origin.x, rect.max.y),
            ),
            Stroke::new(2.0, Color32::WHITE),
        );
    }

    fn render_to_painter(&mut self, painter: &egui::Painter, rect: Rect) {
        use scene::components::*;

        let camera = &self.camera;

        self.scene.render(move |w, _r| {
            for (cir, st) in w
                .query::<(&Circle, &scene::components::render::StrokeSolid)>()
                .iter()
            {
                painter.circle_stroke(
                    camera.math_to_screen(cir.center.x, cir.center.y, rect),
                    camera.length_to_screen(cir.radius),
                    egui::Stroke {
                        width: st.width,
                        color: st.color.into_egui_color32(),
                    },
                );
            }

            for (func, sampler, stroke) in w
                .query::<(
                    &FunctionX2Y<f32, f32>,
                    &Sampler1D<f32>,
                    &scene::components::render::StrokeSolid,
                )>()
                .iter()
            {
                let mut prev: Option<Pos2> = None;

                let mut x = sampler.start;
                while x <= sampler.end {
                    let y = (func.f)(x);
                    let screen_pos = camera.math_to_screen(x, y, rect);

                    if let Some(p) = prev {
                        painter.line_segment(
                            utils::seg(p, screen_pos),
                            egui::Stroke {
                                width: stroke.width,
                                color: stroke.color.into_egui_color32(),
                            },
                        );
                    }

                    prev = Some(screen_pos);
                    x += sampler.step;
                }
            }
        });
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
            let rect = ui.max_rect();
            let painter = ui.painter();

            painter.rect_filled(rect, 0.0, Color32::from_gray(20));

            if self.show_grid {
                self.draw_grid(painter, rect);
            }

            // self.draw_sin(painter, rect);
            self.render_to_painter(painter, rect);
        });
    }
}
