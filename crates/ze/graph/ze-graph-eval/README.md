# ze-graph-eval

> 为 `ze-node-core` 提供的**求值策略**。默认实现 `LazyPullEval`
> （反向依赖惰性求值 + 脏传播 + 错误恢复），以及 `DefaultExecContext` 作为
> `ExecContext` trait 的具体承载。

---

## 在节点栈中的位置

```
ze-node-core (trait / Graph)   →   ze-graph-algo (Topo / Dirty / SCC)
                                             │
                                             ▼
                                   ze-graph-eval  ← 本 crate
                                             │
                                   ┌─────────┴─────────┐
                                   ▼                   ▼
                            ze-node-schema         自定义后端 (CLI / WASM / RPC)
```

---

## `LazyPullEval`：反向依赖惰性求值

核心思路：**只有被请求的输出才会被计算**；与 Eager 模式"整张图按拓扑顺序全部
重算一次"相比，避免了用户未观察到的下游节点的浪费。

```
      [C: f32=2]  [D: f32=3]
           ╲        ╱
            [Add: out=C+D]
                │
              [Sin] ← 用户请求 evaluate(sin, output=0)
                │
            [Display]（未请求 → 不计算）
```

内部执行流程：

1. `LazyPullEval::evaluate(target, output_index)` → DFS 反向遍历依赖
2. 每到达一个节点 `n`：
   - 若 `dirty_tracker.is_dirty(n)` **或** `cached[n]` 不存在 → 执行
   - 否则直接返回缓存值
3. 节点执行通过 `ExecutorFactory::make(node.type_name)` 取到
   `Box<dyn NodeExecutor>`，并将 `DefaultExecContext` 传进去
4. 执行报错时走 `RecoveryHandle::handle`（见 ze-node-core），按策略决定
   `Stop / UseDefault / Retry(n) / Replace(value)`
5. 所有成功执行的输出值被写入 `outputs: HashMap<OutputSocketId, NodeValue>`
   缓存，下次命中直接返回

---

## 最小示例

```rust
use ze_graph_eval::{LazyPullEval, DefaultExecContext};
use ze_graph_algo::{BfsDirtyTracker, KahnSorter};
use ze_node::{Graph, SimpleGraph, OutputSocketId, NodeId,
                ExecutorFactory, DynNodeExecutor, EvalStrategy};

// ── 1) 建图（略） ──
let mut g = SimpleGraph::new();
let display: NodeId = /* 终端显示节点 */ g.add_node(/*...*/);

// ── 2) 注册执行器工厂 ──
let mut factory = ExecutorFactory::new();
factory.register("demo.const",  |_| Box::new(ConstExecutor) as DynNodeExecutor);
factory.register("demo.sin",    |_| Box::new(SinExecutor));
factory.register("demo.display",|_| Box::new(DisplayExecutor));

// ── 3) 组装求值器 ──
let mut ctx = DefaultExecContext::new();
let mut evaluator = LazyPullEval::new(
    KahnSorter,           // 拓扑（用于 eager / 回退）
    BfsDirtyTracker::new_from_graph(&g),
    factory,
    EvalStrategy::Lazy,
);

// ── 4) 求值：拉取 display 第 0 个输出（触发上游整条链） ──
let value = evaluator.evaluate(&g, &mut ctx, display, 0)?;
let v: &f32 = value.downcast_ref().unwrap();
println!("display.out(0) = {v}");
```

---

## `DefaultExecContext` —— 运行时上下文

```rust
pub struct DefaultExecContext { /* 内部：输入、输出、状态、资源表 */ }

impl ExecContext for DefaultExecContext {
    fn read_input(&self, idx: u32)      -> Option<&NodeValue>  { ... }
    fn write_output(&mut self, idx: u32, v: NodeValue)         { ... }
    fn state_mut<T: 'static>(&mut self) -> Option<&mut T>      { ... }
    fn resource<T: 'static>(&self)     -> Option<&T>           { ... }
    fn resource_mut<T: 'static>(&mut self) -> Option<&mut T>   { ... }
    fn request_eval(&mut self, id: NodeId, out: u32) -> Result<NodeValue, EvalError> { ... }
}
```

配套 API：
- `ctx.attach_inputs(node_id, &graph, &output_cache)`：把上游输出值拷贝进来
- `ctx.insert_resource(MyResource)`：全局只读/可写资源（例如时间、随机种子）
- `ctx.take_outputs()`：清空并返回当前节点所有输出（供求值器写入缓存）

---

## Feature：可替换的拓扑 + 脏标记

`LazyPullEval::new(topo: impl TopoSort, dirty: impl DirtyTracker, ...)`
接受 trait bound，意味着可以随时：

- 把 `KahnSorter` 换成你自己的"按优先级排序"的 `TopoSort`；
- 把 `BfsDirtyTracker` 换成支持持久化的 `DirtyTracker`；

求值器本身代码**零改动**。

---

## 依赖

```toml
ze-node-core       = { path = "../ze-node-core" }
# dev
ze-graph-algo = { path = "../ze-graph-algo" }
```

## 测试

```bash
cargo test -p ze-graph-eval
```

覆盖：
- 3 节点链 (Const → Sin → Display) LazyPull 按依赖顺序计算；
- 修改中间节点后二次 evaluate 只刷新受影响分支；
- 自定义 `RecoveryHandle` 在坏节点上正确返回 `UseDefault`；
- 菱形依赖图每个节点恰好执行**一次**（非重复求值）。
