// export.rs — 文档导出：**JSON 是唯一真源**，LuaLS 注解从同一份 `calls()` 生成。
//
// 两组能力（scene / content）都进了同一个产物；类型表覆盖两者的可达类型。
// 形参名来自 schema 的 `ParamSpec.name`（不再是 p1..pN）；unit 枚举变体
// （如 `Target.RevealRange`）在 LuaLS 里是**值**而不是函数。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use xc_vm::{Capability, CapabilityMeta, ErasedFn, FnDoc, ScalarKind, TypeSpec};

use crate::api::{ContentApi, SceneApi};
use crate::binding::{collect_types, component_ctor, short_name};

/// 一组能力的导出视图。
struct CapabilityDocs {
    name: &'static str,
    calls: &'static [ErasedFn],
    docs: &'static [FnDoc],
    meta: CapabilityMeta,
}

fn capabilities() -> [CapabilityDocs; 2] {
    [
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
    ]
}

fn reachable_types() -> BTreeMap<&'static str, &'static TypeSpec> {
    let mut types = BTreeMap::new();
    for capability in capabilities() {
        for item in capability.calls {
            for param in item.io.params {
                collect_types(param.ty, &mut types);
            }
            collect_types(item.io.ret, &mut types);
        }
    }
    types
}

fn scalar_name(kind: ScalarKind) -> &'static str {
    match kind {
        ScalarKind::F32 => "f32",
        ScalarKind::F64 => "f64",
        ScalarKind::I32 => "i32",
        ScalarKind::U32 => "u32",
        ScalarKind::U8 => "u8",
        ScalarKind::Bool => "bool",
        ScalarKind::String => "String",
    }
}

/// Lua 侧类型名（LuaLS 注解 / JSON 的 `lua` 字段）。
pub fn lua_type_name(spec: &'static TypeSpec) -> &'static str {
    match spec {
        TypeSpec::Scalar(kind) => match kind {
            ScalarKind::F32 | ScalarKind::F64 => "number",
            ScalarKind::I32 | ScalarKind::U32 | ScalarKind::U8 => "integer",
            ScalarKind::Bool => "boolean",
            ScalarKind::String => "string",
        },
        TypeSpec::Struct { name, .. } | TypeSpec::Enum { name, .. } => short_name(name),
        TypeSpec::Array { .. } => "table",
        TypeSpec::Opaque(_) => "Handle",
    }
}

fn fields_json(fields: &'static [xc_vm::Field]) -> Vec<Value> {
    fields
        .iter()
        .map(|field| {
            json!({
                "name": field.name,
                "type": spec_ref(field.ty),
            })
        })
        .collect()
}

/// 类型**引用**（命名类型只给名字，定义在顶层 `types` 表里 —— 单一真源）。
fn spec_ref(spec: &'static TypeSpec) -> Value {
    match spec {
        TypeSpec::Scalar(kind) => json!({
            "kind": "scalar",
            "name": scalar_name(*kind),
            "lua": lua_type_name(spec),
        }),
        TypeSpec::Struct { name, .. } => json!({
            "kind": "struct",
            "name": name,
            "lua": lua_type_name(spec),
        }),
        TypeSpec::Enum { name, .. } => json!({
            "kind": "enum",
            "name": name,
            "lua": lua_type_name(spec),
        }),
        TypeSpec::Array { elem } => json!({
            "kind": "array",
            "lua": "table",
            "elem": spec_ref(elem),
        }),
        TypeSpec::Opaque(name) => json!({
            "kind": "opaque",
            "name": name,
            "lua": lua_type_name(spec),
        }),
    }
}

/// 类型**定义**（只用于顶层 `types` 表）。
fn spec_def(spec: &'static TypeSpec) -> Value {
    match spec {
        TypeSpec::Struct { name, fields } => json!({
            "kind": "struct",
            "name": name,
            "lua": lua_type_name(spec),
            "fields": fields_json(fields),
        }),
        TypeSpec::Enum { name, variants } => json!({
            "kind": "enum",
            "name": name,
            "lua": lua_type_name(spec),
            "variants": variants.iter().map(|variant| json!({
                "name": variant.name,
                "fields": fields_json(variant.fields),
            })).collect::<Vec<_>>(),
        }),
        other => spec_ref(other),
    }
}

fn functions_json(capability: &CapabilityDocs) -> Vec<Value> {
    capability
        .calls
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let description = capability
                .docs
                .get(index)
                .map(|doc| doc.description)
                .unwrap_or("");
            json!({
                "qualified": item.signature.qualified().to_string(),
                "namespace": item.signature.namespace,
                "name": item.signature.name,
                "description": description,
                "params": item.io.params.iter().map(|param| json!({
                    "name": param.name,
                    "type": spec_ref(param.ty),
                })).collect::<Vec<_>>(),
                "returns": spec_ref(item.io.ret),
            })
        })
        .collect()
}

/// 导出 `scene_api.json`（机器可读真源，含两组能力与结构化类型表）。
pub fn scene_api_json() -> String {
    let capabilities: serde_json::Map<String, Value> = capabilities()
        .iter()
        .map(|capability| {
            (
                capability.name.to_string(),
                json!({
                    "version": capability.meta.version,
                    "description": capability.meta.description,
                    "functions": functions_json(capability),
                }),
            )
        })
        .collect();

    let value = json!({
        "api": "xcal.script",
        "capabilities": Value::Object(capabilities),
        "types": reachable_types()
            .into_values()
            .map(spec_def)
            .collect::<Vec<_>>(),
    });
    serde_json::to_string_pretty(&value).expect("serde_json 序列化 Value 不会失败")
}

fn push_description(out: &mut String, text: &str) {
    for line in text.lines() {
        out.push_str("---");
        out.push_str(line);
        out.push('\n');
    }
}

/// 导出 `scene_api.lua`（LuaLS 注解；与 JSON 同源）。
pub fn scene_api_lua() -> String {
    let capabilities = capabilities();
    let types = reachable_types();

    let mut out = String::from(
        "---@meta\n\n---@class Handle xcal 场景句柄（组件 / 对象 / 曲线 / 动作）\n\n",
    );

    // ── 类型（一次）────────────────────────────────────────────────────
    for (full_name, spec) in &types {
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
                let returns = if component_ctor(SceneApi::calls(), full_name).is_some() {
                    "Handle"
                } else {
                    lua_name
                };
                out.push_str(&format!(
                    "---@param spec {lua_name}\n---@return {returns}\nfunction {lua_name}(spec) end\n\n"
                ));
            }
            TypeSpec::Enum { variants, .. } => {
                out.push_str(&format!("---@class {lua_name}\n{lua_name} = {{}}\n\n"));
                for variant in *variants {
                    if variant.fields.is_empty() {
                        // unit 变体是值（不是函数）。
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

    // ── 能力表（scene / content）──────────────────────────────────────
    for capability in &capabilities {
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

fn capitalize(name: &str) -> &'static str {
    match name {
        "scene" => "Scene",
        "content" => "Content",
        _ => "Api",
    }
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
