//! Socket geometry inside a node card.
//!
//! Hit-testing (editor layer) and painting (UI layer) must agree on where
//! sockets sit, so the offset computation lives here in pure geometry and is
//! shared by both layers.

use glam::Vec2;

/// Offset of a socket (relative to the node card's top-left corner).
pub trait SocketLayout: Send + Sync + 'static {
    /// Offset of input socket `index` (out of `count` inputs) on a card of
    /// size `node_size`.
    fn input_offset(&self, index: u32, count: u32, node_size: Vec2) -> Vec2;
    /// Offset of output socket `index` (out of `count` outputs).
    fn output_offset(&self, index: u32, count: u32, node_size: Vec2) -> Vec2;
}

/// Default card geometry: inputs hug the left edge, outputs hug the right
/// edge, rows are evenly distributed below the header.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefaultSocketLayout {
    /// Header (title bar) height in pixels.
    pub header_height: f32,
    /// Vertical distance between socket rows.
    pub row_height: f32,
    /// Gap between the header bottom and the first socket row's top.
    pub top_margin: f32,
}

impl Default for DefaultSocketLayout {
    fn default() -> Self {
        Self {
            header_height: 24.0,
            row_height: 20.0,
            top_margin: 6.0,
        }
    }
}

impl DefaultSocketLayout {
    pub fn new() -> Self {
        Self::default()
    }

    fn row_y(&self, index: u32) -> f32 {
        self.header_height + self.top_margin + (index as f32 + 0.5) * self.row_height
    }
}

impl SocketLayout for DefaultSocketLayout {
    fn input_offset(&self, index: u32, _count: u32, _node_size: Vec2) -> Vec2 {
        Vec2::new(0.0, self.row_y(index))
    }

    fn output_offset(&self, index: u32, _count: u32, node_size: Vec2) -> Vec2 {
        Vec2::new(node_size.x, self.row_y(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs_are_left_edge_strictly_increasing_and_within_card() {
        let l = DefaultSocketLayout::default();
        let size = Vec2::new(160.0, 120.0);
        let ys: Vec<f32> = (0..3).map(|i| l.input_offset(i, 3, size).y).collect();
        assert!(ys[1] > ys[0] && ys[2] > ys[1]);
        for y in ys {
            assert!(y >= l.header_height);
            assert!(y <= size.y);
        }
        // Inputs sit on the left edge.
        for i in 0..3 {
            assert_eq!(l.input_offset(i, 3, size).x, 0.0);
        }
    }

    #[test]
    fn outputs_hug_right_edge_with_same_row_y() {
        let l = DefaultSocketLayout::default();
        let size = Vec2::new(160.0, 120.0);
        for i in 0..3 {
            let out = l.output_offset(i, 3, size);
            assert_eq!(out.x, size.x);
            assert_eq!(out.y, l.input_offset(i, 3, size).y);
        }
    }
}
