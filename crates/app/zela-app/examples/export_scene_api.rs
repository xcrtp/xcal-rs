//! 导出 domain 能力的机器可读文档（JSON 真源 + LuaLS + Python `.pyi`）。
//!
//! ```text
//! cargo run -p zela-app --example export_scene_api
//! ```
//!
//! 产物写到 `target/scene_api/`：
//! - `scene_api.json` / `scene_api.lua`（`ze-pod-lua`）；
//! - `zela/__init__.pyi` / `zela/scene.pyi` / `zela/content.pyi` / `zela/python.pyi`
//!   （`ze-pod-py`，路径与 capability 命名空间一致）。

use ze_pod_core::Capability;
use ze_pod_binding::CapabilityDocs;
use zela_scene_ecs::pod::{ContentApi, SceneApi};

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("target")
        .join("scene_api");

    // `capabilities` 只在开启 python feature 时追加 PythonApi。
    #[cfg_attr(not(feature = "python"), allow(unused_mut))]
    let mut capabilities = vec![
        CapabilityDocs {
            name: "scene",
            calls: SceneApi::calls(),
            docs: SceneApi::docs(),
            meta: SceneApi::meta(),
        },
        CapabilityDocs {
            name: "content",
            calls: ContentApi::calls(),
            docs: ContentApi::docs(),
            meta: ContentApi::meta(),
        },
    ];
    #[cfg(feature = "python")]
    capabilities.push(CapabilityDocs {
        name: "python",
        calls: ze_pod_py::PythonApi::calls(),
        docs: ze_pod_py::PythonApi::docs(),
        meta: ze_pod_py::PythonApi::meta(),
    });

    let [json, lua] = ze_pod_lua::export_docs(&dir, &capabilities).expect("写导出文件");
    println!("JSON  : {}", json.display());
    println!("LuaLS : {}", lua.display());

    #[cfg(feature = "python")]
    {
        let paths = ze_pod_py::export_docs(&dir, &capabilities).expect("写 Python 类型存根");
        for path in paths {
            println!("PYI   : {}", path.display());
        }
    }

    println!(
        "函数数: scene {} / content {}",
        SceneApi::signatures().len(),
        ContentApi::signatures().len()
    );
}
