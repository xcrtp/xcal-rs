# xcal-node-egui

> **egui 渲染与交互层**：把 `xcal-node-editor` 的纯逻辑状态机
> （图、命令、撤销、连接校验）绘制成可交互的节点画布。
>
> 本 crate 不依赖 `xcal-core / xcal-render / xcal-app`，可独立于主应用使用。

```
xcal-node  ──►  xcal-node-editor  ──►  xcal-node-egui  ← 本 crate
                    (Editor)              │
                                          ├ NodeView trait（卡片外观）
                                          ├ GraphPainter （整图画线/画卡片）
                                          ├ GraphCanvas （pan / zoom）
                                          ├ InputHandler （pointer + key → Command）
                                          └ NodeGraphApp （eframe 驱动主循环）
```

## 运行示例

```bash
cargo run -p xcal-node-egui --example node_editor_demo
```

示例预置了 Constant/Add/Sin/Mul/Display 等节点：既有单 pin 的
`Constant → Sin → Display` 链路，也有两进两出的 `AddMulF32` 演示节点
（两个输入 pin 汇入、sum/product 两个输出 pin 扇出到两个 Display），
打开即可见贝塞尔连线与实时求值预览。

### 操作方式

| 操作 | 效果 |
|------|------|
| 拖拽节点标题栏 | 移动节点（连线跟随）；`interaction.draggable = false` 的节点类型不响应 |
| 拖拽卡片右下角握把 | resize 卡片（光标变为斜向 ↔）；`interaction.resizable = false` 的类型无握把，如 demo 的 Display |
| 从输出端口拖到输入端口 | 建立连线；接近类型兼容的输入 pin 时自动磁吸（金色 halo）；拖到空白处取消 |
| 类型不匹配 / 连成环时 | 光标变红 → 拒绝连接（不参与磁吸） |
| 右键空白处 | 弹出"添加节点"菜单（`xcal-node-egui-ui::spawn_menu` 增强版） |
| 滚轮 | 以鼠标为锚点缩放（0.2× ~ 5×） |
| 中键 / 右键拖拽 | 平移画布 |
| 选中节点后 `Delete` / `Backspace` | 删除节点及其连线 |
| `Ctrl+Z` | 撤销（移动/resize 整段手势合并为一次） |
| `Ctrl+Y` / `Ctrl+Shift+Z` | 重做 |
| 顶部按钮栏 | 重置示例图 / 适配视图 |

> **防抖动约定**：画布视口每帧取面板剩余空间（`ui.max_rect()`），因此
> 任何**条件渲染的 UI 元素都不要放在画布上方的布局面板里**（如按图状态
> 显示/隐藏的错误标签会改变面板高度、导致整个画布跳一帧）。状态类信息
> 应在画布内以 `painter` 覆盖绘制（demo 的 eval 错误提示即如此处理）。

---

## 关键类型

### `GraphCanvas` — 视口 pan/zoom

```rust
pub struct GraphCanvas {
    pub zoom:   f32,
    pub offset: Vec2,
}
impl GraphCanvas {
    pub fn screen_to_graph(&self, screen: Vec2) -> Vec2;
    pub fn graph_to_screen(&self, graph:  Vec2) -> Vec2;
    pub fn screen_delta_to_graph(&self, screen: Vec2) -> Vec2; // 不考虑 offset
    pub fn pan(&mut self, screen_delta: Vec2);
    pub fn zoom_at(&mut self, screen_anchor: Vec2, factor: f32);
}
```

**zoom_at 数学**：`new_offset = anchor - (anchor - offset) * factor`，保证
`screen_anchor` 缩放后在画面中的像素位置不变（直觉缩放）。

### `NodeView` trait —— 卡片外观接口

```rust
pub trait NodeView {
    fn title_bar_color(&self, node: &dyn Node, selected: bool) -> egui::Color32;
    fn title(&self, node: &dyn Node) -> String;
    fn body_height(&self, ui: &egui::Ui, node: &dyn Node) -> f32;
    fn body_ui(&mut self, ui: &mut egui::Ui, node: &mut dyn Node, events: &mut Vec<EditorEvent>);
}
```

- `DefaultNodeView`：白底 + 深灰标题 + 单行 body text = `node.meta().display_name`
- `xcal-node-egui-ui::PropertyNodeView`：根据 `NodeSpec::properties()` 自动
  绘制滑块 / 颜色选择 / 下拉框等（推荐用于绝大多数节点）。

### `EdgeRenderer` —— 画线风格

```rust
pub trait EdgeRenderer {
    fn draw_edge(&self, painter: &egui::Painter, style: &EdgeStyle,
                 from: Pos2, to: Pos2);
}
```

内建实现：
- `BezierRenderer`（默认）：平滑三次贝塞尔；`ArrowStyle::Triangle` 箭头（`EdgeStyle { stroke, stroke_kind: Solid/Dashed, arrow_head: bool }`）
- `OrthogonalRenderer`：L 形两折，适合整齐布局。

### `InputHandler` —— 输入→命令

```rust
pub fn handle(&mut self, ui: &egui::Ui, editor: &mut Editor,
              canvas: &mut GraphCanvas, view: &mut dyn NodeView) -> Vec<EditorEvent>;
```

状态机（命中目标来自 editor 的可插拔 `HitTestPolicy`）：
```
Idle ── left_down(Node header, draggable) ──► DraggingNode → MoveNode
Idle ── left_down(ResizeHandle) ────────────► Resizing     → ResizeNode（按元数据钳制）
Idle ── left_down(Socket, output=true) ─────► PendingConnect(from)
PendingConnect ── motion ──► snap_target：22px 内吸附最近兼容输入 pin
PendingConnect ── left_up(input pin) ───────► execute(Connect)
Idle ── middle/right_down ──► Panning
Idle ── wheel ──► canvas.zoom_at
Idle ── Ctrl+Z / Ctrl+Y ──► undo / redo
Selected ── Delete ──► execute(RemoveNode ×N)
```
卡片点击是否可选中/可拖拽、握把是否存在，均读节点
`meta.interaction`（`selectable / draggable / resizable`）。

### `GraphPainter` —— 整图绘制

```rust
pub fn paint(&self, painter: &egui::Painter,
             editor: &Editor, canvas: &GraphCanvas,
             view: &mut dyn NodeView, edge_renderer: &dyn EdgeRenderer);
```

绘制顺序（保证覆盖正确）：

1. 背景网格（基于 canvas.zoom 三档）
2. 所有 edges（包括悬停 / pending_connection 预览线）
3. 所有 node cards（按 graph node 顺序）
   - frame → header 标题 → pin 区（header 下方逐行排布，pin 内侧绘类型标签，
     输入左缘/输出右缘）→ 分隔线 → body 属性控件（左上角对齐、宽度填满卡片，
     随 resize 实时回流）→ 右下角 resize 握把三道斜杠（仅 resizable 节点）
   - body 子 Ui 用 `scope_builder(UiBuilder::new().max_rect(body_rect)
     .layout(Layout::top_down(Align::Min)))` 绑定，并把 clip rect 钳在卡片内

### `NodeGraphApp` —— eframe 驱动

```rust
pub struct NodeGraphApp<V: NodeView, E: EdgeRenderer> {
    editor:    Editor,
    canvas:    GraphCanvas,
    input:     InputHandler,
    painter:   GraphPainter,
    view:      V,
    edge_rend: E,
}
impl<V, E> eframe::App for NodeGraphApp<V, E> { ... }
```

示例 `node_editor_demo.rs` 用 `NodeGraphApp<PropertyNodeView, BezierRenderer>`
（完整链路见 xcal-node-egui-ui）。

---

## 依赖

```toml
xcal-node        = { path = "../xcal-node" }
xcal-node-editor = { path = "../xcal-node-editor" }
egui             = "0.36"
glam             = "0.33"
# dev
eframe = "0.36" + xcal-node-ui + xcal-node-ui-macros + xcal-node-egui-ui
       + xcal-graph-eval + xcal-graph-algo
```

## 测试

```bash
cargo test -p xcal-node-egui
```

UI 渲染本身无法 headless 断言，但对以下纯函数部分做了单测：
- `GraphCanvas::screen_to_graph / zoom_at / pan` 数值一致性；
- `InputHandler::handle` 输入序列 replay → 产生的 Command 序列断言；
- `BezierRenderer` 端点与切线方向（`EdgePath` 起点=from，末点=to）。
