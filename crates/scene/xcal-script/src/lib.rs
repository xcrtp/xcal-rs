// xcal-script — 第一个**非原生 VM**（Lua / mlua）与宿主能力文档机制。
//
// # 分层（L0）
//
// - 位置：scene 层（`crates/scene/xcal-script`）—— 将来节点编辑器若要用，不会反向依赖 app；
// - 硬约束：`xc-vm` / `xcal-scene` **不**依赖本 crate（本 crate 只向下依赖）；
// - `mlua` 用 `vendored`：解释器编进二进制，部署零运行时依赖。
//
// # 双向（L4）
//
// ```text
// 脚本 → 宿主：装配期经 `Injector` 拿到 `SceneApi` 的 typed 函数（不每帧）。
// 宿主 → 脚本：`LuaVm::run` / `Vm::call("xcal.script::run")`（erased）。
// ```
//
// # 类型系统与绑定生成（V0–V4）
//
// ```text
// types.rs（#[derive(VmType)]，一处定义）
//    └─ TypeSpec（struct / enum / array / opaque） + 双向转换
//         └─ SceneApi::calls()（#[capability(bindings)] 生成）
//              ├─ binding.rs：按 TypeSpec 生成 Lua 函数与 `Circle { ... }` 构造器
//              └─ export.rs ：JSON（唯一真源） + LuaLS（形参名 / @class / @field）
// ```
//
// 手写胶水只剩"宿主侧落点转换"（结构化值 → 场景原语），不再逐函数重复。
#![forbid(unsafe_code)]

pub mod api;
pub mod binding;
pub mod export;
pub mod lua_vm;
pub mod service;
pub mod types;

pub use api::SceneApi;
pub use binding::{LuaHandle, LuaVmValue};
pub use export::{export_scene_api, scene_api_json, scene_api_lua};
pub use lua_vm::{INJECT_PATH, LuaVm, REQUIREMENTS_PATH, RUN_PATH, ScriptHost};
pub use service::{FINISH_PATH, RESET_PATH, SERVICE_NAME, SceneServiceVm};
pub use types::{Camera2D, Circle, Color, Curve, Reveal, Stroke, Vec2};

/// L5 的脚本输入：现有分镜中**可被最小接口表达**的那一段（镜头 2–5）。
///
/// 公式书写（Typst）/ 标注文字 / 箭头 / 坐标轴不在本轮接口内 —— 见交付说明的缺口记账。
pub const STORYBOARD_LUA: &str = include_str!("../scripts/storyboard.lua");
