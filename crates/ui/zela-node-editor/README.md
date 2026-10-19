# zela-node-editor

> **纯逻辑节点编辑器状态机**。位于领域模型（ze-node-core）与任何 UI 层之间。
>
> **硬约束**：**绝不引入 egui / eframe，绝不依赖 zela-scene/zela-render**。
> 保证可被 CLI、WASM、移动端等非 egui 前端复用。
>
> 它的坐标空间永远是"图本地"（`glam::Vec2`，左上原点、y 下、1:1 像素），
> 不关心 pan/zoom（那是 UI 层 canvas 的事）。

```
ze-node-core + ze-graph-algo
          │
          ▼
   zela-node-editor  ← 本 crate (Editor / Command / Undo / Validation)
          │
          ▼
   zela-node-egui / zela-node-egui-ui (UI 层)
```

---

## 模块架构

```
geometry       Rect / HitTarget / 命中常量 (SOCKET_HIT_RADIUS, SOCKET_SNAP_RADIUS,
               RESIZE_HIT_RADIUS)
hit_test       HitTestPolicy trait + HitTestContext + DefaultHitTestPolicy（可插拔命中策略）
layout         LayoutStrategy / FreeLayout / NodeSizeMap
socket_layout  SocketLayout / DefaultSocketLayout + 卡片几何常量
edge_geom      EdgePath / EdgeGeometry { BezierGeometry, OrthogonalGeometry }
command        Command (MoveNode / ResizeNode / Connect / Disconnect / Delete / …)
event          EditorEvent (NodeAdded / EdgeAdded / NodeMoved / NodeResized / UndoRedo / …)
error          EditorError (Cycle / TypeMismatch / UnknownNode / SocketMissing)
editor         Editor 状态机
validation     ConnectionValidator（类型兼容 + 环检测）
undo           UndoStack（向量栈 + cursor，支持 drop tail on 新分支）
```

---

## Editor：公共 API 速览

`Editor` 持有 1 张 `Graph`、1 个 `CommandBuffer`、1 个 `UndoStack<Command>`、
1 个 `LayoutStrategy`、1 个 `ConnectionValidator`、以及 UI 层需要的
`hover / selection / pending_connection / viewport_rect` 纯状态。

```rust
impl Editor {
    // ── 构造 ──
    pub fn new(graph: impl Graph + 'static) -> Self;
    pub fn with_layout(graph: impl Graph + 'static, layout: impl LayoutStrategy + 'static) -> Self;

    // ── 图查询 ──
    pub fn graph(&self) -> &dyn Graph;
    pub fn graph_mut(&mut self) -> &mut dyn Graph;
    pub fn node_rect(&self, id: NodeId) -> Option<Rect>;

    // ── 命中测试（委托给可插拔 HitTestPolicy）──
    pub fn hit_test(&self, pos: Vec2) -> HitTarget;
    // HitTarget::{Node(NodeId), InputSocket(InputSocketId),
    //             OutputSocket(OutputSocketId), ResizeHandle(NodeId),
    //             Edge(EdgeId), Nothing}
    pub fn hit_policy(&self) -> &dyn HitTestPolicy;
    pub fn set_hit_policy(&mut self, policy: Box<dyn HitTestPolicy>);

    // ── 状态：选择 / 悬停 / 待连线 ──
    pub fn select(&mut self, id: NodeId);
    pub fn toggle_select(&mut self, id: NodeId);
    pub fn clear_selection(&mut self);
    pub fn selection(&self) -> &BTreeSet<NodeId>;

    pub fn set_hover(&mut self, target: HitTarget);
    pub fn pending_connection(&self) -> Option<&PendingConnection>;

    // ── 命令：原子修改 + 撤销 ──
    pub fn execute(&mut self, cmd: Command) -> Result<Vec<EditorEvent>, EditorError>;
    pub fn undo(&mut self)    -> Option<Vec<EditorEvent>>;
    pub fn redo(&mut self)    -> Option<Vec<EditorEvent>>;

    // ── 布局 ──
    pub fn relayout(&mut self, sizes: &NodeSizeMap);
}
```

### Command（核心原子操作，都可逆）

| Variant | 说明 |
|---------|------|
| `AddNode { node, pos }` | 插入节点并放初始位置；undo = `RemoveNode` |
| `RemoveNode { id }` | 删除节点 + 所有连线；undo 时恢复节点 + 每条边 |
| `MoveNode { id, from, to }` | 平移；undo 用 `from/to` 对调；同节点连续移动合并为一次撤销 |
| `SetNodeSize { node, size }` | UI 层首次绘制节点时播种 `size_hint` 初始卡片尺寸；此时无旧值、不产生逆命令，故不进撤销栈 |
| `ResizeNode { node, size }` | 用户拖拽握把 resize；执行时按 `NodeInteraction::clamp_size` 钳到 min/max；同节点连续 resize 与 MoveNode 一样合并为一次撤销，发出 `NodeResized` 事件 |
| `Batch(Vec<Command>)` | 多命令打包；undo 时反向执行 |
| `Connect { from, to }` | 建立连线；校验类型 + 环不通过 → `EditorError` 不提交 |
| `Disconnect { edge_id }` | 断连 |
| `SetProperty { id, name, value }` | 设置节点属性（配合 NodeSpec）；undo 用旧值回滚 |
| `ReplaceGraph { new_graph }` | 整张图替换（用于 File → Open） |

### UndoStack

```rust
let mut s = UndoStack::<Command>::new();
s.push(cmd1); s.push(cmd2);
s.undo(&mut editor);  // 撤销 cmd2
s.undo(&mut editor);  // 撤销 cmd1
s.redo(&mut editor);  // 重做 cmd1
s.push(cmd_new);      // 推入新分支 → 原 cmd2 被截断丢弃
```

---

## ConnectionValidator（连接校验）

```rust
let v = ConnectionValidator::new();
let result: Result<EdgeId, EditorError> = v.validate(
    &editor.graph(),
    OutputSocketId::new(src, 0),
    InputSocketId::new(dst, 1),
);
```

校验项（短路径拒绝 → 提交失败）：

| 检查项 | 错误 |
|--------|------|
| 输入 socket 已被占用（默认单输入；若设置 allow_multi=false 则拒绝多对一） | `EditorError::SocketOccupied` |
| `TypeCast::can_cast(from_type, to_type)` = false | `EditorError::TypeMismatch` |
| `TarjanSccDetector.detect(pre_connect)` 出现环 | `EditorError::Cycle(Vec<NodeId>)` |
| 端点类型不匹配（Output→Input） | `EditorError::SocketDirection` |

类型转换策略默认 `DefaultTypeCast`：相同 TypeTag.name 才允许。
**自定义 TypeCast 注入到 Validator 可实现 `f32 → f64` 这类隐式提升。**

---

## Layout & SocketLayout

- `LayoutStrategy::layout(&mut self, graph: &dyn Graph, sizes: &NodeSizeMap) -> HashMap<NodeId, Vec2>`
  - `FreeLayout`：保持用户位置不动（默认）。
  - `#[feature = "auto-layout"]`（Stage 10 可选）：基于 Kahn 拓扑的分层布局。
- `SocketLayout::input_offset / output_offset(idx, count, card_size) -> Vec2`：
  返回**节点卡片局部坐标**下的端口像素位置，UI 层用它绘制连线端点。
  pin 全部位于 **header 下方的专用 pin 区域**（不在 header 上），输入左缘、
  输出右缘，按索引逐行排布；UI 层在 pin 内侧绘制等宽字体的类型标签。
- 默认常量（保证 UI 层与编辑器 hit-test 一致）：
  - `CARD_HEADER_HEIGHT = 24.0`
  - `SOCKET_ROW_HEIGHT  = 24.0`
  - `SOCKET_TOP_MARGIN  = 8.0`
  - `SOCKET_BOTTOM_GAP  = 6.0`
  - `SOCKET_HIT_RADIUS  = 12.0`（pin 点击/连线拾取半径）
  - `SOCKET_SNAP_RADIUS = 22.0`（拖线时类型兼容 pin 的磁吸半径）
  - `RESIZE_HIT_RADIUS  = 14.0`（右下角 resize 握把拾取半径）

---

## HitTestPolicy（可插拔命中策略）

命中检测不再是 `Editor` 的硬编码逻辑，而是策略对象，宿主可整体替换：

```rust
pub trait HitTestPolicy: Send + Sync + 'static {
    fn hit_test(&self, ctx: &HitTestContext<'_>, pos: Vec2) -> HitTarget;
}

// 注册自定义策略（如自定义命中区、调整优先级、按全局开关禁用交互）：
editor.set_hit_policy(Box::new(MyPolicy));
```

- `HitTestContext` 通过闭包暴露 graph / 卡片矩形 / socket 位置 / 边路径与
  各命中半径，策略与状态机解耦、可独立单测。
- `DefaultHitTestPolicy` 为优先级链（每级 miss 才进入下一级）：
  **输出 pin → 输入 pin → resize 握把 → 卡片 → 边**；
  其中 resize 握把只对 `meta.interaction.resizable == true` 的节点类型生效，
  后插入的节点（NodeId 更大）在重叠时优先命中（与绘制顺序一致）。
- 磁吸：`Editor::snap_target(cursor, from) -> Option<(InputSocketId, Vec2)>`
  在 `SOCKET_SNAP_RADIUS` 内返回距光标最近的**类型兼容**输入 pin 及其位置；
  UI 层把线端点落在 pin 上并显示金色高亮 halo，松手即使指针略偏也能连上。

> 这些常量同时被 `zela-node-egui::socket_layout` 重新导出，方便 UI 层依赖。

---

## EdgeGeometry（贝塞尔 / 正交）

`Editor` 在渲染时调用：

```rust
let geom = BezierGeometry::default();  // 或 OrthogonalGeometry::default()
let path: EdgePath = geom.build(
    output_global_pos,   // 起点（egui 屏幕坐标，由 UI 层换算）
    input_global_pos,    // 终点
);
// EdgePath { points: Vec<Vec2> } — UI 层直接画多边形 / cubic strip
```

- `BezierGeometry`：三次贝塞尔，控制点在 `start+dx/0, end-dx/0`；
- `OrthogonalGeometry`：先横后纵再横（L 形），对布局整齐的视觉图更干净。

---

## 依赖

```toml
ze-node-core       = { path = "../ze-node-core" }
ze-graph-algo = { path = "../ze-graph-algo" }
glam            = "0.33"
thiserror       = "2.0"

[features]
auto-layout = []   # Stage 10 可选：基于 Kahn 拓扑的自动布局（WIP）
```

## 测试

```bash
cargo test -p zela-node-editor
```

覆盖：
- Command 原子性：execute + undo + redo 后图完全等价（状态机断言）
- 连接校验：正向通过 / 反向拒绝（类型 / 环 / 占用）
- HitTest 边界：socket 半径内外、edge 近远、node 内外
- UndoStack：截断语义——在 redo 链中 push 新命令会丢弃原 redo 尾
