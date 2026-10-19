//! zela 统一入口。
//!
//! 聚合两个基础层门面：
//!
//! - [`ze-pod`]：Pod 抽象 + 绑定驱动 + capability 宏（不含 lua / py）；
//! - [`ze-node`]：节点图核心 + schema + 图算法 + 求值（不含 UI）。
//!
//! `zela` **不是空壳**：它的全部内容都是对上述真实 API 的 re-export。
//! 场景 / 渲染 / UI / App 层不在本门面内。
//!
//! ```
//! use zela::{Graph, Pod};
//!
//! fn _assert_graph<T: Graph>() {}
//! fn _assert_pod<T: Pod>() {}
//! ```

pub use ze_node::*;
pub use ze_pod::*;

// 显式消歧：pod 与 node 都有 `NodeSpec`（前者是 Broker 的装配描述，
// 后者是节点图 schema trait）。统一入口固定为节点图的 `NodeSpec`；
// Pod 侧的装配描述请直接依赖 `ze-pod`。
pub use ze_node::NodeSpec;
