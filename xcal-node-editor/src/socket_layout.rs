//! Socket geometry inside a node card.
//!
//! Hit-testing (editor layer) and painting (UI layer) must agree on where
//! sockets sit, so the offset computation lives here in pure geometry and is
//! shared by both layers.
//!
//! Card layout (graph-local units, 1:1 with pixels at zoom 1.0):
//!
//! ```text
//! ┌──────────────────────────────┐ y = 0
//! │  header (title only, no pins)│ height = [`CARD_HEADER_HEIGHT`]
//! ├──────────────────────────────┤ body starts
//! │ pin rows (dot on edge +      │ socket region: [`SOCKET_TOP_MARGIN`]
//! │ type label inside the card)  │   + rows * [`SOCKET_ROW_HEIGHT`]
//! │                              │   + [`SOCKET_BOTTOM_GAP`]
//! ├──────────────────────────────┤ property widgets (UI layer)
//! │  node body widgets …         │
//! └──────────────────────────────┘
//! ```

use glam::Vec2;

/// Header band height: title only, sockets never overlap the header.
pub const CARD_HEADER_HEIGHT: f32 = 24.0;
/// Vertical distance between socket rows.
pub const SOCKET_ROW_HEIGHT: f32 = 24.0;
/// Gap between the header bottom and the first socket row's top.
pub const SOCKET_TOP_MARGIN: f32 = 8.0;
/// Gap below the last socket row before the property-widget region starts.
pub const SOCKET_BOTTOM_GAP: f32 = 6.0;

/// Offset of a socket (relative to the node card's top-left corner).
pub trait SocketLayout: Send + Sync + 'static {
    /// Offset of input socket `index` (out of `count` inputs) on a card of
    /// size `node_size`.
    fn input_offset(&self, index: u32, count: u32, node_size: Vec2) -> Vec2;
    /// Offset of output socket `index` (out of `count` outputs).
    fn output_offset(&self, index: u32, count: u32, node_size: Vec2) -> Vec2;

    /// Height of the title band. Property widgets must not be drawn above
    /// this offset.
    fn header_height(&self) -> f32 {
        CARD_HEADER_HEIGHT
    }

    /// Height of the body band reserved for socket rows (below the header,
    /// above the property-widget region). The UI layer starts the interactive
    /// body at `header_height() + socket_region_height(..)`; card height
    /// estimates include this band.
    fn socket_region_height(&self, inputs: u32, outputs: u32) -> f32 {
        let rows = inputs.max(outputs);
        SOCKET_TOP_MARGIN + rows as f32 * SOCKET_ROW_HEIGHT + SOCKET_BOTTOM_GAP
    }
}

/// Default card geometry: inputs hug the left edge, outputs hug the right
/// edge, rows are evenly distributed in the socket region below the header.
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
            header_height: CARD_HEADER_HEIGHT,
            row_height: SOCKET_ROW_HEIGHT,
            top_margin: SOCKET_TOP_MARGIN,
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
    fn header_height(&self) -> f32 {
        self.header_height
    }

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
        let size = Vec2::new(160.0, 160.0);
        let ys: Vec<f32> = (0..3).map(|i| l.input_offset(i, 3, size).y).collect();
        assert!(ys[1] > ys[0] && ys[2] > ys[1]);
        for y in ys {
            // Rows live strictly below the header.
            assert!(y > l.header_height);
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
        let size = Vec2::new(160.0, 160.0);
        for i in 0..3 {
            let out = l.output_offset(i, 3, size);
            assert_eq!(out.x, size.x);
            assert_eq!(out.y, l.input_offset(i, 3, size).y);
        }
    }

    #[test]
    fn socket_region_starts_after_header_and_grows_with_rows() {
        let l = DefaultSocketLayout::default();
        let empty = l.socket_region_height(0, 0);
        let two = l.socket_region_height(2, 2);
        // Each additional pin row adds exactly one row height.
        assert_eq!(two - empty, 2.0 * SOCKET_ROW_HEIGHT);
        // The widget region starts strictly below the header.
        assert!(l.header_height() + empty > l.header_height());
        // Pin rows sit inside the region (below header, above widgets).
        let first_row = l.header_height + SOCKET_TOP_MARGIN + 0.5 * SOCKET_ROW_HEIGHT;
        let last_row = l.header_height + SOCKET_TOP_MARGIN + 1.5 * SOCKET_ROW_HEIGHT;
        assert!(first_row > l.header_height);
        assert!(last_row < l.header_height + two);
    }
}
