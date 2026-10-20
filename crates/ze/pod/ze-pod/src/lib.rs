//! zela Pod 统一入口。
//!
//! 聚合 `ze-pod-core`（核心抽象）、`ze-pod-binding`（绑定驱动）与
//! `ze-pod-macro`（`#[capability]` 派生宏）。
//!
//! **不含** `ze-pod-lua` / `ze-pod-py`：前者依赖 vendored Lua（需要 C 工具链），
//! 后者依赖系统 Python。把它们排除在门面之外，`ze-pod` 才能保持纯 Rust、
//! 零额外系统依赖，并继续编译到 `wasm32-unknown-unknown`。
//!
//! ```
//! use ze_pod::Pod;
//!
//! fn _assert_pod<T: Pod>() {}
//! ```

pub use ze_pod_binding::*;
pub use ze_pod_core::*;
pub use ze_pod_macro::*;
