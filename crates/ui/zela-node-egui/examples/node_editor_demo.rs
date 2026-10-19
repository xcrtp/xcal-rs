//! Interactive node editor demo — property-driven (Phase D rewrite).
//!
//! Run with:
//! ```text
//! cargo run -p zela-node-egui --example node_editor_demo
//! ```
//!
//! Every node is `#[derive(NodeSpec)]`, the body UI is rendered by
//! [`zela_node_egui_ui::PropertyNodeView`] (no per-node `downcast_mut`), the
//! spawn menu is generated from [`ze_node_schema::NodeRegistry::categories`],
//! and forward propagation uses [`ze_graph_eval::LazyPullEval`] (the old
//! `propagate_values` O(n²) chain is gone).

use eframe::egui;
use glam::Vec2;
use ze_graph_algo::KahnSorter;
use ze_graph_eval::LazyPullEval;
use ze_node_core::{
    EvalError, EvalErrorPolicy, EvalStrategy, ExecContext, InputSocketId, Node, NodeExecutor,
    NodeId, OutputSocketId, SimpleGraph,
};
use ze_node_schema::{NodeRegistry, Property};
use zela_node_editor::{BezierGeometry, Command, Editor, EditorEvent, FreeLayout};
use zela_node_egui::{BezierRenderer, NodeGraphApp};
use zela_node_egui_ui::{PropertyNodeView, spawn_menu};

// ----------------------------------------------------------------------------
// Derived nodes — zero hand-written Node/NodeSpec boilerplate.
// ----------------------------------------------------------------------------

#[derive(Debug, Clone, ze_node_schema::NodeSpec)]
#[node(
    type_name = "demo.constant.f32",
    display = "Constant",
    category = "Source",
    outputs = ["f32"],
    state = "()",
    dirty = "always"
)]
struct ConstantF32 {
    #[property(label = "value", range = 0.0..=10.0, step = 0.01, default = 1.0)]
    value: f32,
}

#[derive(Debug, Clone, ze_node_schema::NodeSpec)]
#[node(
    type_name = "demo.add.f32",
    display = "Add",
    category = "Math",
    inputs = ["f32"],
    outputs = ["f32"],
    state = "()",
    dirty = "always"
)]
struct AddF32 {
    #[property(label = "b", range = -5.0..=5.0, step = 0.1, default = 1.0)]
    b: f32,
}

#[derive(Debug, Clone, ze_node_schema::NodeSpec)]
#[node(
    type_name = "demo.sin.f32",
    display = "Sin",
    category = "Math",
    inputs = ["f32"],
    outputs = ["f32"],
    state = "()",
    dirty = "always"
)]
struct SinF32 {}

/// Two-in / two-out node: output 0 is `a + b`, output 1 is `a * b`.
/// Showcases multi-pin nodes — the card renders two input pins on the left
/// and two output pins on the right (socket index = pin row order).
#[derive(Debug, Clone, ze_node_schema::NodeSpec)]
#[node(
    type_name = "demo.addmul.f32",
    display = "Add/Mul",
    category = "Math",
    inputs = ["f32", "f32"],
    outputs = ["f32", "f32"],
    state = "()",
    dirty = "always"
)]
struct AddMulF32 {}

/// Read-only sink: fixed-size card (no resize grip) — demonstrates the
/// per-node-type interaction metadata (`resizable = false`). Other demo
/// nodes keep the permissive default and expose the bottom-right grip.
#[derive(Debug, Clone, ze_node_schema::NodeSpec)]
#[node(
    type_name = "demo.display.f32",
    display = "Display",
    category = "Sink",
    inputs = ["f32"],
    state = "f32",
    dirty = "always",
    resizable = false
)]
struct DisplayF32 {
    #[property(label = "label", default = "value".to_string())]
    label: String,
}

// ----------------------------------------------------------------------------
// Property-driven executor — reads properties via the registry (no downcast
// in the dispatch); writes outputs/state per node type. Same pattern as the
// Phase A headless tests, now driving the live demo.
// ----------------------------------------------------------------------------

struct PropertyExecutor {
    registry: NodeRegistry,
}

impl NodeExecutor for PropertyExecutor {
    fn execute(&self, node: &dyn Node, ctx: &mut dyn ExecContext) -> Result<(), EvalError> {
        let here = ctx.current_node();
        let spec = self.registry.spec_of(node).ok_or_else(|| {
            EvalError::new(
                format!("no NodeSpec for `{}`", node.meta().type_name),
                here,
                None,
            )
        })?;
        let props = spec.properties();
        match node.meta().type_name.as_str() {
            "demo.constant.f32" => {
                let v = scalar_prop(&props, "value")?;
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(v))?;
            }
            "demo.add.f32" => {
                let input = peek_f32(ctx, here, 0)?;
                let b = scalar_prop(&props, "b")?;
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(input + b))?;
            }
            "demo.sin.f32" => {
                let input = peek_f32(ctx, here, 0)?;
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(input.sin()))?;
            }
            "demo.addmul.f32" => {
                // Two input pins (index 0 = a, index 1 = b) ...
                let a = peek_f32(ctx, here, 0)?;
                let b = peek_f32(ctx, here, 1)?;
                // ... fan out to two output pins: out 0 = sum, out 1 = product.
                ctx.set_output(OutputSocketId::new(here, 0), Box::new(a + b))?;
                ctx.set_output(OutputSocketId::new(here, 1), Box::new(a * b))?;
            }
            "demo.display.f32" => {
                let input = peek_f32(ctx, here, 0)?;
                ctx.set_state(Box::new(input))?;
            }
            other => {
                return Err(EvalError::new(
                    format!("unknown node type `{other}`"),
                    here,
                    None,
                ));
            }
        }
        Ok(())
    }
}

fn scalar_prop(props: &[(String, Property)], name: &str) -> Result<f32, EvalError> {
    for (n, p) in props {
        if n == name
            && let Property::Scalar(c) = p
        {
            return Ok(c.value);
        }
    }
    Err(EvalError::new(
        format!("missing scalar property `{name}`"),
        NodeId(0),
        None,
    ))
}

fn peek_f32(ctx: &mut dyn ExecContext, node: NodeId, idx: u32) -> Result<f32, EvalError> {
    let v = ctx.peek_input(InputSocketId::new(node, idx))?;
    v.downcast_ref::<f32>()
        .copied()
        .ok_or_else(|| EvalError::new("input not f32", node, None))
}

// ----------------------------------------------------------------------------
// Registry + editor construction
// ----------------------------------------------------------------------------

fn build_registry() -> NodeRegistry {
    let mut reg = NodeRegistry::new();
    reg.register::<ConstantF32>("Source", ConstantF32::default_node);
    reg.register::<AddF32>("Math", AddF32::default_node);
    reg.register::<SinF32>("Math", SinF32::default_node);
    reg.register::<AddMulF32>("Math", AddMulF32::default_node);
    reg.register::<DisplayF32>("Sink", DisplayF32::default_node);
    reg
}

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

/// Spawn a Constant with a preset value (property set before insertion).
fn spawn_constant(registry: &NodeRegistry, value: f32) -> Box<dyn Node> {
    let mut node = registry.spawn("demo.constant.f32").expect("spawn constant");
    if let Some(c) = node.downcast_mut::<ConstantF32>() {
        c.value = value;
    }
    node
}

/// Spawn a Display with a preset label.
fn spawn_display(registry: &NodeRegistry, label: &str) -> Box<dyn Node> {
    let mut node = registry.spawn("demo.display.f32").expect("spawn display");
    if let Some(d) = node.downcast_mut::<DisplayF32>() {
        d.label = label.to_string();
    }
    node
}

/// Connect output pin `from_socket` of `from` to input pin `to_socket` of `to`.
fn connect(editor: &mut Editor, from: NodeId, from_socket: u32, to: NodeId, to_socket: u32) {
    editor
        .apply(Command::BeginConnection(OutputSocketId::new(
            from,
            from_socket,
        )))
        .expect("begin connection");
    editor
        .apply(Command::CompleteConnection(InputSocketId::new(
            to, to_socket,
        )))
        .expect("complete connection");
    editor.drain_events().for_each(drop);
}

fn build_editor(registry: &NodeRegistry) -> Editor {
    let mut editor = Editor::new(
        Box::new(SimpleGraph::default()),
        Box::new(FreeLayout::new()),
        Box::new(BezierGeometry::default()),
    );
    // Preset chain (single-pin nodes): Constant → Sin → Display.
    let c = add_node(
        &mut editor,
        spawn_constant(registry, 1.0),
        Vec2::new(0.0, 0.0),
    );
    let s = add_node(
        &mut editor,
        registry.spawn("demo.sin.f32").unwrap(),
        Vec2::new(280.0, -30.0),
    );
    let d = add_node(
        &mut editor,
        spawn_display(registry, "sin"),
        Vec2::new(560.0, 10.0),
    );
    connect(&mut editor, c, 0, s, 0);
    connect(&mut editor, s, 0, d, 0);

    // Multi-pin branch: two Constants feed the *two input pins* of Add/Mul,
    // whose *two output pins* fan out to two separate Display nodes.
    let c2 = add_node(
        &mut editor,
        spawn_constant(registry, 1.5),
        Vec2::new(0.0, 300.0),
    );
    let c3 = add_node(
        &mut editor,
        spawn_constant(registry, 2.0),
        Vec2::new(0.0, 420.0),
    );
    let am = add_node(
        &mut editor,
        registry.spawn("demo.addmul.f32").unwrap(),
        Vec2::new(300.0, 350.0),
    );
    let d_sum = add_node(
        &mut editor,
        spawn_display(registry, "sum (a+b)"),
        Vec2::new(580.0, 300.0),
    );
    let d_prod = add_node(
        &mut editor,
        spawn_display(registry, "product (a*b)"),
        Vec2::new(580.0, 420.0),
    );
    // two distinct input pins ...
    connect(&mut editor, c2, 0, am, 0);
    connect(&mut editor, c3, 0, am, 1);
    // ... fanning out to two distinct output pins.
    connect(&mut editor, am, 0, d_sum, 0);
    connect(&mut editor, am, 1, d_prod, 0);

    editor
}

// ----------------------------------------------------------------------------
// eframe host
// ----------------------------------------------------------------------------

struct DemoApp {
    app: NodeGraphApp,
    registry: NodeRegistry,
    eval: LazyPullEval,
    /// Owned clone of the executor (kept in the app so the registry reference
    /// inside it stays live for the whole frame).
    exec: PropertyExecutor,
    /// Open spawn menu: (screen position, graph-local spawn position).
    menu: Option<(egui::Pos2, Vec2)>,
    /// Last evaluation error, if any — shown as a hint in the top panel.
    last_error: Option<String>,
}

impl Default for DemoApp {
    fn default() -> Self {
        let registry = build_registry();
        let exec = PropertyExecutor {
            registry: registry.clone(),
        };
        let eval = LazyPullEval::new(Box::new(KahnSorter::new()), EvalErrorPolicy::FailFast);
        let editor = build_editor(&registry);
        let view = Box::new(PropertyNodeView::new(registry.clone()));
        Self {
            app: NodeGraphApp::new(editor, view, Box::new(BezierRenderer)),
            registry,
            eval,
            exec,
            menu: None,
            last_error: None,
        }
    }
}

impl eframe::App for DemoApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Hints bar: content is STATIC (no conditional widgets) so its wrapped
        // height never changes with graph state. The eval error used to live
        // here as a conditional label, which changed the bar's wrapped height
        // on connect/disconnect, shifting the canvas viewport one row for a
        // frame (a visible whole-canvas jitter). It is now painted as an
        // overlay inside the canvas instead (see below).
        egui::Panel::top("hints").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    "拖拽标题移动 · 从输出端口拖到输入端口连线 · 右键空白添加节点 \
                     · 滚轮缩放 · 中键/右键平移 · Delete 删除选中 · Ctrl+Z / Ctrl+Y 撤销重做",
                );
                if ui.button("重置示例图").clicked() {
                    *self = DemoApp::default();
                }
                if ui.button("适配视图").clicked() {
                    self.app.request_fit();
                }
            });
        });

        egui::CentralPanel::default().show(ui, |ui| {
            let ctx = ui.ctx().clone();

            // Evaluate the whole graph via LazyPullEval (replaces the old
            // `propagate_values` O(n²) chain). `target = None` means "evaluate
            // every node in topo order", which is what we want for live
            // previews.
            let eval_result = self
                .eval
                .evaluate(self.app.editor().graph(), &self.exec, None);
            self.last_error = match eval_result {
                Ok(()) => None,
                Err(e) => Some(format!("{e}")),
            };

            let _events = self.app.show(ui);

            // Eval error overlay: painted onto the canvas (no layout impact),
            // so showing/hiding it never moves the viewport.
            if let Some(err) = &self.last_error {
                let painter = ui.painter();
                let anchor = ui.max_rect().left_top() + egui::vec2(10.0, 10.0);
                let galley = painter.layout_no_wrap(
                    format!("eval: {err}"),
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_rgb(235, 90, 90),
                );
                let bg = ui.visuals().window_fill.gamma_multiply(0.85);
                painter.rect_filled(
                    egui::Rect::from_min_size(anchor, galley.size()).expand(6.0),
                    4.0,
                    bg,
                );
                painter.galley(
                    anchor + egui::vec2(6.0, 6.0),
                    galley,
                    egui::Color32::from_rgb(235, 90, 90),
                );
            }

            if let Some(graph_pos) = self.app.take_context_menu_pos() {
                let screen = self.app.canvas_mut().to_screen(graph_pos);
                self.menu = Some((screen, graph_pos));
            }

            if let Some((screen_pos, graph_pos)) = self.menu {
                let registry = &self.registry;
                let resp = egui::Area::new(egui::Id::new("spawn_menu"))
                    .fixed_pos(screen_pos)
                    .order(egui::Order::Foreground)
                    .show(&ctx, |ui| {
                        let inner = egui::Frame::menu(ui.style()).show(ui, |ui| {
                            ui.set_min_width(180.0);
                            ui.label(egui::RichText::new("添加节点").strong());
                            ui.separator();
                            spawn_menu(ui, registry)
                        });
                        inner.inner
                    });
                if let Some(type_name) = resp.inner {
                    if let Some(node) = self.registry.spawn(&type_name) {
                        add_node(self.app.editor_mut(), node, graph_pos);
                    }
                    self.menu = None;
                }
                if ctx.input(|i| i.pointer.primary_pressed()) && !resp.response.contains_pointer() {
                    self.menu = None;
                }
            }
        });

        // Repaint continuously so live edits stay responsive.
        ui.ctx().request_repaint();
    }
}

// ----------------------------------------------------------------------------
// CJK font setup — egui's built-in fonts carry no CJK glyphs, so Chinese
// labels would render as tofu boxes. Register a system font as a *fallback*
// (appended at the end of each family: Latin still uses the default fonts,
// CJK falls through to this one). Silently keeps the defaults when no
// candidate font exists.
//
// The `C:\Windows\Fonts\...` entries only exist on Windows; on Linux / macOS
// they are skipped by the `std::fs::read` existence filter below, so this list
// is intentionally platform-agnostic (it is a candidate list, not a config).
// ----------------------------------------------------------------------------

const CJK_FONT_CANDIDATES: &[&str] = &[
    // Windows
    r"C:\Windows\Fonts\msyh.ttc",   // Microsoft YaHei
    r"C:\Windows\Fonts\msyh.ttf",   // Microsoft YaHei (Win7)
    r"C:\Windows\Fonts\simhei.ttf", // SimHei
    r"C:\Windows\Fonts\simsun.ttc", // SimSun
    // macOS
    "/System/Library/Fonts/PingFang.ttc",
    // Linux
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
];

fn install_cjk_fonts(ctx: &egui::Context) {
    let Some(bytes) = CJK_FONT_CANDIDATES
        .iter()
        .find_map(|p| std::fs::read(p).ok())
    else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "cjk_fallback".into(),
        egui::FontData::from_owned(bytes).into(),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("cjk_fallback".into());
    }
    ctx.set_fonts(fonts);
}

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions::default();
    eframe::run_native(
        "xcal node editor demo",
        options,
        Box::new(|cc| {
            install_cjk_fonts(&cc.egui_ctx);
            Ok(Box::<DemoApp>::default())
        }),
    )
}
