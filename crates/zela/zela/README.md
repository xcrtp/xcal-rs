# zela

zela 的统一入口：把 Pod 层与节点图基础层聚合到一个 crate。

- `ze-pod`：Pod 抽象、绑定驱动、`#[capability]` 宏（不含 lua / py，保持 wasm32）；
- `ze-node`：节点图核心、schema、图算法、求值（不含 UI）。

`zela` 的全部内容都是对真实 API 的 re-export，不是只为占名的空壳。

```toml
[dependencies]
zela = "0.1"
```

```rust
use zela::{Graph, Pod};
```

场景 / 渲染 / UI / App 层仍在各自 crate 中，不在本门面内。

License: MIT OR Apache-2.0
