// export.rs — **语言侧文档生成**：JSON 由通用层产出，LuaLS 注解在这里生成。
//
// 本文件不引用任何 domain 类型：能力清单由调用方（App / domain crate）传入。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ze_pod_core::{ScalarKind, TypeSpec};
use ze_pod_binding::{BindingPlan, CapabilityDocs, short_name};

/// Lua 侧类型名（LuaLS 注解用）。
pub fn lua_type_name(spec: &'static TypeSpec) -> &'static str {
    match spec {
        TypeSpec::Scalar(kind) => match kind {
            ScalarKind::F32 | ScalarKind::F64 => "number",
            ScalarKind::I32 | ScalarKind::U32 | ScalarKind::U64 | ScalarKind::U8 => "integer",
            ScalarKind::Bool => "boolean",
            ScalarKind::String => "string",
        },
        TypeSpec::Struct { name, .. } | TypeSpec::Enum { name, .. } => short_name(name),
        TypeSpec::Array { .. } => "table",
        TypeSpec::Opaque(_) => "Handle",
    }
}

fn push_description(out: &mut String, text: &str) {
    for line in text.lines() {
        out.push_str("---");
        out.push_str(line);
        out.push('\n');
    }
}

/// 从 `CapabilityDocs` 生成 LuaLS 注解（**无 domain 知识**）。
pub fn lua_ls(capabilities: &[CapabilityDocs]) -> String {
    let types = ze_pod_binding::reachable_types(capabilities);

    let mut out =
        String::from("---@meta\n\n---@class Handle xcal 场景句柄（组件 / 对象 / 曲线 / 动作）\n\n");

    // ── 类型（一次）────────────────────────────────────────────────────
    for (&full_name, &spec) in &types {
        let lua_name = short_name(full_name);
        match spec {
            TypeSpec::Struct { fields, .. } => {
                out.push_str(&format!("---@class {lua_name}\n"));
                for field in *fields {
                    out.push_str(&format!(
                        "---@field {} {}\n",
                        field.name,
                        lua_type_name(field.ty)
                    ));
                }
                out.push('\n');
                // 组件构造器：由"单 struct 参数 + 返回 opaque"识别。
                let component = capabilities.iter().any(|capability| {
                    BindingPlan::new(capability.calls)
                        .component_ctor(full_name)
                        .is_some()
                });
                let returns = if component { "Handle" } else { lua_name };
                out.push_str(&format!(
                    "---@param spec {lua_name}\n---@return {returns}\nfunction {lua_name}(spec) end\n\n"
                ));
            }
            TypeSpec::Enum { variants, .. } => {
                out.push_str(&format!("---@class {lua_name}\n{lua_name} = {{}}\n\n"));
                for variant in *variants {
                    if variant.fields.is_empty() {
                        out.push_str(&format!(
                            "---@type {lua_name}\n{lua_name}.{} = {{}}\n\n",
                            variant.name
                        ));
                        continue;
                    }
                    out.push_str(&format!("---@class {lua_name}.{}\n", variant.name));
                    for field in variant.fields {
                        out.push_str(&format!(
                            "---@field {} {}\n",
                            field.name,
                            lua_type_name(field.ty)
                        ));
                    }
                    out.push_str(&format!(
                        "---@param spec {lua_name}.{}\n---@return {lua_name}\nfunction {lua_name}.{}(spec) end\n\n",
                        variant.name, variant.name
                    ));
                }
            }
            _ => {}
        }
    }

    // ── 能力表 ────────────────────────────────────────────────────────
    for capability in capabilities {
        out.push_str(&format!(
            "---@class {}Api\n{} = {{}}\n\n",
            capitalize(capability.name),
            capability.name
        ));
        for (index, item) in capability.calls.iter().enumerate() {
            if let Some(doc) = capability.docs.get(index) {
                push_description(&mut out, doc.description);
            }
            for param in item.io.params {
                out.push_str(&format!(
                    "---@param {} {}\n",
                    param.name,
                    lua_type_name(param.ty)
                ));
            }
            if !matches!(item.io.ret, TypeSpec::Struct { name: "()", .. }) {
                out.push_str(&format!("---@return {}\n", lua_type_name(item.io.ret)));
            }
            let names: Vec<&str> = item.io.params.iter().map(|param| param.name).collect();
            out.push_str(&format!(
                "function {}.{}({}) end\n\n",
                capability.name,
                item.signature.name,
                names.join(", ")
            ));
        }
    }
    out
}

fn capitalize(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Api".to_string(),
    }
}

/// 把 JSON + LuaLS 写到 `dir`（返回实际写入的两个路径）。
pub fn export_docs(dir: &Path, capabilities: &[CapabilityDocs]) -> io::Result<[PathBuf; 2]> {
    fs::create_dir_all(dir)?;
    let json_path = dir.join("scene_api.json");
    let lua_path = dir.join("scene_api.lua");
    fs::write(&json_path, ze_pod_binding::capabilities_json(capabilities))?;
    fs::write(&lua_path, lua_ls(capabilities))?;
    Ok([json_path, lua_path])
}
