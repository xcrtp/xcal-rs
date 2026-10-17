// export.rs — L3 文档导出：**JSON 是唯一真源**，LuaLS 注解从它生成。
//
// 类型名映射（`f32 → number` / `Handle → userdata`）是**脚本语言方言**问题，
// 只在本层做：`xc-vm` 只出 VM 类型名（`f32` / `xc_vm.Handle`），不出现语言专有词。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use xc_vm::Capability;

use crate::api::SceneApi;

/// VM 类型名 → Lua 类型名。
///
/// 未知类型按 `any` 处理（不 panic：文档导出不该因为新增类型名而炸）。
pub fn lua_type_name(vm_type: &str) -> &'static str {
    match vm_type {
        "()" => "nil",
        "bool" => "boolean",
        "String" => "string",
        "f32" | "f64" => "number",
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => "integer",
        "xc_vm.Handle" => "Handle",
        // 胶水层在宿主调用前把 Lua table 摊平成列表。
        "FloatList" | "ComponentList" => "table",
        _ => "any",
    }
}

/// 导出 `scene_api.json`（机器可读真源）。
pub fn scene_api_json() -> String {
    let meta = SceneApi::meta();
    let functions: Vec<Value> = SceneApi::docs()
        .iter()
        .map(|doc| {
            let params: Vec<Value> = doc
                .params
                .iter()
                .enumerate()
                .map(|(i, ty)| {
                    json!({
                        "name": format!("p{}", i + 1),
                        "type": ty,
                        "lua": lua_type_name(ty),
                    })
                })
                .collect();
            json!({
                "qualified": doc.qualified().to_string(),
                "namespace": doc.namespace,
                "name": doc.name,
                "description": doc.description,
                "params": params,
                "returns": { "type": doc.ret, "lua": lua_type_name(doc.ret) },
            })
        })
        .collect();

    let value = json!({
        "api": "xcal.scene",
        "version": meta.version,
        "description": meta.description,
        "functions": functions,
    });
    serde_json::to_string_pretty(&value).expect("serde_json 序列化 Value 不会失败")
}

/// 导出 `scene_api.lua`（LuaLS 注解；从 JSON 的同一来源生成）。
pub fn scene_api_lua() -> String {
    let mut out = String::from("---@meta\n\n");
    out.push_str("---@class Handle xcal 场景句柄（组件 / 对象 / 曲线 / 动作）\n");
    out.push_str("---@class SceneApi\n");
    out.push_str("scene = {}\n");

    for doc in SceneApi::docs() {
        out.push('\n');
        for line in doc.description.lines() {
            out.push_str("---");
            out.push_str(line);
            out.push('\n');
        }
        let params: Vec<String> = doc
            .params
            .iter()
            .enumerate()
            .map(|(i, ty)| {
                let name = format!("p{}", i + 1);
                out.push_str(&format!("---@param {name} {}\n", lua_type_name(ty)));
                name
            })
            .collect();
        if doc.ret != "()" {
            out.push_str(&format!("---@return {}\n", lua_type_name(doc.ret)));
        }
        out.push_str(&format!(
            "function scene.{}({}) end\n",
            doc.name,
            params.join(", ")
        ));
    }
    out
}

/// 把两份产物写到 `dir`（返回实际写入的两个路径）。
pub fn export_scene_api(dir: &Path) -> io::Result<[PathBuf; 2]> {
    fs::create_dir_all(dir)?;
    let json_path = dir.join("scene_api.json");
    let lua_path = dir.join("scene_api.lua");
    fs::write(&json_path, scene_api_json())?;
    fs::write(&lua_path, scene_api_lua())?;
    Ok([json_path, lua_path])
}
