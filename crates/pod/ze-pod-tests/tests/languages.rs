//! 7. Lua 与 Python **两侧各跑同一套绑定生成用例**。
//!
//! 两种语言使用同一份 `MiniApi`（`calls()` / `docs()`），脚本语义逐项等价；
//! 断言两侧的录制结果逐字段相同，且文档生成都来自同一份 `CapabilityDocs`。
//!
//! # feature 门控
//!
//! Python 执行侧需要系统 Python（`ze-pod-py/python`），由本 crate 的
//! `python` feature 转发。默认路径只跑 Lua + 两侧共用的代码生成，
//! 因此 `cargo test -p ze-pod-tests` 不需要 Python。

use std::any::Any;
use std::sync::Arc;

use ze_pod::{Broker, Capability, Pod, PodNode};
use ze_pod_binding::CapabilityDocs;
use ze_pod_lua::{LuaPod, RUN_ENTRY, lua_ls};
#[cfg(feature = "python")]
use ze_pod_py::PyPod;
use ze_pod_tests::{MiniApi, MiniOutput, MiniRecorderPod};

/// Lua 版：与 Python 版逐项等价。
const LUA_SOURCE: &str = r#"
    local dot = Dot { x = 1.0, y = 2.0, r = 0.5 }
    scene.emit(3.5)
    scene.kind(Kind.Const)
    scene.kind(Kind.Weight { value = 2.0 })
    scene.emit(scene.total() + 1.0)
"#;

/// Python 版：同一套操作（仅 `python` feature 下编译）。
#[cfg(feature = "python")]
const PY_SOURCE: &str = r#"
dot = Dot(x=1.0, y=2.0, r=0.5)
scene.emit(3.5)
scene.kind(Kind.Const)
scene.kind(Kind.Weight(value=2.0))
scene.emit(scene.total() + 1.0)
"#;

fn expected() -> MiniOutput {
    MiniOutput {
        dots: 1,
        values: vec![3.5, 4.5],
        kinds: vec!["Const".to_string(), "Weight(2)".to_string()],
    }
}

fn run_lua(source: &str) -> MiniOutput {
    let recorder = Arc::new(MiniRecorderPod::new());
    let lua = Arc::new(LuaPod::new());
    lua.register_capability::<MiniApi>("scene", true);
    let broker = Broker::new(vec![
        PodNode::new_sync("scene", recorder, &["scene"]),
        PodNode::new("lua", lua as Arc<dyn Pod>, &["script"]),
    ]);
    broker.assemble().expect("Lua 装配成功");
    let owned = source.to_string();
    let arg: &dyn Any = &owned;
    broker
        .run_with::<MiniOutput>("lua", RUN_ENTRY, &[arg])
        .expect("Lua 脚本执行成功")
}

#[cfg(feature = "python")]
fn run_python(source: &str) -> MiniOutput {
    let recorder = Arc::new(MiniRecorderPod::new());
    // Python 解释器由 PYO3_PYTHON / PYTHONHOME 指定（见交付说明）。
    let py = Arc::new(PyPod::new().expect("Python 解释器初始化失败（检查 PYO3_PYTHON / PYTHONHOME）"));
    py.register_capability::<MiniApi>("scene", true);
    let broker = Broker::new(vec![
        PodNode::new_sync("scene", recorder, &["scene"]),
        PodNode::new_sync("python", Arc::clone(&py), &["script"]),
    ]);
    broker.assemble().expect("Python 装配成功");
    py.run::<MiniOutput>(source).expect("Python 脚本执行成功")
}

/// Lua 侧基线（默认 feature 集即可运行，不依赖 Python）。
#[test]
fn lua_script_produces_the_shared_binding_results() {
    let lua = run_lua(LUA_SOURCE);
    assert_eq!(lua, expected(), "Lua 侧结果");
    eprintln!("[P6-7] Lua = {lua:?}");
}

/// Lua 与 Python 逐字段一致（仅 `python` feature 下运行）。
#[cfg(feature = "python")]
#[test]
fn lua_and_python_agree_on_the_same_binding_cases() {
    let lua = run_lua(LUA_SOURCE);
    let python = run_python(PY_SOURCE);

    assert_eq!(lua, expected(), "Lua 侧结果");
    assert_eq!(python, expected(), "Python 侧结果");
    assert_eq!(lua, python, "两种语言必须逐字段相同");

    eprintln!("[P6-7] Lua    = {lua:?}");
    eprintln!("[P6-7] Python = {python:?}");
}

#[test]
fn lua_and_python_generate_docs_from_the_same_calls() {
    let capabilities = [CapabilityDocs {
        name: "mini",
        calls: MiniApi::calls(),
        docs: MiniApi::docs(),
        meta: MiniApi::meta(),
    }];

    // Lua：LuaLS 注解。
    let lua_text = lua_ls(&capabilities);
    for function in ["dot", "emit", "total", "kind"] {
        assert!(
            lua_text.contains(&format!("function mini.{function}(")),
            "LuaLS 缺少 `{function}`：\n{lua_text}"
        );
    }
    assert!(lua_text.contains("---@class Dot"), "LuaLS 缺少类型 Dot");

    // Python：`.pyi` 文件树。
    let pyi = ze_pod_py::pyi_files(&capabilities);
    let (path, text) = pyi
        .iter()
        .find(|(path, _)| path.contains("mini"))
        .unwrap_or_else(|| panic!("没有 mini 的 .pyi；实际文件：{:?}", pyi.keys()));
    for function in ["dot", "emit", "total", "kind"] {
        assert!(
            text.contains(&format!("def {function}(")),
            "`.pyi` 缺少 `{function}`（{path}）：\n{text}"
        );
    }

    // 两侧都来自同一份 JSON 真源。
    let json: serde_json::Value =
        serde_json::from_str(&ze_pod_binding::capabilities_json(&capabilities))
            .expect("合法 JSON");
    let functions = json["capabilities"]["mini"]["functions"]
        .as_array()
        .expect("mini functions");
    assert_eq!(functions.len(), MiniApi::signatures().len());

    eprintln!("[P6-7] LuaLS 行数 = {}", lua_text.lines().count());
    eprintln!("[P6-7] .pyi 文件 = {:?}", pyi.keys().collect::<Vec<_>>());
    eprintln!("[P6-7] JSON functions = {}", functions.len());
}
