//! Interactive node editor demo.
//!
//! Run with:
//! ```text
//! cargo run -p xcal-node-egui --example node_editor_demo
//! ```
//!
//! Four node types (all carrying the `f32` wire type) demonstrate the full
//! loop: card rendering, drag connections with type/cycle validation, live
//! parameter widgets, undo/redo, pan/zoom and a right-click spawn menu.
//! A tiny forward propagation pass feeds values from `Constant` nodes
//! through the graph so `Add` / `Sin` / `Display` bodies show live results.

use std::any::Any;

use eframe::egui;
use glam::Vec2;
use xcal_node::{
    DirtyPolicy, InputSocket, InputSocketId, NamedTypeTag, Node, NodeId, NodeMeta, OutputSocket,
    OutputSocketId, SimpleGraph, TypeTag,
};
use xcal_node_editor::{BezierGeometry, Command, Editor, EditorEvent, FreeLayout};
use xcal_node_egui::{BezierRenderer, NodeGraphApp, NodeView};

// ----------------------------------------------------------------------------
// Demo node types
// ----------------------------------------------------------------------------

fn f32_tag() -> Box<dyn TypeTag> {
    Box::new(NamedTypeTag::new("f32"))
}

fn node_sockets(inputs: usize, outputs: usize) -> (Vec<InputSocket>, Vec<OutputSocket>) {
    let ins = (0..inputs)
        .map(|i| InputSocket::new(f32_tag()).with_index(i as u32))
        .collect();
    let outs = (0..outputs)
        .map(|i| OutputSocket::new(f32_tag()).with_index(i as u32))
        .collect();
    (ins, outs)
}

/// 0 inputs, 1 output; its `value` is the start of every propagation chain.
#[derive(Debug)]
struct ConstantF32 {
    meta: NodeMeta,
    inputs: Vec<InputSocket>,
    outputs: Vec<OutputSocket>,
    pub value: f32,
}

impl ConstantF32 {
    fn new(value: f32) -> Self {
        let (inputs, outputs) = node_sockets(0, 1);
        Self {
            meta: NodeMeta::new("demo.constant.f32", "Constant", 1),
            inputs,
            outputs,
            value,
        }
    }
}

impl Node for ConstantF32 {
    fn meta(&self) -> &NodeMeta {
        &self.meta
    }
    fn inputs(&self) -> &[InputSocket] {
        &self.inputs
    }
    fn outputs(&self) -> &[OutputSocket] {
        &self.outputs
    }
    fn dirty_policy(&self) -> DirtyPolicy {
        DirtyPolicy::Always
    }
    fn state_type(&self) -> Box<dyn TypeTag> {
        Box::new(NamedTypeTag::new("f32"))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// 1 input, 1 output; output = input + `b`.
#[derive(Debug)]
struct AddF32 {
    meta: NodeMeta,
    inputs: Vec<InputSocket>,
    outputs: Vec<OutputSocket>,
    pub b: f32,
    pub cached: f32,
}

impl AddF32 {
    fn new(b: f32) -> Self {
        let (inputs, outputs) = node_sockets(1, 1);
        Self {
            meta: NodeMeta::new("demo.add.f32", "Add", 1),
            inputs,
            outputs,
            b,
            cached: 0.0,
        }
    }
}

impl Node for AddF32 {
    fn meta(&self) -> &NodeMeta {
        &self.meta
    }
    fn inputs(&self) -> &[InputSocket] {
        &self.inputs
    }
    fn outputs(&self) -> &[OutputSocket] {
        &self.outputs
    }
    fn dirty_policy(&self) -> DirtyPolicy {
        DirtyPolicy::Always
    }
    fn state_type(&self) -> Box<dyn TypeTag> {
        Box::new(NamedTypeTag::new("f32"))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// 1 input, 1 output; output = sin(input). Shows a read-only preview.
#[derive(Debug)]
struct SinF32 {
    meta: NodeMeta,
    inputs: Vec<InputSocket>,
    outputs: Vec<OutputSocket>,
    pub input: f32,
    pub output: f32,
}

impl SinF32 {
    fn new() -> Self {
        let (inputs, outputs) = node_sockets(1, 1);
        Self {
            meta: NodeMeta::new("demo.sin.f32", "Sin", 1),
            inputs,
            outputs,
            input: 0.0,
            output: 0.0,
        }
    }
}

impl Node for SinF32 {
    fn meta(&self) -> &NodeMeta {
        &self.meta
    }
    fn inputs(&self) -> &[InputSocket] {
        &self.inputs
    }
    fn outputs(&self) -> &[OutputSocket] {
        &self.outputs
    }
    fn dirty_policy(&self) -> DirtyPolicy {
        DirtyPolicy::Always
    }
    fn state_type(&self) -> Box<dyn TypeTag> {
        Box::new(NamedTypeTag::new("f32"))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// 1 input, 0 outputs; shows the value it receives plus an editable label.
#[derive(Debug)]
struct DisplayF32 {
    meta: NodeMeta,
    inputs: Vec<InputSocket>,
    outputs: Vec<OutputSocket>,
    pub label: String,
    pub received: Option<f32>,
}

impl DisplayF32 {
    fn new(label: impl Into<String>) -> Self {
        let (inputs, outputs) = node_sockets(1, 0);
        Self {
            meta: NodeMeta::new("demo.display.f32", "Display", 1),
            inputs,
            outputs,
            label: label.into(),
            received: None,
        }
    }
}

impl Node for DisplayF32 {
    fn meta(&self) -> &NodeMeta {
        &self.meta
    }
    fn inputs(&self) -> &[InputSocket] {
        &self.inputs
    }
    fn outputs(&self) -> &[OutputSocket] {
        &self.outputs
    }
    fn dirty_policy(&self) -> DirtyPolicy {
        DirtyPolicy::Always
    }
    fn state_type(&self) -> Box<dyn TypeTag> {
        Box::new(NamedTypeTag::new("()"))
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

// ----------------------------------------------------------------------------
// Card view
// ----------------------------------------------------------------------------

struct DemoNodeView;

impl NodeView for DemoNodeView {
    fn header_color(&self, node: &dyn Node) -> egui::Color32 {
        match node.meta().type_name.as_str() {
            // ConstantF32 = gray-blue, AddF32 = green, SinF32 = purple,
            // DisplayF32 = orange.
            "demo.constant.f32" => egui::Color32::from_rgb(91, 123, 196),
            "demo.add.f32" => egui::Color32::from_rgb(112, 168, 96),
            "demo.sin.f32" => egui::Color32::from_rgb(176, 112, 196),
            "demo.display.f32" => egui::Color32::from_rgb(206, 141, 78),
            _ => egui::Color32::from_rgb(130, 130, 150),
        }
    }

    fn body_ui(&self, ui: &mut egui::Ui, node: &mut dyn Node) {
        let any = node.as_any_mut();
        if let Some(c) = any.downcast_mut::<ConstantF32>() {
            ui.add(egui::Slider::new(&mut c.value, 0.0..=std::f32::consts::TAU).text("value"));
        } else if let Some(a) = any.downcast_mut::<AddF32>() {
            ui.add(egui::Slider::new(&mut a.b, -5.0..=5.0).text("b"));
            ui.label(format!("out = {:.2}", a.cached));
        } else if let Some(s) = any.downcast_mut::<SinF32>() {
            ui.label(format!("x = {:.3}", s.input));
            ui.label(format!("sin = {:.3}", s.output));
        } else if let Some(d) = any.downcast_mut::<DisplayF32>() {
            ui.horizontal(|ui| {
                ui.label("label");
                ui.add(egui::TextEdit::singleline(&mut d.label).desired_width(90.0));
            });
            match d.received {
                Some(v) => {
                    ui.label(format!("value = {v:.3}"));
                }
                None => {
                    ui.label("value = —");
                }
            }
        }
    }

    fn size_hint(&self, node: &dyn Node) -> Vec2 {
        match node.meta().type_name.as_str() {
            "demo.constant.f32" => Vec2::new(170.0, 72.0),
            _ => Vec2::new(170.0, 92.0),
        }
    }
}

// ----------------------------------------------------------------------------
// Tiny forward propagation (demo-only, no eval crate involved)
// ----------------------------------------------------------------------------

/// Output value of `socket` for nodes that produce one.
fn output_value(node: &dyn Node, socket: u32) -> Option<f32> {
    if socket != 0 {
        return None;
    }
    let any = node.as_any();
    any.downcast_ref::<ConstantF32>()
        .map(|c| c.value)
        .or_else(|| any.downcast_ref::<AddF32>().map(|a| a.cached))
        .or_else(|| any.downcast_ref::<SinF32>().map(|s| s.output))
}

/// Push values along edges until nothing changes (chains are short here).
fn propagate_values(editor: &mut Editor) {
    let ids: Vec<NodeId> = editor.graph().all_nodes().collect();
    for _ in 0..=ids.len() {
        let mut changed = false;
        for &id in &ids {
            let Some(source) = editor.graph().upstream(InputSocketId::new(id, 0)).pop() else {
                continue;
            };
            let Some(value) = editor
                .graph()
                .get_node(source.node())
                .and_then(|n| output_value(n, source.index()))
            else {
                continue;
            };
            let Some(node) = editor.graph_mut().get_node_mut(id) else {
                continue;
            };
            if let Some(a) = node.as_any_mut().downcast_mut::<AddF32>() {
                let new = value + a.b;
                if (a.cached - new).abs() > 1e-6 {
                    a.cached = new;
                    changed = true;
                }
            } else if let Some(s) = node.as_any_mut().downcast_mut::<SinF32>() {
                if (s.input - value).abs() > 1e-6 {
                    s.input = value;
                    changed = true;
                }
                let out = value.sin();
                if (s.output - out).abs() > 1e-6 {
                    s.output = out;
                    changed = true;
                }
            } else if let Some(d) = node.as_any_mut().downcast_mut::<DisplayF32>()
                && d.received != Some(value)
            {
                d.received = Some(value);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

// ----------------------------------------------------------------------------
// Editor construction helpers
// ----------------------------------------------------------------------------

fn add_node(editor: &mut Editor, node: Box<dyn Node>, at: Vec2) -> NodeId {
    editor
        .apply(Command::AddNode { node, at })
        .expect("add node");
    editor
        .drain_events()
        .find_map(|e| match e {
            EditorEvent::NodeAdded(id) => Some(id),
            _ => None,
        })
        .expect("NodeAdded event")
}

fn connect(editor: &mut Editor, from: NodeId, to: NodeId) {
    editor
        .apply(Command::BeginConnection(OutputSocketId::new(from, 0)))
        .expect("begin connection");
    editor
        .apply(Command::CompleteConnection(InputSocketId::new(to, 0)))
        .expect("complete connection");
    editor.drain_events().for_each(drop);
}

fn build_editor() -> Editor {
    let graph = Box::new(SimpleGraph::default());
    let mut editor = Editor::new(
        graph,
        Box::new(FreeLayout::new()),
        Box::new(BezierGeometry::default()),
    );
    // Preset chain: Constant → Sin → Display.
    let c = add_node(
        &mut editor,
        Box::new(ConstantF32::new(1.0)),
        Vec2::new(0.0, 0.0),
    );
    let s = add_node(
        &mut editor,
        Box::new(SinF32::new()),
        Vec2::new(280.0, -30.0),
    );
    let d = add_node(
        &mut editor,
        Box::new(DisplayF32::new("result")),
        Vec2::new(560.0, 10.0),
    );
    connect(&mut editor, c, s);
    connect(&mut editor, s, d);
    editor
}

fn new_app() -> NodeGraphApp {
    NodeGraphApp::new(
        build_editor(),
        Box::new(DemoNodeView),
        Box::new(BezierRenderer),
    )
}

// ----------------------------------------------------------------------------
// eframe host
// ----------------------------------------------------------------------------

struct DemoApp {
    app: NodeGraphApp,
    /// Open spawn menu: (screen position, graph-local spawn position).
    menu: Option<(egui::Pos2, Vec2)>,
}

impl Default for DemoApp {
    fn default() -> Self {
        Self {
            app: new_app(),
            menu: None,
        }
    }
}

impl eframe::App for DemoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("hints").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    "拖拽标题移动 · 从输出端口拖到输入端口连线 · 右键空白添加节点 \
                     · 滚轮缩放 · 中键/右键平移 · Delete 删除选中 · Ctrl+Z / Ctrl+Y 撤销重做",
                );
                if ui.button("重置示例图").clicked() {
                    self.app = new_app();
                    self.menu = None;
                }
                if ui.button("适配视图").clicked() {
                    self.app.request_fit();
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let ctx = ui.ctx().clone();

            // Live values feed the read-only previews inside node bodies.
            propagate_values(self.app.editor_mut());
            let _events = self.app.show(ui);

            if let Some(graph_pos) = self.app.take_context_menu_pos() {
                let screen = self.app.canvas_mut().to_screen(graph_pos);
                self.menu = Some((screen, graph_pos));
            }

            if let Some((screen_pos, graph_pos)) = self.menu {
                let resp = egui::Area::new(egui::Id::new("spawn_menu"))
                    .fixed_pos(screen_pos)
                    .order(egui::Order::Foreground)
                    .show(&ctx, |ui| {
                        egui::Frame::menu(ui.style()).show(ui, |ui| {
                            ui.set_min_width(180.0);
                            ui.label(egui::RichText::new("添加节点").strong());
                            ui.separator();
                            let mut spawn: Option<Box<dyn Node>> = None;
                            if ui.button("Constant (常量 f32)").clicked() {
                                spawn = Some(Box::new(ConstantF32::new(0.5)));
                            }
                            if ui.button("Add (加法 f32)").clicked() {
                                spawn = Some(Box::new(AddF32::new(1.0)));
                            }
                            if ui.button("Sin (正弦 f32)").clicked() {
                                spawn = Some(Box::new(SinF32::new()));
                            }
                            if ui.button("Display (显示 f32)").clicked() {
                                spawn = Some(Box::new(DisplayF32::new("value")));
                            }
                            if let Some(node) = spawn {
                                add_node(self.app.editor_mut(), node, graph_pos);
                                self.menu = None;
                            }
                        });
                    });
                // Click outside the menu closes it.
                if ctx.input(|i| i.pointer.primary_pressed()) && !resp.response.contains_pointer() {
                    self.menu = None;
                }
            }
        });

        // Repaint continuously so animated dashes and previews stay live.
        ui.ctx().request_repaint();
    }
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "xcal node editor demo",
        options,
        Box::new(|_cc| Ok(Box::<DemoApp>::default())),
    )
}
