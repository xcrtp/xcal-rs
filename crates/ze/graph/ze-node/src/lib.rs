//! zela 节点图统一入口。
//!
//! 聚合四个平台无关的 crate：
//!
//! - [`ze-node-core`]：节点 / 边 / 端口 / 图 / 执行上下文的纯 trait 层；
//! - [`ze-node-schema`]：属性模型（`NodeSpec` / `Property` / `NodeRegistry`）；
//! - [`ze-graph-algo`]：拓扑排序、环检测、脏标记传播；
//! - [`ze-graph-eval`]：拉取式求值上下文。
//!
//! **不含** `zela-node-editor` / `zela-node-egui` / `zela-node-egui-ui`
//!（UI 层），因此可编译到 `wasm32-unknown-unknown`，也可在 headless 环境使用。
//!
//! ```
//! use ze_node::{Graph, NodeSpec};
//!
//! fn _assert_graph<T: Graph>() {}
//! fn _assert_spec<T: NodeSpec>() {}
//! ```

pub use ze_graph_algo::*;
pub use ze_graph_eval::*;
pub use ze_node_core::*;
pub use ze_node_schema::*;

// 显式消歧：core 与 schema 都有 `error` 模块；类型本身（EvalError /
// PropertyError）已由上面的 glob 导出，这里固定 `error` 指向 core。
pub use ze_node_core::error;
