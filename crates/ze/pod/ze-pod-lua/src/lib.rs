// ze-pod-lua — **纯 Lua 适配器**（mlua）+ 语言侧文档生成。
//
// 生产依赖只有 `ze-pod-core` / `ze-pod-binding` / `mlua`：
// 不依赖任何 `zela-*`，不知道 scene / content / 渲染为何物。
//
// ```text
// C::calls() ──► BindingPlan（ze-pod-binding）──► Lua 闭包
//                        ▲
//                   LuaCodec（本 crate）
// ```
//
// 宿主侧装配（节点声明 / Broker / 注入）归 App；本 crate 只提供：
// - `LuaPod`：解释器 + `register_capability::<C>`；
// - `lua_ls`：从 `CapabilityDocs` 生成 LuaLS 注解（无 domain 知识）。
#![forbid(unsafe_code)]

pub mod binding;
pub mod export;
pub mod lua_pod;

pub use binding::{LuaHandle, LuaPodValue};
pub use export::{export_docs, lua_ls};
pub use lua_pod::{
    ENTRY_SERVICE, FINISH_PATH, INJECT_PATH, LuaPod, REQUIREMENTS_PATH, RESET_PATH, RUN_ENTRY,
    RUN_PATH,
};
