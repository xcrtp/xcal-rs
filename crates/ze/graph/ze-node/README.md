# ze-node

zela 的节点图统一入口。

聚合四个平台无关的 crate：

- `ze-node-core`：节点 / 边 / 端口 / 图 / 执行上下文；
- `ze-node-schema`：`NodeSpec` / `Property` / `NodeRegistry` / `DynamicObject`；
- `ze-graph-algo`：拓扑排序、环检测、脏标记传播；
- `ze-graph-eval`：拉取式求值上下文。

不包含任何 UI 依赖，可编译到 `wasm32-unknown-unknown`，也可在 headless 环境使用。

```toml
[dependencies]
ze-node = "0.1"
```

```rust
use ze_node::{Graph, NodeSpec};
```

License: MIT OR Apache-2.0
