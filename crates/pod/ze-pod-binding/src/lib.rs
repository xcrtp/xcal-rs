//! ze-pod-binding：**语言无关**的绑定驱动。
//!
//! ```text
//! capability 的 calls()  ──►  BindingPlan（函数 + 可达类型）
//!                                   │
//!                    ┌──────────────┴──────────────┐
//!                    ▼                             ▼
//!              Lua 适配器                    Python 适配器
//!              ValueCodec = LuaValue           ValueCodec = PyObject
//! ```
//!
//! 三层职责：
//!
//! | 模块 | 内容 | 依赖 |
//! |------|------|------|
//! | [`plan`] | `BindingPlan` / `component_ctor` / 类型收集 | `ze-pod` |
//! | [`codec`] | `ValueCodec` + 通用调用编排 [`invoke`] | `ze-pod` |
//! | [`docs`] | JSON 文档（唯一真源） | `ze-pod` |
//!
//! **本 crate 不出现任何具体语言类型**（不含语言运行时依赖）；
//! `cargo tree -p ze-pod-binding` 只有 `ze-pod` + 自身，可进 wasm gate。
#![forbid(unsafe_code)]

pub mod codec;
pub mod docs;
pub mod plan;

pub use codec::{ValueCodec, invoke};
pub use docs::{CapabilityDocs, capabilities_json, reachable_types};
pub use plan::{BindingPlan, collect_types, short_name};
