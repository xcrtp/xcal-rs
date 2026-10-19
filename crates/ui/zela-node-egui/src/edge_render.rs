//! Edge painting: bezier / orthogonal renderers, styles and arrowheads.
//!
//! All coordinates here are screen coordinates: the caller transforms
//! graph-local [`EdgePath`]s via the viewport transform before drawing.

use egui::epaint::{CubicBezierShape, PathShape};
use egui::{Color32, Pos2, Shape, Stroke};
use zela_node_editor::{EdgePath, Vec2};

/// Line treatment for an edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StrokeKind {
    Solid,
    Dashed,
    /// Dashed with a moving offset; `phase` is animation time in seconds.
    AnimatedDashed {
        phase: f32,
    },
}

/// Arrowhead treatment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ArrowStyle {
    None,
    Triangle { size: f32 },
}

/// Visual style for an edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeStyle {
    pub color: Color32,
    pub width: f32,
    pub stroke: StrokeKind,
    pub arrow: ArrowStyle,
    /// Corner rounding radius for orthogonal polylines (beveled corners).
    pub corner_radius: f32,
}

impl Default for EdgeStyle {
    fn default() -> Self {
        Self {
            color: Color32::from_rgb(190, 190, 205),
            width: 2.0,
            stroke: StrokeKind::Solid,
            arrow: ArrowStyle::Triangle { size: 9.0 },
            corner_radius: 8.0,
        }
    }
}

impl EdgeStyle {
    /// Style for the in-progress drag connection.
    pub fn pending(phase: f32) -> Self {
        Self {
            color: Color32::from_rgb(250, 200, 90),
            width: 2.0,
            stroke: StrokeKind::AnimatedDashed { phase },
            arrow: ArrowStyle::None,
            corner_radius: 8.0,
        }
    }
}

/// Paints an [`EdgePath`] with a fixed visual strategy.
pub trait EdgeRenderer: Send + Sync + 'static {
    fn draw(&self, painter: &egui::Painter, path: &EdgePath, style: &EdgeStyle);
}

fn to_pos2(p: Vec2) -> Pos2 {
    Pos2::new(p.x, p.y)
}

/// Draws cubic-bezier edges natively; other path variants fall back to
/// sampled polylines.
#[derive(Debug, Default, Clone, Copy)]
pub struct BezierRenderer;

impl BezierRenderer {
    pub fn new() -> Self {
        Self
    }
}

impl EdgeRenderer for BezierRenderer {
    fn draw(&self, painter: &egui::Painter, path: &EdgePath, style: &EdgeStyle) {
        let stroke = Stroke::new(style.width, style.color);
        match path {
            EdgePath::Bezier {
                from,
                ctrl1,
                ctrl2,
                to,
            } => {
                if matches!(style.stroke, StrokeKind::Solid) {
                    let shape = CubicBezierShape::from_points_stroke(
                        [
                            to_pos2(*from),
                            to_pos2(*ctrl1),
                            to_pos2(*ctrl2),
                            to_pos2(*to),
                        ],
                        false,
                        Color32::TRANSPARENT,
                        stroke,
                    );
                    painter.add(Shape::CubicBezier(shape));
                } else {
                    stroke_samples(painter, &path.sample(32), stroke, style);
                }
            }
            EdgePath::Straight { from, to } => {
                if matches!(style.stroke, StrokeKind::Solid) {
                    painter.line_segment([to_pos2(*from), to_pos2(*to)], stroke);
                } else {
                    stroke_samples(painter, &[*from, *to], stroke, style);
                }
            }
            EdgePath::Polyline { points } => {
                stroke_samples(painter, points, stroke, style);
            }
        }
        draw_arrow(painter, path, style);
    }
}

/// Draws orthogonal (polyline) edges with beveled corners.
#[derive(Debug, Default, Clone, Copy)]
pub struct OrthogonalRenderer;

impl OrthogonalRenderer {
    pub fn new() -> Self {
        Self
    }
}

impl EdgeRenderer for OrthogonalRenderer {
    fn draw(&self, painter: &egui::Painter, path: &EdgePath, style: &EdgeStyle) {
        let stroke = Stroke::new(style.width, style.color);
        match path {
            EdgePath::Polyline { points } => {
                let beveled = bevel_corners(points, style.corner_radius);
                let screen: Vec<Pos2> = beveled.iter().copied().map(to_pos2).collect();
                if matches!(style.stroke, StrokeKind::Solid) {
                    painter.add(PathShape::line(screen, stroke));
                } else {
                    let graph: Vec<Vec2> = beveled;
                    stroke_samples(painter, &graph, stroke, style);
                }
            }
            // Non-polyline paths (shouldn't happen with OrthogonalGeometry):
            // fall back to sampled lines.
            other => stroke_samples(painter, &other.sample(32), stroke, style),
        }
        draw_arrow(painter, path, style);
    }
}

/// Cut each interior corner by `radius` along both adjacent segments,
/// producing beveled corners instead of sharp 90° turns.
fn bevel_corners(points: &[Vec2], radius: f32) -> Vec<Vec2> {
    if points.len() < 3 || radius <= 0.0 {
        return points.to_vec();
    }
    let mut out = Vec::with_capacity(points.len() * 2);
    out.push(points[0]);
    for i in 1..points.len() - 1 {
        let prev = points[i - 1];
        let corner = points[i];
        let next = points[i + 1];
        let to_prev = prev - corner;
        let to_next = next - corner;
        let len_prev = to_prev.length();
        let len_next = to_next.length();
        if len_prev < 1e-3 || len_next < 1e-3 {
            out.push(corner);
            continue;
        }
        let cut = radius.min(len_prev / 2.0).min(len_next / 2.0);
        out.push(corner + to_prev / len_prev * cut);
        out.push(corner + to_next / len_next * cut);
    }
    out.push(*points.last().unwrap());
    out
}

/// Stroke a polyline either solid or dashed (with optional animation).
fn stroke_samples(painter: &egui::Painter, points: &[Vec2], stroke: Stroke, style: &EdgeStyle) {
    let screen: Vec<Pos2> = points.iter().copied().map(to_pos2).collect();
    match style.stroke {
        StrokeKind::Solid => {
            painter.add(PathShape::line(screen, stroke));
        }
        StrokeKind::Dashed => {
            for shape in Shape::dashed_line(&screen, stroke, 10.0, 7.0) {
                painter.add(shape);
            }
        }
        StrokeKind::AnimatedDashed { phase } => {
            // Moving offset makes the dashes flow toward the target.
            let offset = (phase * 40.0).rem_euclid(17.0);
            for shape in Shape::dashed_line_with_offset(&screen, stroke, &[10.0], &[7.0], offset) {
                painter.add(shape);
            }
        }
    }
}

/// Draw a filled triangle at half the path's arc length, oriented along the
/// flow direction (source → target).
fn draw_arrow(painter: &egui::Painter, path: &EdgePath, style: &EdgeStyle) {
    let (size, tip_offset) = match style.arrow {
        ArrowStyle::Triangle { size } => (size, size * 0.9),
        ArrowStyle::None => return,
    };
    let samples = path.sample(32);
    if samples.len() < 2 {
        return;
    }
    let total: f32 = samples.windows(2).map(|w| (w[1] - w[0]).length()).sum();
    let target = total * 0.5;
    let mut acc = 0.0;
    for w in samples.windows(2) {
        let seg = w[1] - w[0];
        let len = seg.length();
        if acc + len >= target {
            let t = if len > 1e-6 {
                (target - acc) / len
            } else {
                0.0
            };
            let center = Vec2::lerp(w[0], w[1], t);
            let dir = seg / len.max(1e-6);
            let perp = Vec2::new(-dir.y, dir.x);
            let tip = center + dir * tip_offset;
            let base = center - dir * tip_offset * 0.7;
            let points = [
                to_pos2(tip),
                to_pos2(base + perp * size * 0.55),
                to_pos2(base - perp * size * 0.55),
            ];
            painter.add(Shape::convex_polygon(
                points.to_vec(),
                style.color,
                Stroke::NONE,
            ));
            return;
        }
        acc += len;
    }
}
