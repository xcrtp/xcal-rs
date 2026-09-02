//! Full-scene painting: background grid, edges, node cards, sockets and the
//! in-progress drag connection.
//!
//! Everything is drawn through a transformed `to_screen` closure so this
//! layer stays unaware of pan/zoom state (owned by `GraphCanvas`).

use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};
use glam::Vec2;
use xcal_node::{InputSocketId, NodeId, OutputSocketId};
use xcal_node_editor::{Command, EdgePath, Editor};

use crate::edge_render::{EdgeRenderer, EdgeStyle};
use crate::view::NodeView;

/// Header band height; must match
/// `xcal_node_editor::DefaultSocketLayout::default().header_height`.
const HEADER_HEIGHT: f32 = 24.0;
const CARD_ROUNDING: f32 = 6.0;
const SOCKET_RADIUS: f32 = 5.0;
/// Screen-space grid spacing. Zoom is fixed at 1.0 in this stage; stage 8
/// scales this by the canvas zoom factor.
const GRID_SPACING: f32 = 32.0;

/// Paints the whole editor scene for one frame.
pub struct GraphPainter<'a> {
    view: &'a dyn NodeView,
    edge: &'a dyn EdgeRenderer,
}

impl<'a> GraphPainter<'a> {
    pub fn new(view: &'a dyn NodeView, edge: &'a dyn EdgeRenderer) -> Self {
        Self { view, edge }
    }

    /// Full repaint: background grid → edges → node cards → sockets → the
    /// pending drag connection.
    pub fn paint(
        &self,
        ui: &mut egui::Ui,
        editor: &mut Editor,
        to_screen: impl Fn(Vec2) -> Pos2 + Copy,
    ) {
        let painter = ui.painter().clone();
        draw_grid(&painter, painter.clip_rect(), ui.visuals());

        // --- committed edges ---
        let edges = editor.all_edges();
        for edge in edges {
            if let Some(path) = editor.connection_path(edge.source, edge.target) {
                let path = transform_path(&path, to_screen);
                self.edge.draw(&painter, &path, &EdgeStyle::default());
            }
        }

        // --- node cards ---
        let node_ids: Vec<NodeId> = editor.graph().all_nodes().collect();
        for &id in &node_ids {
            // Register an estimated card size the first time we see a node.
            if let Some(size) = editor.graph().get_node(id).map(|n| self.view.size_hint(n))
                && !editor.sizes().contains_key(&id)
            {
                let _ = editor.apply(Command::SetNodeSize { node: id, size });
            }

            let Some(graph_rect) = editor.node_rect(id) else {
                continue;
            };
            let screen_rect =
                Rect::from_two_pos(to_screen(graph_rect.min), to_screen(graph_rect.max));
            let (header_color, title) = editor
                .graph()
                .get_node(id)
                .map(|n| (self.view.header_color(n), self.view.title(n)))
                .unwrap_or((Color32::GRAY, String::new()));
            let selected = editor.selection().contains(&id);

            painter.rect_filled(screen_rect, CARD_ROUNDING, ui.visuals().window_fill);
            let header_rect = Rect::from_min_size(
                screen_rect.min,
                egui::Vec2::new(screen_rect.width(), HEADER_HEIGHT),
            );
            painter.rect_filled(
                header_rect,
                egui::CornerRadius {
                    nw: 6,
                    ne: 6,
                    sw: 0,
                    se: 0,
                },
                header_color,
            );
            painter.text(
                header_rect.left_center() + egui::Vec2::new(8.0, 0.0),
                Align2::LEFT_CENTER,
                title,
                FontId::proportional(13.0),
                Color32::WHITE,
            );
            if selected {
                painter.rect_stroke(
                    screen_rect,
                    CARD_ROUNDING,
                    Stroke::new(2.0, Color32::from_rgb(250, 200, 90)),
                    egui::epaint::StrokeKind::Outside,
                );
            }

            // Interactive body: host egui widgets inside the card. Widget
            // state lives in the nodes themselves (egui rebuilds every
            // frame), so direct mutation is the source of truth.
            let body_rect = Rect::from_min_max(
                header_rect.left_bottom() + egui::Vec2::new(4.0, 2.0),
                screen_rect.right_bottom() - egui::Vec2::new(4.0, 4.0),
            );
            let view = self.view;
            ui.put(body_rect, |ui: &mut egui::Ui| {
                if let Some(node) = editor.graph_mut().get_node_mut(id) {
                    view.body_ui(ui, node);
                }
                ui.allocate_response(egui::Vec2::ZERO, Sense::hover())
            });
        }

        // --- sockets (on top of cards) ---
        let (pending_from, pending_hover) = editor
            .pending()
            .map(|p| (Some(p.from), p.hover_target))
            .unwrap_or((None, None));
        let highlight = Color32::from_rgb(250, 200, 90);
        for id in &node_ids {
            let Some(node) = editor.graph().get_node(*id) else {
                continue;
            };
            let inputs: Vec<&'static str> = node
                .inputs()
                .iter()
                .map(|s| s.type_tag.type_name())
                .collect();
            let outputs: Vec<&'static str> = node
                .outputs()
                .iter()
                .map(|s| s.type_tag.type_name())
                .collect();

            for (i, tag) in inputs.iter().enumerate() {
                let socket = InputSocketId::new(*id, i as u32);
                let Some(pos) = editor.input_socket_pos(socket) else {
                    continue;
                };
                let center = to_screen(pos);
                painter.circle_filled(center, SOCKET_RADIUS, socket_color(tag));
                painter.circle_stroke(
                    center,
                    SOCKET_RADIUS,
                    Stroke::new(1.5, Color32::from_gray(25)),
                );
                if pending_hover == Some(socket) {
                    painter.circle_stroke(center, SOCKET_RADIUS + 3.0, Stroke::new(2.0, highlight));
                }
            }
            for (i, tag) in outputs.iter().enumerate() {
                let socket = OutputSocketId::new(*id, i as u32);
                let Some(pos) = editor.output_socket_pos(socket) else {
                    continue;
                };
                let center = to_screen(pos);
                painter.circle_filled(center, SOCKET_RADIUS, socket_color(tag));
                painter.circle_stroke(
                    center,
                    SOCKET_RADIUS,
                    Stroke::new(1.5, Color32::from_gray(25)),
                );
                if pending_from == Some(socket) {
                    painter.circle_stroke(center, SOCKET_RADIUS + 3.0, Stroke::new(2.0, highlight));
                }
            }
        }

        // --- pending drag connection (drawn on top) ---
        if let Some(path) = editor.pending_path() {
            let path = transform_path(&path, to_screen);
            let phase = ui.ctx().input(|i| i.time) as f32;
            self.edge.draw(&painter, &path, &EdgeStyle::pending(phase));
        }
    }
}

/// Map every point of a graph-local path through the screen transform.
fn transform_path(path: &EdgePath, to_screen: impl Fn(Vec2) -> Pos2) -> EdgePath {
    let v = |p: Vec2| {
        let s = to_screen(p);
        Vec2::new(s.x, s.y)
    };
    match path {
        EdgePath::Straight { from, to } => EdgePath::Straight {
            from: v(*from),
            to: v(*to),
        },
        EdgePath::Bezier {
            from,
            ctrl1,
            ctrl2,
            to,
        } => EdgePath::Bezier {
            from: v(*from),
            ctrl1: v(*ctrl1),
            ctrl2: v(*ctrl2),
            to: v(*to),
        },
        EdgePath::Polyline { points } => EdgePath::Polyline {
            points: points.iter().copied().map(v).collect(),
        },
    }
}

/// Faint dot grid filling the visible clip rectangle.
fn draw_grid(painter: &egui::Painter, clip: Rect, visuals: &egui::Visuals) {
    let stroke = Stroke::new(1.0, visuals.window_stroke().color.gamma_multiply(0.15));
    let mut x = (clip.left() / GRID_SPACING).floor() * GRID_SPACING;
    while x <= clip.right() {
        painter.line_segment(
            [Pos2::new(x, clip.top()), Pos2::new(x, clip.bottom())],
            stroke,
        );
        x += GRID_SPACING;
    }
    let mut y = (clip.top() / GRID_SPACING).floor() * GRID_SPACING;
    while y <= clip.bottom() {
        painter.line_segment(
            [Pos2::new(clip.left(), y), Pos2::new(clip.right(), y)],
            stroke,
        );
        y += GRID_SPACING;
    }
}

/// Fixed socket color per wire type; unknown types get neutral gray.
fn socket_color(type_name: &str) -> Color32 {
    match type_name {
        "f32" => Color32::from_rgb(245, 166, 35),  // orange
        "vec2" => Color32::from_rgb(86, 140, 255), // blue
        "bool" => Color32::from_rgb(228, 86, 86),  // red
        _ => Color32::from_rgb(170, 170, 180),     // gray
    }
}
