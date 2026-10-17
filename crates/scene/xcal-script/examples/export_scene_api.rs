//! 导出 `SceneApi` 的机器可读文档（JSON 真源 + LuaLS 注解）。
//!
//! ```text
//! cargo run -p xcal-script --example export_scene_api
//! ```
//!
//! 产物写到 `target/scene_api/`；两个文件都从 `SceneApi::docs()` 生成。

use xc_vm::Capability;

fn main() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("target")
        .join("scene_api");
    let [json, lua] = xcal_script::export_scene_api(&dir).expect("写导出文件");
    println!("JSON  : {}", json.display());
    println!("LuaLS : {}", lua.display());
    println!("函数数: {}", xcal_script::SceneApi::signatures().len());
}
