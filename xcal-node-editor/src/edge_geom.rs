//! Edge path geometry: a renderer-agnostic intermediate representation plus
//! two path builders (cubic bezier / orthogonal polyline).
//!
//! Pure geometry — no rendering types appear here. The UI layer converts the
//! sampled points into painter calls.

use glam::Vec2;

/// Intermediate representation of a connection's shape, in graph-local
/// coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum EdgePath {
    Straight {
        from: Vec2,
        to: Vec2,
    },
    Bezier {
        from: Vec2,
        ctrl1: Vec2,
        ctrl2: Vec2,
        to: Vec2,
    },
    Polyline {
        points: Vec<Vec2>,
    },
}

impl EdgePath {
    /// Sample the path into `segments + 1` points (plus endpoints). Used for
    /// hit-testing and arrow placement. Polylines return their own vertices.
    pub fn sample(&self, segments: usize) -> Vec<Vec2> {
        match self {
            EdgePath::Straight { from, to } => (0..=segments)
                .map(|i| {
                    let t = i as f32 / segments.max(1) as f32;
                    Vec2::lerp(*from, *to, t)
                })
                .collect(),
            EdgePath::Bezier {
                from,
                ctrl1,
                ctrl2,
                to,
            } => (0..=segments)
                .map(|i| {
                    let t = i as f32 / segments.max(1) as f32;
                    let u = 1.0 - t;
                    *from * u * u * u
                        + *ctrl1 * 3.0 * u * u * t
                        + *ctrl2 * 3.0 * u * t * t
                        + *to * t * t * t
                })
                .collect(),
            EdgePath::Polyline { points } => points.clone(),
        }
    }

    /// Approximate total length (polyline accumulation; beziers are sampled
    /// with a fixed 32-segment resolution).
    pub fn length(&self) -> f32 {
        match self {
            EdgePath::Straight { from, to } => (*to - *from).length(),
            EdgePath::Bezier { .. } => polyline_length(&self.sample(32)),
            EdgePath::Polyline { points } => polyline_length(points),
        }
    }

    /// Shortest distance from `p` to the path (hit-testing). Beziers are
    /// approximated by 32 sampled segments.
    pub fn distance_to(&self, p: Vec2) -> f32 {
        match self {
            EdgePath::Straight { from, to } => point_segment_distance(p, *from, *to),
            EdgePath::Bezier { .. } => min_segment_distance(p, &self.sample(32)),
            EdgePath::Polyline { points } => min_segment_distance(p, points),
        }
    }
}

fn polyline_length(points: &[Vec2]) -> f32 {
    points.windows(2).map(|w| (w[1] - w[0]).length()).sum()
}

fn min_segment_distance(p: Vec2, points: &[Vec2]) -> f32 {
    points
        .windows(2)
        .map(|w| point_segment_distance(p, w[0], w[1]))
        .fold(f32::INFINITY, f32::min)
}

fn point_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len_sq = ab.length_squared();
    if len_sq <= f32::EPSILON {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / len_sq).clamp(0.0, 1.0);
    let proj = a + ab * t;
    (p - proj).length()
}

/// Builds an [`EdgePath`] between two socket positions.
pub trait EdgeGeometry: Send + Sync + 'static {
    fn build_path(&self, from: Vec2, to: Vec2) -> EdgePath;
}

/// Cubic bezier with horizontal tangents: the control points are pushed
/// sideways so curves never loop back on themselves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BezierGeometry {
    /// Horizontal control-point distance as a fraction of the (clamped)
    /// horizontal gap.
    pub curvature: f32,
}

impl Default for BezierGeometry {
    fn default() -> Self {
        Self { curvature: 0.5 }
    }
}

impl BezierGeometry {
    pub fn new() -> Self {
        Self::default()
    }
}

impl EdgeGeometry for BezierGeometry {
    fn build_path(&self, from: Vec2, to: Vec2) -> EdgePath {
        let dx = (to.x - from.x).abs().max(40.0) * self.curvature;
        EdgePath::Bezier {
            from,
            ctrl1: from + Vec2::new(dx, 0.0),
            ctrl2: to - Vec2::new(dx, 0.0),
            to,
        }
    }
}

/// Orthogonal (manhattan) routing: horizontal exit, vertical middle,
/// horizontal entry. Reverse connections (target to the left) detour below.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrthogonalGeometry {
    /// Minimum horizontal runway before a turn.
    pub min_offset: f32,
    /// Corner rounding radius (only consumed by the renderer; the geometry
    /// itself stays a sharp polyline).
    pub corner_radius: f32,
}

impl Default for OrthogonalGeometry {
    fn default() -> Self {
        Self {
            min_offset: 20.0,
            corner_radius: 8.0,
        }
    }
}

impl OrthogonalGeometry {
    pub fn new() -> Self {
        Self::default()
    }
}

impl EdgeGeometry for OrthogonalGeometry {
    fn build_path(&self, from: Vec2, to: Vec2) -> EdgePath {
        let points = if to.x - from.x >= 2.0 * self.min_offset {
            // Forward: S-curve through a vertical midline.
            let mid_x = (from.x + to.x) / 2.0;
            vec![from, Vec2::new(mid_x, from.y), Vec2::new(mid_x, to.y), to]
        } else {
            // Reverse (or nearly vertical): detour below both endpoints.
            let y_off = from.y.max(to.y) + self.min_offset * 2.0;
            vec![from, Vec2::new(from.x, y_off), Vec2::new(to.x, y_off), to]
        };
        EdgePath::Polyline { points }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bezier_controls_pull_outward_forward() {
        let g = BezierGeometry::default();
        let from = Vec2::new(0.0, 0.0);
        let to = Vec2::new(200.0, 100.0);
        match g.build_path(from, to) {
            EdgePath::Bezier { ctrl1, ctrl2, .. } => {
                assert!(ctrl1.x > from.x);
                assert!(ctrl2.x < to.x);
            }
            other => panic!("expected bezier, got {other:?}"),
        }
    }

    #[test]
    fn orthogonal_forward_is_four_points_with_exact_endpoints() {
        let g = OrthogonalGeometry::default();
        let from = Vec2::new(0.0, 0.0);
        let to = Vec2::new(200.0, 100.0);
        match g.build_path(from, to) {
            EdgePath::Polyline { points } => {
                assert_eq!(points.len(), 4);
                assert_eq!(points[0], from);
                assert_eq!(points[3], to);
            }
            other => panic!("expected polyline, got {other:?}"),
        }
    }

    #[test]
    fn orthogonal_reverse_detours_below() {
        let g = OrthogonalGeometry::default();
        let from = Vec2::new(200.0, 0.0);
        let to = Vec2::new(0.0, 0.0);
        match g.build_path(from, to) {
            EdgePath::Polyline { points } => {
                assert_eq!(points.len(), 4);
                let straight_y = 0.0_f32;
                // The detour segment must run below the straight from-to line.
                assert!(points[1].y > straight_y.max(from.y).max(to.y));
                assert!(points[2].y > from.y.max(to.y));
                assert_eq!(points[0], from);
                assert_eq!(points[3], to);
            }
            other => panic!("expected polyline, got {other:?}"),
        }
    }

    #[test]
    fn distance_to_straight_path() {
        let path = EdgePath::Straight {
            from: Vec2::new(0.0, 0.0),
            to: Vec2::new(100.0, 0.0),
        };
        assert!(path.distance_to(Vec2::new(50.0, 0.0)) < 1e-4);
        assert!(path.distance_to(Vec2::new(50.0, 30.0)) > 29.0);
        // Point beyond the endpoint clamps to the endpoint.
        assert!(path.distance_to(Vec2::new(150.0, 0.0)) > 49.0);
    }

    #[test]
    fn bezier_length_is_positive_and_samples_include_endpoints() {
        let g = BezierGeometry::default();
        let from = Vec2::new(0.0, 0.0);
        let to = Vec2::new(100.0, 60.0);
        let path = g.build_path(from, to);
        // A bowed curve is strictly longer than the chord.
        let chord = (to - from).length();
        assert!(path.length() > chord);
        let samples = path.sample(16);
        assert_eq!(samples.first().copied(), Some(from));
        assert_eq!(samples.last().copied(), Some(to));
    }
}
