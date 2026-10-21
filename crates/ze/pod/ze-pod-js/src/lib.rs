//! JS 适配器（boa）：能力注入 + 宿主 → 脚本入口。
//!
//! # 定位（B3-P）
//!
//! JS 只是**第 N 个语言适配器**：复用 `ze-pod-core` 的 Injector / Broker 与
//! `ze-pod-binding` 的 `BindingPlan` / `ValueCodec`，**不新增 capability**，
//! 也不改 `zela.scene` / `zela.content` 的 namespace 与 version。
//!
//! # 双引擎 feature
//!
//! | feature | 目标 | 行为 |
//! |---|---|---|
//! | `boa`（默认） | native + wasm | 纯 Rust 引擎，两边**同一个实现** |
//! | `host-js`（可选，**本批次未接线**） | 仅 wasm | 非 wasm 启用 ⇒ **编译期报错**；wasm 启用 ⇒ 显式"尚未接线" |
//!
//! 生产依赖**不含任何 `zela-*`**（与 `ze-pod-lua` / `ze-pod-py` 同形状）。

// J3：`host-js` 在非 wasm 目标下必须显式失败，不得静默降级到宿主引擎。
#[cfg(all(feature = "host-js", not(target_arch = "wasm32")))]
compile_error!(
    "feature `host-js` 只能在 wasm32 目标上启用：native 目标请用默认的 boa 引擎（B3-P J3）"
);

// B3-Pe 未接线：显式失败，不静默忽略（铁律 3）。
#[cfg(all(feature = "host-js", target_arch = "wasm32"))]
compile_error!(
    "feature `host-js` 在本批次尚未接线（浏览器原生引擎 + CSP 回退未实现）；请使用默认的 boa 引擎"
);

mod binding;
mod export;
mod js_pod;

pub use export::{d_ts, export_docs, ts_type_name};
pub use js_pod::{
    ENTRY_SERVICE, FINISH_PATH, INJECT_PATH, JsPod, REQUIREMENTS_PATH, RESET_PATH, RUN_ENTRY,
    RUN_PATH,
};
