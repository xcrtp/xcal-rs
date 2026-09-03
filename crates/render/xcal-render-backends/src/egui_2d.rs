// egui_2d.rs
//
// 基于 egui::Painter 的 Painter2D 实现（由 xcal-egui/egui_support.rs 迁移，
// 冻结的 paint(&Shape, &Style) 签名与逻辑不变）。
//
// 受孤儿规则限制，xcal-geometry 的 Color32 无法在本层之外实现
// Into<egui::Color32>，这里以自由函数 color_to_egui 提供转换。

use egui::{Painter, Pos2, Shape as EguiShape, Stroke, StrokeKind};
use xc_geometry::Color32;
use xcal_render::{Painter2D, Shape, Style};

/// 把 xcal-geometry Color32 转换为 egui Color32（直取 RGBA）。
pub fn color_to_egui(c: Color32) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), c.a())
}

/// egui 2D 绘制器：把冻结 2D 管线的 Shape/Style 翻译为 egui 绘制命令。
pub struct EguiPainter<'a> {
    painter: &'a Painter,
}

impl<'a> EguiPainter<'a> {
    pub fn new(painter: &'a Painter) -> Self {
        Self { painter }
    }
}

impl<'a> Painter2D for EguiPainter<'a> {
    fn paint(&self, shape: &Shape, style: &Style) {
        match (shape, style) {
            (Shape::Circle(c), Style::Stroke(s)) => {
                self.painter.circle_stroke(
                    Pos2::new(c.center.x, c.center.y),
                    c.radius,
                    Stroke::new(s.width, color_to_egui(s.color)),
                );
            }
            (Shape::Circle(c), Style::Fill(f)) => {
                self.painter.circle_filled(
                    Pos2::new(c.center.x, c.center.y),
                    c.radius,
                    color_to_egui(f.color),
                );
            }
            (Shape::Path(p), Style::Stroke(s)) => {
                let stroke = Stroke::new(s.width, color_to_egui(s.color));
                let pts: Vec<Pos2> = p.vertices.iter().map(|v| Pos2::new(v.x, v.y)).collect();
                for w in pts.windows(2) {
                    self.painter.line_segment([w[0], w[1]], stroke);
                }
                if p.closed && pts.len() >= 2 {
                    self.painter
                        .line_segment([*pts.last().unwrap(), pts[0]], stroke);
                }
            }
            (Shape::Path(p), Style::Fill(f)) => {
                let pts: Vec<Pos2> = p.vertices.iter().map(|v| Pos2::new(v.x, v.y)).collect();
                if pts.len() >= 3 {
                    self.painter.add(EguiShape::convex_polygon(
                        pts,
                        color_to_egui(f.color),
                        Stroke::NONE,
                    ));
                }
            }
            (Shape::Line(l), Style::Stroke(s)) => {
                self.painter.line_segment(
                    [Pos2::new(l.a.x, l.a.y), Pos2::new(l.b.x, l.b.y)],
                    Stroke::new(s.width, color_to_egui(s.color)),
                );
            }
            (Shape::Line(_), Style::Fill(_)) => {}
            (Shape::Rect(r), Style::Fill(f)) => {
                self.painter
                    .rect_filled((*r).into_egui_rect(), 0.0, color_to_egui(f.color));
            }
            (Shape::Rect(r), Style::Stroke(s)) => {
                self.painter.rect_stroke(
                    (*r).into_egui_rect(),
                    0.0,
                    Stroke::new(s.width, color_to_egui(s.color)),
                    StrokeKind::Middle,
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_core::types::{Rect, Vec3};
    use xcal_render::components::{Circle, FillSolid, Line, Path, StrokeSolid};

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

        let circle = Circle::new((0.0, 0.0), 1.0);
        let path = Path::new(
            vec![
                Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
                Vec3::new(glam::Vec3::new(2.0, 0.0, 0.0)),
            ],
            true,
        );
        let line = Line::new((0.0, 0.0), (1.0, 1.0));
        let rect = Rect::new((0.0, 0.0).into(), (10.0, 10.0).into());
        let stroke = StrokeSolid::new(1.0, (255, 0, 0, 255));
        let fill = FillSolid::new((0, 255, 0, 128));

        r.paint(
            &Shape::Circle(circle.clone()),
            &Style::Stroke(stroke.clone()),
        );
        r.paint(&Shape::Circle(circle), &Style::Fill(fill.clone()));
        r.paint(&Shape::Path(path.clone()), &Style::Stroke(stroke.clone()));
        r.paint(&Shape::Path(path), &Style::Fill(fill.clone()));
        r.paint(&Shape::Line(line), &Style::Stroke(stroke.clone()));
        r.paint(&Shape::Line(line), &Style::Fill(fill.clone()));
        r.paint(&Shape::Rect(rect), &Style::Stroke(stroke.clone()));
        r.paint(&Shape::Rect(rect), &Style::Fill(fill));
    }
}
