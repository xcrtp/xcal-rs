//! Full-scene painting: background grid, edges, node cards, sockets and the
//! in-progress drag connection.
//!
//! Everything is drawn through a transformed `to_screen` closure so this
//! layer stays unaware of pan/zoom state (owned by `GraphCanvas`).

use egui::{Align2, Color32, FontId, Pos2, Rect, Stroke};
use glam::Vec2;
use xc_node::{InputSocketId, NodeId, OutputSocketId};
use xcal_node_editor::{Command, EdgePath, Editor};

use crate::edge_render::{EdgeRenderer, EdgeStyle};
use crate::view::NodeView;

const CARD_ROUNDING: f32 = 6.0;
const SOCKET_RADIUS: f32 = 5.0;
/// Horizontal gap between a pin dot and its type label (screen pixels).
const SOCKET_LABEL_GAP: f32 = 7.0;
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
    // 体检第 9 项（观察，暂不改）：本函数约 225 行，可按上述五个绘制阶段拆分。
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
            let Some(node) = editor.graph().get_node(id) else {
                continue;
            };
            let header_color = self.view.header_color(node);
            let title = self.view.title(node);
            let selected = editor.selection().contains(&id);

            // Card geometry is authored in graph space and mapped through the
            // viewport transform, so everything stays aligned at any zoom.
            let n_in = node.inputs().len() as u32;
            let n_out = node.outputs().len() as u32;
            let header_h = editor.socket_layout().header_height();
            let region_h = editor.socket_layout().socket_region_height(n_in, n_out);

            let header_bottom =
                to_screen(graph_rect.min + Vec2::new(graph_rect.size().x, header_h));
            let header_rect = Rect::from_min_max(
                screen_rect.min,
                Pos2::new(screen_rect.right(), header_bottom.y),
            );
            // Pin rows live in the band below the header; body widgets start
            // under that band so sliders never collide with pins/labels.
            let widgets_top = to_screen(graph_rect.min + Vec2::new(4.0, header_h + region_h));
            let widgets_bottom = to_screen(graph_rect.max - Vec2::new(4.0, 6.0));
            let body_rect = Rect::from_two_pos(
                Pos2::new(screen_rect.left() + 4.0, widgets_top.y),
                Pos2::new(screen_rect.right() - 4.0, widgets_bottom.y),
            );
            // Separator between the pin band and the property-widget band.
            let sep_y = widgets_top.y - 2.0;

            painter.rect_filled(screen_rect, CARD_ROUNDING, ui.visuals().window_fill);
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
            if n_in + n_out > 0 {
                painter.line_segment(
                    [
                        Pos2::new(screen_rect.left() + 4.0, sep_y),
                        Pos2::new(screen_rect.right() - 4.0, sep_y),
                    ],
                    Stroke::new(1.0, ui.visuals().window_stroke().color.gamma_multiply(0.5)),
                );
            }
            if selected {
                painter.rect_stroke(
                    screen_rect,
                    CARD_ROUNDING,
                    Stroke::new(2.0, Color32::from_rgb(250, 200, 90)),
                    egui::epaint::StrokeKind::Outside,
                );
            }

            // Resize grip: three diagonal strokes tucked into the bottom-right
            // corner (only for interaction-resizable node types).
            if node.meta().interaction.resizable {
                let corner = to_screen(graph_rect.max);
                let grip_color = ui.visuals().weak_text_color();
                let stroke = Stroke::new(1.5, grip_color);
                for (i, &inset) in [4.0_f32, 8.0, 12.0].iter().enumerate() {
                    let tail = i as f32 * 3.0 + 4.0;
                    painter.line_segment(
                        [
                            corner - egui::Vec2::new(inset, tail),
                            corner - egui::Vec2::new(tail, inset),
                        ],
                        stroke,
                    );
                }
            }

            // Interactive body: host egui widgets inside the card. Widget
            // state lives in the nodes themselves (egui rebuilds every
            // frame), so direct mutation is the source of truth. The body
            // Ui is bound exactly to the widget band and clips to it, so
            // nothing is ever painted outside the card; a plain top-down
            // left-aligned layout lets property widgets stretch to (and fill)
            // the whole band width, reflowing live when the card is resized.
            if body_rect.height() > 0.0 {
                let view = self.view;
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(body_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                    |ui: &mut egui::Ui| {
                        ui.set_clip_rect(body_rect.intersect(ui.clip_rect()));
                        if let Some(node) = editor.graph_mut().get_node_mut(id) {
                            view.body_ui(ui, node);
                        }
                    },
                );
            }
        }

        // --- sockets (on top of cards) ---
        let (pending_from, pending_hover) = editor
            .pending()
            .map(|p| (Some(p.from), p.hover_target))
            .unwrap_or((None, None));
        let highlight = Color32::from_rgb(250, 200, 90);
        let label_color = ui.visuals().weak_text_color();
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
                // Type label just inside the card edge.
                painter.text(
                    center + egui::Vec2::new(SOCKET_RADIUS + SOCKET_LABEL_GAP, 0.0),
                    Align2::LEFT_CENTER,
                    *tag,
                    FontId::monospace(11.0),
                    label_color,
                );
                if pending_hover == Some(socket) {
                    // Magnetic snap: the wire is locked onto this pin —
                    // halo + bright ring make the target obvious.
                    painter.circle_filled(
                        center,
                        SOCKET_RADIUS + 5.0,
                        highlight.gamma_multiply(0.25),
                    );
                    painter.circle_stroke(center, SOCKET_RADIUS + 4.0, Stroke::new(2.0, highlight));
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
                // Type label just inside the card edge (right-aligned).
                painter.text(
                    center - egui::Vec2::new(SOCKET_RADIUS + SOCKET_LABEL_GAP, 0.0),
                    Align2::RIGHT_CENTER,
                    *tag,
                    FontId::monospace(11.0),
                    label_color,
                );
                if pending_from == Some(socket) {
                    painter.circle_stroke(center, SOCKET_RADIUS + 4.0, Stroke::new(2.0, highlight));
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
