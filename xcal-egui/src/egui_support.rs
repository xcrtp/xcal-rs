// egui_support.rs: Egui2dRender - 基于 egui::Painter 的 Painter2D 实现

use egui::{Painter, Pos2, Shape as EguiShape, Stroke, StrokeKind};
use xcal_render::{Painter2D, Shape, Style};

pub struct Egui2dRender<'a> {
    painter: &'a Painter,
}

impl<'a> Egui2dRender<'a> {
    pub fn new(painter: &'a Painter) -> Self {
        Self { painter }
    }
}

impl<'a> Painter2D for Egui2dRender<'a> {
    fn paint(&self, shape: &Shape, style: &Style) {
        match (shape, style) {
            (Shape::Circle(c), Style::Stroke(s)) => {
                self.painter.circle_stroke(
                    Pos2::new(c.center.x, c.center.y),
                    c.radius,
                    Stroke::new(s.width, s.color.into_egui_color32()),
                );
            }
            (Shape::Circle(c), Style::Fill(f)) => {
                self.painter.circle_filled(
                    Pos2::new(c.center.x, c.center.y),
                    c.radius,
                    f.color.into_egui_color32(),
                );
            }
            (Shape::Path(p), Style::Stroke(s)) => {
                let stroke = Stroke::new(s.width, s.color.into_egui_color32());
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
                        f.color.into_egui_color32(),
                        Stroke::NONE,
                    ));
                }
            }
            (Shape::Line(l), Style::Stroke(s)) => {
                self.painter.line_segment(
                    [Pos2::new(l.a.x, l.a.y), Pos2::new(l.b.x, l.b.y)],
                    Stroke::new(s.width, s.color.into_egui_color32()),
                );
            }
            (Shape::Line(_), Style::Fill(_)) => {}
            (Shape::Rect(r), Style::Fill(f)) => {
                self.painter.rect_filled(
                    (*r).into_egui_rect(),
                    0.0,
                    f.color.into_egui_color32(),
                );
            }
            (Shape::Rect(r), Style::Stroke(s)) => {
                self.painter.rect_stroke(
                    (*r).into_egui_rect(),
                    0.0,
                    Stroke::new(s.width, s.color.into_egui_color32()),
                    StrokeKind::Middle,
                );
            }
        }
    }
}

#[cfg(test)]
mod egui_tests {
    use super::*;
    use xcal_render::components::{Circle, FillSolid, Line, Path, StrokeSolid};
    use xcal_core::types::{Rect, Vec3};

    #[test]
    fn test_egui2d_render_paint_all_shapes() {
        let ctx = egui::Context::default();
        let painter = ctx.debug_painter();
        let r = Egui2dRender::new(&painter);

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
