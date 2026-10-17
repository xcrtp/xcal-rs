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
// # 文档（L2/L3）
//
// 文档真源 = [`SceneApi`] 的 `#[capability]` 声明（字段名 + 类型 + `///`）；
// [`export_scene_api`] 产出 JSON（唯一真源）与 LuaLS 注解。
#![forbid(unsafe_code)]

pub mod api;
pub mod export;
pub mod lua_vm;
pub mod service;

pub use api::SceneApi;
pub use export::{export_scene_api, lua_type_name, scene_api_json, scene_api_lua};
pub use lua_vm::{INJECT_PATH, LuaVm, REQUIREMENTS_PATH, RUN_PATH, ScriptHost};
pub use service::{FINISH_PATH, RESET_PATH, SERVICE_NAME, SceneServiceVm};

/// L5 的脚本输入：现有分镜中**可被最小接口表达**的那一段（镜头 2–5）。
///
/// 公式书写（Typst）/ 标注文字 / 箭头 / 坐标轴不在本轮接口内 —— 见交付说明的缺口记账。
pub const STORYBOARD_LUA: &str = include_str!("../scripts/storyboard.lua");
