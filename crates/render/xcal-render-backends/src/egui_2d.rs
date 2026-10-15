// egui_2d.rs
//
// 基于 egui::Painter 的 Painter2D 实现（旧 2D 管线，S3-1 后仍保留可切换）。
//
// M0（图元体系迁移）：入参已由旧 `Shape`/`Style` 改为 [`Shape2D`]/[`Style2D`]。
// 绘制分支与原实现一一对应（填充 / 描边各自至多一次），故画面不变：
// - 圆：`circle_filled` / `circle_stroke`；
// - 线段：`line_segment`；
// - 矩形：`rect_filled` / `rect_stroke(StrokeKind::Middle)`；
// - 路径：按**子路径分组**展平后逐段 `line_segment`（闭合由 `Path2D::Close`
//   在展平时补出回起点段）；填充走
//   `convex_polygon`（与旧 `Shape::Path` 填充分支一致）。
// Shape2D 的其余变体（Ellipse/Polygon/Group/Empty/Custom）本条管线当前不产生，
// 留待 S3-1 GPU 后端统一处理。
//
// 受孤儿规则限制，xcal-geometry 的 Color32 无法在本层之外实现
// Into<egui::Color32>，这里以自由函数 color_to_egui 提供转换。

use egui::{Painter, Pos2, Shape as EguiShape, Stroke, StrokeKind};
use xc_geometry::Color32;
use xcal_render::{Painter2D, Shape2D, Style2D};

/// 曲线展平容差（屏幕像素）。
const FLATTEN_TOLERANCE: f32 = 0.1;

/// 把 xcal-geometry Color32 转换为 egui Color32（直取 RGBA）。
pub fn color_to_egui(c: Color32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), c.a())
}

#[inline]
fn pos(p: xc_geometry::Vec2) -> Pos2 {
    Pos2::new(p.x, p.y)
}

/// egui 2D 绘制器：把 2D 管线的 Shape2D/Style2D 翻译为 egui 绘制命令。
pub struct EguiPainter<'a> {
    painter: &'a Painter,
}

impl<'a> EguiPainter<'a> {
    pub fn new(painter: &'a Painter) -> Self {
        Self { painter }
    }

    fn stroke_of(&self, style: &Style2D) -> Option<Stroke> {
        style
            .stroke
            .map(|c| Stroke::new(style.stroke_width, color_to_egui(c)))
    }
}

impl<'a> Painter2D for EguiPainter<'a> {
    fn paint(&self, shape: &Shape2D, style: &Style2D) {
        match shape {
            Shape2D::Circle { center, radius } => {
                if let Some(fill) = style.fill {
                    self.painter
                        .circle_filled(pos(*center), *radius, color_to_egui(fill));
                }
                if let Some(stroke) = self.stroke_of(style) {
                    self.painter.circle_stroke(pos(*center), *radius, stroke);
                }
            }
            Shape2D::Line { start, end } => {
                if let Some(stroke) = self.stroke_of(style) {
                    self.painter
                        .line_segment([pos(*start), pos(*end)], stroke);
                }
            }
            Shape2D::Rect(r) => {
                let rect = egui::Rect::from_min_max(pos(r.min), pos(r.max));
                if let Some(fill) = style.fill {
                    self.painter.rect_filled(rect, 0.0, color_to_egui(fill));
                }
                if let Some(stroke) = self.stroke_of(style) {
                    self.painter
                        .rect_stroke(rect, 0.0, stroke, StrokeKind::Middle);
                }
            }
            Shape2D::Polyline { points } | Shape2D::Polygon { points } => {
                if let Some(stroke) = self.stroke_of(style) {
                    let pts: Vec<Pos2> = points.iter().map(|p| pos(*p)).collect();
                    for w in pts.windows(2) {
                        self.painter.line_segment([w[0], w[1]], stroke);
                    }
                }
            }
            Shape2D::Path(p) => {
                let groups = p.flatten_subpaths(FLATTEN_TOLERANCE);
                if let Some(stroke) = self.stroke_of(style) {
                    for group in &groups {
                        let pts: Vec<Pos2> = group.iter().map(|p| pos(*p)).collect();
                        for w in pts.windows(2) {
                            self.painter.line_segment([w[0], w[1]], stroke);
                        }
                    }
                }
                if let Some(fill) = style.fill {
                    for group in &groups {
                        if group.len() >= 3 {
                            let pts: Vec<Pos2> = group.iter().map(|p| pos(*p)).collect();
                            self.painter.add(EguiShape::convex_polygon(
                                pts,
                                color_to_egui(fill),
                                Stroke::NONE,
                            ));
                        }
                    }
                }
            }
            // 本条管线不产生以下变体（S3-1 GPU 后端负责）。
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_geometry::{Path2D, Rect2D, Vec2};

    fn stroke_style(width: f32, color: Color32) -> Style2D {
        Style2D {
            fill: None,
            stroke: Some(color),
            stroke_width: width,
            space: xc_geometry::MeasureSpace::Screen,
        }
    }

    fn fill_style(color: Color32) -> Style2D {
        Style2D {
            fill: Some(color),
            stroke: None,
            stroke_width: 0.0,
            space: xc_geometry::MeasureSpace::Screen,
        }
    }

    #[test]
    fn test_color_to_egui_maps_channels() {
        let c = color_to_egui(Color32::new(10, 20, 30, 40));
        assert_eq!(c, egui::Color32::from_rgba_unmultiplied(10, 20, 30, 40));
    }

    #[test]
    fn test_egui_painter_paint_all_shapes() {
        let ctx = egui::Context::default();
        let painter = ctx.debug_painter();
        let r = EguiPainter::new(&painter);

        let circle = Shape2D::Circle {
            center: Vec2::new(0.0, 0.0),
            radius: 1.0,
        };
        let path = Shape2D::Path(Path2D::from_polyline(
            &[
                Vec2::new(0.0, 0.0),
                Vec2::new(1.0, 1.0),
                Vec2::new(2.0, 0.0),
            ],
            true,
        ));
        let line = Shape2D::Line {
            start: Vec2::new(0.0, 0.0),
            end: Vec2::new(1.0, 1.0),
        };
        let rect = Shape2D::Rect(Rect2D::new(Vec2::new(0.0, 0.0), Vec2::new(10.0, 10.0)));
        let stroke = stroke_style(1.0, Color32::new(255, 0, 0, 255));
        let fill = fill_style(Color32::new(0, 255, 0, 128));

        r.paint(&circle, &stroke);
        r.paint(&circle, &fill);
        r.paint(&path, &stroke);
        r.paint(&path, &fill);
        r.paint(&line, &stroke);
        r.paint(&line, &fill);
        r.paint(&rect, &stroke);
        r.paint(&rect, &fill);
    }

    /// 未产生的变体不 panic、不绘制（`_ => {}` 分支）。
    #[test]
    fn test_egui_painter_ignores_unused_variants() {
        let ctx = egui::Context::default();
        let painter = ctx.debug_painter();
        let r = EguiPainter::new(&painter);
        r.paint(&Shape2D::Empty, &fill_style(Color32::WHITE));
        r.paint(
            &Shape2D::Group { children: vec![] },
            &stroke_style(1.0, Color32::WHITE),
        );
    }
}
