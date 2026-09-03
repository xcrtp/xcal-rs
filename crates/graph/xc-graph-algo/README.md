# xc-graph-algo

> 为 `xc-node` 提供的**具体图算法实现**。只依赖
> `xc-node`（trait 层），不依赖任何上层。算法可随时替换——只要新的实现满足
> 相同的 `TopoSort` / `DirtyTracker` / `CycleDetector` trait。

---

## 提供的实现

| Trait | 实现 | 算法 | 复杂度 | 说明 |
|-------|------|------|--------|------|
| `TopoSort` | `KahnSorter` | **Kahn BFS 拓扑排序**（入度表 + 队列） | O(V + E) | 字典序最小；遇到环返回 `CycleError { nodes: Vec<NodeId> }` 包含任意**环中节点** |
| `DirtyTracker` | `BfsDirtyTracker` | **反向 BFS 脏标记**：`mark_dirty(n)` 把 n 的所有传递前驱（输出 → 输入反方向）都标记为脏 | O(V + E) 单次 mark；查询 O(1) per node |
| `CycleDetector` | `TarjanSccDetector` | **Tarjan 强连通分量**：所有 size ≥ 2 的 SCC 就是环；自环单独检测 | O(V + E) | 输出 `Vec<Vec<NodeId>>` 每个独立环一组；支持在 `Graph::connect` 前做预校验 |

---

## Kahn 拓扑排序示例

```rust
use xc_graph_algo::KahnSorter;
use xcal_node::{TopoSort, SimpleGraph, ConnectError};

fn eval_eager<G: xcal_node::Graph>(g: &G) -> Result<(), xcal_node::CycleError> {
    let order: Vec<_> = KahnSorter::sort(g)?;
    for n in order { /* execute node n */ }
    Ok(())
}

let g = build_my_graph();
match eval_eager(&g) {
    Ok(()) => println!("ok"),
    Err(cyc) => eprintln!("cycle at nodes: {:?}", cyc.nodes),
}
```

---

## 脏标记传播（用于增量求值）

```rust
use xc_graph_algo::BfsDirtyTracker;
use xcal_node::{DirtyTracker, Graph, SimpleGraph};

let g: &SimpleGraph = /* ... */;

// 每次外部输入变化，把源节点标记为脏
let mut tracker = BfsDirtyTracker::new_from_graph(g);
tracker.mark_dirty(user_changed_constant_id);

// 求值时仅遍历脏集合
for id in tracker.dirty_nodes() {
    re_execute(id);
}
tracker.clear();
```

典型应用：`xc-graph-eval::LazyPullEval` 默认依赖 `BfsDirtyTracker` 来跳过
已稳定子图。

---

## 环检测（用于 UI 禁止连回边）

```rust
use xc_graph_algo::TarjanSccDetector;
use xcal_node::{CycleDetector, Graph};

fn can_connect<G: Graph>(g: &G, from: xcal_node::OutputSocketId, to: xcal_node::InputSocketId) -> bool {
    // 1) 临时连
    // 2) 环检测
    // 3) 真有环就回滚（Editor::can_connect 在 xcal-node-editor 中实现）
    TarjanSccDetector::detect(g).is_ok()
}
```

> 注：`xcal-node-editor::ConnectionValidator` 已经封装"类型兼容 + 环检测"
> 的统一校验流程。

---

## 算法约束与可替换性

- 所有算法对 `Graph` trait 做**纯 trait bounded 泛型**，不关心具体存储。
- `KahnSorter::sort` / `TarjanSccDetector::detect` 是 **无状态关联函数**（可以
  静态调用，不需要实例）。
- `BfsDirtyTracker` 需要保留内部 `HashMap<NodeId, bool>` 状态，因此实例
  需**跟随图生命周期**，建议作为 `Editor` 或求值器的字段持有。
- 需要其它算法？在你自己的 crate 里实现对应 trait 即可，无需改本 crate。

---

## 测试

```bash
cargo test -p xc-graph-algo
```

覆盖：
- 线性链 / 菱形依赖图 / 含环图：Kahn 返回正确顺序或 `CycleError`
- BFS 脏传播：三节点链上标记 source→末端节点应被标记
- Tarjan SCC：自环、双向环、两个独立环能被分组识别

## 依赖

```toml
xc-node = { path = "../xc-node" }
```
