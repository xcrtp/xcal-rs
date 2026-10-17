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
pub mod content;
pub mod export;
pub mod lua_vm;
pub mod service;
pub mod types;

pub use api::{ContentApi, SceneApi};
pub use binding::{LuaHandle, LuaVmValue};
pub use content::{CONTENT_SERVICE_NAME, ContentServiceVm, PathStore};
pub use export::{export_scene_api, scene_api_json, scene_api_lua};
pub use lua_vm::{INJECT_PATH, LuaVm, REQUIREMENTS_PATH, RUN_PATH, ScriptHost};
pub use service::{FINISH_PATH, RESET_PATH, SERVICE_NAME, SceneServiceVm};
pub use types::{
    Arrow, Axes2D, Camera2D, Circle, Color, Curve, Fill, Formula, LineStyle, Path, PathList,
    PathShape, Reveal, RevealPath, Stroke, Text, Transform, Vec2,
};

/// **完整分镜的脚本输入**（6 个镜头 + T2 箭头 + T3 标注文字）。
///
/// 与 `xcal-app` 的硬编码参考实现逐对象 / 逐绑定等价
/// （用例 `script_storyboard_matches_hardcoded`）。
pub const STORYBOARD_LUA: &str = include_str!("../scripts/storyboard.lua");
