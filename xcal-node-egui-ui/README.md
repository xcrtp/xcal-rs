# xcal-node-egui-ui

> 为 `xcal-node-ui` 属性模型提供的 **egui 控件映射层**。
>
> 位于 `xcal-node-ui`（属性模型）与 `xcal-node-egui`（canvas +
> `NodeView` trait）之上，提供三样东西：
>
> - `draw_property` — 按 `Property` 变体分发绘制 egui 控件，返回
>   `Option<PropertyValue>` 由调用方调用 `NodeSpec::apply_property` 写入；
>   **全程不发生任何 downcast**。
> - `PropertyNodeView` — 一个通用 `NodeView` 实现，基于 `&NodeRegistry`
>   自动把任意注册过的节点 body 画成属性表单。
> - `spawn_menu` — 从 `NodeRegistry::categories()` 生成右键"添加节点"菜单。
>
> **依赖方向承诺**：本 crate 只依赖 `xcal-node-ui + xcal-node-egui + egui`，
> egui 只在这一层以上出现。

```
xcal-node-ui (属性/Registry)
      │
      ▼
xcal-node-egui-ui  ← 本 crate — draw_property / PropertyNodeView / spawn_menu
      │
      ▼
node_editor_demo 或任何最终 egui 应用
```

---

## 1. `draw_property`：属性 → egui 控件

```rust
pub fn draw_property(
    ui:       &mut egui::Ui,
    name:     &str,
    prop:     &Property,
) -> Option<PropertyValue>;
```

| Property 变体 | 绘制的 egui 控件 | 交互即返回新值 |
|---|---|---|
| `Scalar` / `Double` | `ui.add(egui::DragValue::new(&mut v).clamp_range(min..=max).speed(step))` — 数字拖拽；如果 range + step 等价于 slider 且范围有限，回退到滑块 | ✅ |
| `Integer` | `DragValue::new(&mut v).prefix("int ").clamp_range(...)` | ✅ |
| `Bool`    | `ui.checkbox(&mut v, label)` | ✅ |
| `String`  | `TextEdit::singleline / multiline`（按 `StringKind`） | ✅（失焦或 Enter 提交） |
| `VecN`(2..4) | N 个横向 DragValue + 颜色条 `egui::color::HsvaGamma` 预览 | ✅（任何分量变即返回）|
| `Color`   | `egui::color_picker::color_edit_button_srgba`（可开关 alpha） | ✅ |
| `Enum`    | `egui::ComboBox::from_label(label).selected_text(...).show_ui` | ✅ |
| `Range`   | 上下两个 `DragValue`（lo / hi），自动强制 lo ≤ hi | ✅ |
| `Function`| 只读标签（`handle.to_string()`，不可直接编辑） | ❌ |
| `Dynamic` | 嵌套 `egui::CollapsingHeader`，内部递归 `draw_property` 每个 `DynamicField` | ✅（字段级）|
| `List`    | `egui::Grid` + 每行一个 element；顶端"增/删行"按钮 | ✅（按行） |

**返回约定**：返回 `None` 表示用户当前没动这个控件；返回 `Some(new_value)`
表示发生了变化，宿主**必须**用 `node_spec.apply_property(name, new_value)`
写回——把写操作外置，能保证 `PropertyNodeView` 本身不需要 `&mut dyn NodeSpec`
（避免再引入一次 trait upcast 问题）。

---

## 2. `PropertyNodeView`：通用 `NodeView` 实现

```rust
pub struct PropertyNodeView<'a> {
    registry: &'a NodeRegistry,
    // 内部：每个节点上次 apply_property 结果缓存 + height 估算
}
impl<'a> NodeView for PropertyNodeView<'a> { /* ... */ }
```

工作流程：

```
NodeGraphApp::ui()
  → GraphPainter::paint(ui, editor, canvas, &mut property_node_view, &edges)
      → 对每个 node:
          view.title(node)            → node.meta().display_name
          view.title_bar_color(node,selected) → category 调色板
          view.body_height(ui,node)   → 根据 registry.properties().len() × row_height
          view.body_ui(ui, &mut node, events)
              ┌─ 调 registry.properties(&*node) 拿 schema
              └─ 每个字段: draw_property(ui, name, schema)
                   → Some(v) → apply_property → push EditorEvent::PropertyChanged
                   → None    → 无事
```

### 标题配色（按 category）

`category` → `HSL(hue, 0.55, 0.4)` 循环，避免相邻类别色撞；
`Source = 蓝` / `Math = 紫` / `Display = 绿` / `Transform = 橙`
可在 `PropertyNodeView::new_with_palette(registry, palette_map)` 时自定义。

### 与自定义视图组合

需要特殊 body（比如图像预览、文本大编辑器）？
**不是继承**而是用"链"：

```rust
struct MyView<'a> { base: PropertyNodeView<'a>, /* custom state */ }
impl NodeView for MyView<'_> {
    fn body_ui(&mut self, ui: &mut egui::Ui, n: &mut dyn Node, ev: &mut Vec<EditorEvent>) {
        match n.meta().type_name {
            "app.preview.image" => self.draw_image_preview(ui, n, ev),
            _                   => self.base.body_ui(ui, n, ev), // 默认走属性表单
        }
    }
    // title / header_color / body_height 同理 fallthrough
}
```

---

## 3. `spawn_menu`：右键添加节点

```rust
pub fn spawn_menu(
    ui:          &mut egui::Ui,
    registry:    &NodeRegistry,
    screen_pos:  egui::Pos2,    // 菜单锚点（右键位置，转换为 egui 坐标系）
    on_spawn:    impl FnMut(&RegistryNodeInfo) -> Command,
) -> Option<Command>;
```

菜单结构：

```
┌─ Source ───────┐
│  Constant f32  │  ← on_spawn(info)
│  Constant i32  │
├─ Math ─────────┤
│  Add f32       │
│  Sin f32       │
├─ Display ──────┤
│  Display f32   │
└────────────────┘
```

返回 `Some(command)` 时，UI 层调用 `editor.execute(command)?`。
`node_editor_demo` 默认把它作为"右键空白处"菜单；也可绑定快捷键或工具栏按钮。

---

## 完整示例（与 node_editor_demo 相同）

```rust
use eframe;
use xcal_node::{Node, NodeRegistry};
use xcal_node_ui_macros::NodeSpec;
use xcal_node_editor::Editor;
use xcal_node_egui::{BezierRenderer, NodeGraphApp};
use xcal_node_egui_ui::{PropertyNodeView, spawn_menu};

#[derive(NodeSpec)]
#[node(type_name = "demo.const.f32", display = "Constant",
       category = "Source", outputs = ["f32"])]
struct ConstantF32 {
    #[property(label = "value", range = 0.0..=10.0, step = 0.01, default = 3.14)]
    value: f32,
}
// (AddF32 / SinF32 / DisplayF32 同理，共 4 个)

fn build_app(cc: &eframe::CreationContext) -> Box<dyn eframe::App> {
    // 1) Registry
    let mut reg = NodeRegistry::new();
    reg.register::<ConstantF32>(ConstantF32::CATEGORY, ConstantF32::default_node);
    reg.register::<AddF32>     (AddF32::CATEGORY,      AddF32::default_node);
    reg.register::<SinF32>     (SinF32::CATEGORY,      SinF32::default_node);
    reg.register::<DisplayF32> (DisplayF32::CATEGORY,  DisplayF32::default_node);

    // 2) 预置链：Const → Add → Sin → Display
    let mut editor = /* build with reg.spawn(...) + connect */;

    // 3) 视图
    let view = PropertyNodeView::new(&reg);
    let edge = BezierRenderer::default();

    // 4) 交给 app
    Box::new(NodeGraphApp::new(editor, view, edge))
}

fn main() {
    eframe::run_native("Node Demo", Default::default(),
                       Box::new(|cc| build_app(cc))).unwrap();
}
```

（详见 `xcal-node-egui/examples/node_editor_demo.rs`，共 302 行——
用 `PropertyNodeView` + derive 后比原手写缩短 40%）。

---

## 依赖

```toml
xcal-node        = { path = "../xcal-node" }
xcal-node-ui     = { path = "../xcal-node-ui" }
xcal-node-egui   = { path = "../xcal-node-egui" }
egui             = "0.36"
glam             = "0.33"
# dev
eframe = "0.36" (features=["default_fonts"])
```

## 测试

```bash
cargo test -p xcal-node-egui-ui
# 真正的集成在示例中验证：
cargo run  -p xcal-node-egui --example node_editor_demo --features default
```

headless 单测覆盖纯函数：
- `draw_property` 在"无 egui 渲染上下文"模式下用占位 Ui 返回值（None）；
- `PropertyNodeView` 对 4 种 demo 节点计算的 body_height 符合 row_count×row_height；
- `spawn_menu` 对空 registry、单类别、多类别生成的 `MenuButton` 数量。
