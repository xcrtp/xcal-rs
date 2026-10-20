# ze-node-core

> **节点编辑器核心 trait 层（纯接口）**。
> 定义"世界长什么样"——值类型、Socket、Edge、Node、Graph、执行上下文——
> **不绑定任何具体算法、存储策略或 UI 后端**。

刻意保持**零项目内部依赖**（除了 `serde`/`thiserror`/`derive_more` 通用库），
可作为独立节点图内核在任何项目中复用（WASM、桌面、CLI、服务器都行）。

```
ze-node-core (trait + 数据模型 kernel)
      │
      ├── ze-graph-algo  (TopoSort / DirtyTracker / CycleDetector 实现)
      ├── ze-graph-eval  (LazyPullEval 求值策略)
      ├── ze-node-schema     (Property / NodeSpec / DynamicObject 属性模型)
      └── zela-node-editor (Editor 状态机 + 撤销)
              │
              zela-node-egui (egui 画布)
```

---

## 核心 trait / 类型

### 值与类型系统

| 类型 | 说明 |
|------|------|
| `NodeValue` | `dyn Any + Send + Sync + TypeTag` 的类型擦除容器，附 `downcast_ref/mut`、`clone_box`、`default_value` |
| `TypeTag` trait | `fn name(&self) -> &str`；提供 `NamedTypeTag::new("f32")` 与 `DefaultTypeCast` 默认 cast 器 |
| `TypeCast` trait | `fn can_cast(&self, from: &TypeTag, to: &TypeTag) -> bool` + 实际转换，用于跨 socket 的**隐式类型转换** |

### 节点 / 端口 / 连线

| 类型 / trait | 说明 |
|------|------|
| `InputSocket { tag, index }` + `OutputSocket { tag, index }` | 带 TypeTag 的端口声明；以 `SocketId(NodeId, u32)` 标识实例 |
| `Node` trait | `meta / inputs / outputs / dirty_policy / state_type / as_any / as_any_mut`；可选 `DirtyFn` + `DirtyPolicy::{Always, Never, OnInputChange}` |
| `NodeMeta` | `{ type_name, display_name, version, interaction }`；`type_name` 是跨序列化稳定标识；`interaction: NodeInteraction` 携带该节点类型的编辑器交互策略 |
| `NodeInteraction` | `{ draggable, selectable, resizable, min_size, max_size }`（`min/max_size` 为 `[f32; 2]` 图本地尺寸，避免引入向量依赖）；`Default` 全开（120×64 ~ 1200×900），提供 `locked()`、链式 builder（`resizable(false)` 等）与 `clamp_size(w, h)`；`#[serde(default)]` 兼容旧数据。派生宏可用 `#[node(resizable = false, …)]` 声明 |
| `Edge { id, from, to }` | 连接 `OutputSocketId → InputSocketId`；`UnitEdge` 简化单输出形式 |
| `NodeId / InputSocketId / OutputSocketId / SocketId` | 全局唯一 id 封装（支持 serde） |

### 图 (Graph) 接口

```rust
pub trait Graph {
    fn add_node(&mut self, node: Box<dyn Node>) -> NodeId;
    fn remove_node(&mut self, id: NodeId) -> Option<Box<dyn Node>>;
    fn node(&self, id: NodeId) -> Option<&dyn Node>;
    fn node_mut(&mut self, id: NodeId) -> Option<&mut dyn Node>;

    fn connect(&mut self, from: OutputSocketId, to: InputSocketId)
        -> Result<EdgeId, ConnectError>;
    fn disconnect(&mut self, edge: EdgeId) -> Result<(), ConnectError>;

    fn edges_from(&self, out: OutputSocketId) -> impl Iterator<Item = EdgeId> + '_;
    fn edges_into(&self, inp: InputSocketId) -> impl Iterator<Item = EdgeId> + '_;
    fn edge(&self, e: EdgeId) -> Option<&Edge>;
}
```

默认实现 `SimpleGraph`（内部是 `HashMap<NodeId, Box<dyn Node>>` + 双邻接表），
满足大多数场景。可用**自定义实现**替换（持久化、索引化、远端代理……）。

### 执行 (Executor)

| Trait | 说明 |
|-------|------|
| `NodeExecutor` | `fn execute(&mut self, ctx: &mut dyn ExecContext) -> Result<(), EvalError>`；类型擦除 |
| `TypedNodeExecutor<S: Node>` | `fn typed_execute(&mut self, state: &mut S, ctx: &mut dyn ExecContext) -> ...`；`TypedExecutorAdapter` 自动桥接 |
| `EvalStrategy` enum | `Lazy` / `Eager` / `Incremental` — 交给上层调度器（见 ze-graph-eval） |
| `ExecutorFactory` | `DynNodeExecutor` 注册表：按 `NodeMeta.type_name` 映射到 `Box<dyn NodeExecutor>` |
| `ExecContext` | 运行时上下文：`read_input/write_output/state/read_resource/request_eval` 等 |

### 错误 / 恢复

```rust
pub enum EvalError {
    MissingInput  { input: InputSocketId },
    TypeMismatch  { expected: String, got: String },
    NodeNotFound  { id: NodeId },
    Custom        { source: Box<dyn Error + Send + Sync> },
}
pub enum RecoveryAction { Stop, UseDefault, Retry(usize), Replace(NodeValue) }
pub trait RecoveryHandle { fn handle(&self, node: NodeId, err: &EvalError) -> RecoveryAction; }
```

提供 `EvalErrorPolicy` enum 与默认策略（StopOnFirst / UseDefault / SkipBadNodes），
保证求值管线在遇到坏节点时可**优雅降级**而不是整个图崩溃。

### 算法 trait 接口（实现见 ze-graph-algo）

```rust
pub trait TopoSort       { fn sort<G: Graph>(g: &G) -> Result<Vec<NodeId>, CycleError>; }
pub trait DirtyTracker   { fn mark_dirty(&mut self, id: NodeId); fn dirty_nodes(&self) -> impl Iterator<Item=NodeId>; }
pub trait CycleDetector  { fn detect<G: Graph>(g: &G) -> Result<(), CycleError>; }
```

---

## 使用示例：手动构造一张图

```rust
use ze_node::*;

// 1) 定义节点
struct Constant { value: f32 }
impl Node for Constant {
    fn meta(&self) -> &'static NodeMeta {
        static M: std::sync::OnceLock<NodeMeta> = std::sync::OnceLock::new();
        M.get_or_init(|| NodeMeta::new("demo.const", "Constant", 1))
    }
    fn outputs(&self) -> &'static [OutputSocket] {
        static O: std::sync::OnceLock<Vec<OutputSocket>> = std::sync::OnceLock::new();
        O.get_or_init(|| vec![OutputSocket::new(Box::new(NamedTypeTag::new("f32"))).with_index(0)])
    }
    fn inputs(&self) -> &'static [InputSocket] { &[] }
    fn dirty_policy(&self) -> DirtyPolicy { DirtyPolicy::Never }
    fn state_type(&self) -> Box<dyn TypeTag> { Box::new(NamedTypeTag::new("()")) }
    fn as_any(&self) -> &dyn std::any::Any { self }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any { self }
}

// 2) 图
let mut g = SimpleGraph::new();
let c = g.add_node(Box::new(Constant { value: 3.14 }));
let a = g.add_node(/* Box::new(Add) another impl */);

// 3) 连接 (OutputSocketId(c,0) → InputSocketId(a,0))
g.connect(OutputSocketId::new(c, 0), InputSocketId::new(a, 0))?;
```

> ⚠️ 样板较多——日常开发请使用 `ze-node-schema` 的 `#[derive(NodeSpec)]` 宏，
> 它为你自动生成 `Node + NodeSpec + Default` 三个 impl（见 ze-node-schema-macros）。

---

## 依赖

```toml
serde       = { version = "1.0", features = ["derive"] }
serde_json  = "1.0"
thiserror   = "2.0"
derive_more = "2.1" (full)
```

## 测试

```bash
cargo test -p ze-node-core
```

覆盖 `SimpleGraph` 的增删连断、`NodeValue` 向下转型、`CycleError` 结构化构造
与 serde round-trip。
