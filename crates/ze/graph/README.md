# crates/ze/graph — 节点图契约层

## 职责

ze-node-schema = 节点结构契约（Property / NodeSpec / DynamicObject / NodeRegistry）
+ 对应宏（ze-node-schema-macros）。

## 铁律

1. **禁止依赖 egui** — 本层是纯逻辑/数据契约，不绑定任何 UI 框架。
2. **禁止依赖任何 `zela-*` crate** — 依赖方向单向：`zela-* → ze-*`，反向禁止。
   - 不依赖 zela-scene / zela-render / zela-render-backends / zela-egui / ze-macro
   - 不依赖 zela-node-editor / zela-node-egui / zela-node-egui-ui / zela-app
3. **禁止依赖领域类型** — DynamicObject / DynamicNode 不得引用 FunctionDef、AST、
   AnimationClip 等领域特定类型。DynamicObject 只持有 PropertyValue（标量/向量/
   字符串/颜色），不持有领域语义。
4. **仅依赖 ze-node-core** — ze-node-schema 与 ze-node-schema-macros 只能依赖 ze-node-core
   （以及 ze-graph-algo / ze-graph-eval 用于 dev-dependencies 测试）。
5. **外部依赖仅限** serde / serde_json / thiserror / derive_more / proc-macro 工具链
   （quote / syn / proc-macro2）。

## Crate 清单

| Crate | 说明 |
|-------|------|
| `ze-node-core` | 节点图内核：Node / Graph / Socket trait、NodeMeta + NodeInteraction |
| `ze-graph-algo` | 图算法：Kahn topo sort、Tarjan SCC、BFS 脏标记传播 |
| `ze-graph-eval` | 惰性拉取求值：LazyPullEval + EvalContext |
| `ze-node-schema` | 属性模型：Property / NodeSpec / NodeRegistry / DynamicObject |
| `ze-node-schema-macros` | `#[derive(NodeSpec)]` 过程宏 + `#[property]` / `#[node]` 属性 |
