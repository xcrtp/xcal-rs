//! 导出 domain 能力的机器可读文档（JSON 真源 + LuaLS + Python `.pyi`）。
//!
//! ```text
//! cargo run -p xcal-app --example export_scene_api
//! ```
//!
//! 产物写到 `target/scene_api/`：
//! - `scene_api.json` / `scene_api.lua`（`xc-pod-lua`）；
//! - `xcal/__init__.pyi` / `xcal/scene.pyi` / `xcal/content.pyi` / `xcal/python.pyi`
//!   （`xc-pod-py`，路径与 capability 命名空间一致）。

use xc_pod::Capability;
use xc_pod_binding::CapabilityDocs;
use xcal_scene::pod::{ContentApi, SceneApi};

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
        calls: xc_pod_py::PythonApi::calls(),
        docs: xc_pod_py::PythonApi::docs(),
        meta: xc_pod_py::PythonApi::meta(),
    });

    let [json, lua] = xc_pod_lua::export_docs(&dir, &capabilities).expect("写导出文件");
    println!("JSON  : {}", json.display());
    println!("LuaLS : {}", lua.display());

    #[cfg(feature = "python")]
    {
        let paths = xc_pod_py::export_docs(&dir, &capabilities).expect("写 Python 类型存根");
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
