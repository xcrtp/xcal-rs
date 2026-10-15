# crates/graph — 节点图契约层

## 职责

xc-node-schema = 节点结构契约（Property / NodeSpec / DynamicObject / NodeRegistry）
+ 对应宏（xc-node-schema-macros）。

## 铁律

1. **禁止依赖 egui** — 本层是纯逻辑/数据契约，不绑定任何 UI 框架。
2. **禁止依赖任何 `xcal-*` crate** — 依赖方向单向：`xcal-* → xc-*`，反向禁止。
   - 不依赖 xcal-scene / xcal-render / xcal-render-backends / xcal-egui / xc-macro
   - 不依赖 xcal-node-editor / xcal-node-egui / xcal-node-egui-ui / xcal-app
3. **禁止依赖领域类型** — DynamicObject / DynamicNode 不得引用 FunctionDef、AST、
   AnimationClip 等领域特定类型。DynamicObject 只持有 PropertyValue（标量/向量/
   字符串/颜色），不持有领域语义。
4. **仅依赖 xc-node** — xc-node-schema 与 xc-node-schema-macros 只能依赖 xc-node
   （以及 xc-graph-algo / xc-graph-eval 用于 dev-dependencies 测试）。
5. **外部依赖仅限** serde / serde_json / thiserror / derive_more / proc-macro 工具链
   （quote / syn / proc-macro2）。

## Crate 清单

| Crate | 说明 |
|-------|------|
| `xc-node` | 节点图内核：Node / Graph / Socket trait、NodeMeta + NodeInteraction |
| `xc-graph-algo` | 图算法：Kahn topo sort、Tarjan SCC、BFS 脏标记传播 |
| `xc-graph-eval` | 惰性拉取求值：LazyPullEval + EvalContext |
| `xc-node-schema` | 属性模型：Property / NodeSpec / NodeRegistry / DynamicObject |
| `xc-node-schema-macros` | `#[derive(NodeSpec)]` 过程宏 + `#[property]` / `#[node]` 属性 |
