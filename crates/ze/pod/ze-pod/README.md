# ze-pod

zela 的 Pod 层统一入口。

聚合三个平台无关的 crate：

- `ze-pod-core`：Pod 抽象（不透明状态机 + capability 协商 + 函数集合），纯 std、零依赖；
- `ze-pod-binding`：语言无关的绑定驱动（能力计划 / 调用编排 / JSON 文档）；
- `ze-pod-macro`：`#[capability]` / `#[derive(PodType)]` 等派生宏。

`ze-pod` **不包含** `ze-pod-lua` 与 `ze-pod-py`：它们分别需要 C 工具链与系统
Python，属于平台相关的语言适配器，保持独立 crate。因此 `ze-pod` 可以编译到
`wasm32-unknown-unknown`。

```toml
[dependencies]
ze-pod = "0.1"
```

```rust
use ze_pod::Pod;
```

License: MIT OR Apache-2.0
