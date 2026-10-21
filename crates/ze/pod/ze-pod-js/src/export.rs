// export.rs — **语言侧文档生成**：JSON 由通用层产出，TypeScript `.d.ts` 在这里生成。
//
// 本文件不引用任何 domain 类型：能力清单由调用方（App / domain crate）传入。
//
// # 为什么是 `.d.ts`（而不是别的）
//
// JS 侧没有 LuaLS 那种注解方言；TypeScript 声明文件是**工具链原生**的补全来源，
// 编辑器（VSCode / WebStorm）直接识别，不需要插件。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use ze_pod_binding::{BindingPlan, CapabilityDocs, short_name};
use ze_pod_core::{ScalarKind, TypeSpec};

/// JS 侧类型名（TypeScript 声明用）。
///
/// 注意：JS **只有 number** —— `i32` / `u32` / `u64` / `u8` 都落成 `number`，
/// 整数语义由绑定层显式校验（越界报错，不静默截断）。
pub fn ts_type_name(spec: &'static TypeSpec) -> String {
    match spec {
        TypeSpec::Scalar(kind) => match kind {
            ScalarKind::F32 | ScalarKind::F64 => "number".to_string(),
            ScalarKind::I32 | ScalarKind::U32 | ScalarKind::U64 | ScalarKind::U8 => {
                "number".to_string()
            }
            ScalarKind::Bool => "boolean".to_string(),
            ScalarKind::String => "string".to_string(),
        },
        TypeSpec::Struct { name, .. } | TypeSpec::Enum { name, .. } => {
            short_name(name).to_string()
        }
        TypeSpec::Array { elem } => format!("{}[]", ts_type_name(elem)),
        TypeSpec::Opaque(_) => "Handle".to_string(),
    }
}

/// 从 `CapabilityDocs` 生成 TypeScript 声明（**无 domain 知识**）。
pub fn d_ts(capabilities: &[CapabilityDocs]) -> String {
    let types = ze_pod_binding::reachable_types(capabilities);

    let mut out = String::from(
        "// 自动生成：zela 脚本能力声明（TypeScript）\n\
         // JS 只有 number ⇒ i32/u32/u64/u8 一律映射为 number，整数语义在运行时校验。\n\n\
         /** zela 场景句柄（组件 / 对象 / 曲线 / 动作） */\n\
         export type Handle = { readonly __zela_handle: unique symbol };\n\n",
    );

    // ── 类型（一次）────────────────────────────────────────────────────
    for (&full_name, &spec) in &types {
        let name = short_name(full_name);
        match spec {
            TypeSpec::Struct { fields, .. } => {
                out.push_str(&format!("export interface {name} {{\n"));
                for field in *fields {
                    out.push_str(&format!("  {}: {};\n", field.name, ts_type_name(field.ty)));
                }
                out.push_str("}\n");
                let component = capabilities.iter().any(|capability| {
                    BindingPlan::new(capability.calls)
                        .component_ctor(full_name)
                        .is_some()
                });
                let returns = if component { "Handle" } else { name };
                out.push_str(&format!(
                    "export declare function {name}(spec: {name}): {returns};\n\n"
                ));
            }
            TypeSpec::Enum { variants, .. } => {
                out.push_str(&format!("export type {name} = {name}.Value;\n"));
                out.push_str(&format!("export declare namespace {name} {{\n"));
                out.push_str("  interface Value { readonly __zela_enum: unique symbol }\n");
                for variant in *variants {
                    if variant.fields.is_empty() {
                        out.push_str(&format!("  const {0}: Value;\n", variant.name));
                        continue;
                    }
                    let fields: Vec<String> = variant
                        .fields
                        .iter()
                        .map(|field| format!("{}: {}", field.name, ts_type_name(field.ty)))
                        .collect();
                    out.push_str(&format!(
                        "  function {}(spec: {{ {} }}): Value;\n",
                        variant.name,
                        fields.join("; ")
                    ));
                }
                out.push_str("}\n\n");
            }
            _ => {}
        }
    }

    // ── 能力表 ────────────────────────────────────────────────────────
    for capability in capabilities {
        out.push_str(&format!("export declare const {}: {{\n", capability.name));
        for item in capability.calls {
            let params: Vec<String> = item
                .io
                .params
                .iter()
                .map(|param| format!("{}: {}", param.name, ts_type_name(param.ty)))
                .collect();
            let returns = match item.io.ret {
                TypeSpec::Struct { name: "()", .. } => "void".to_string(),
                other => ts_type_name(other),
            };
            out.push_str(&format!(
                "  {}({}): {returns};\n",
                item.signature.name,
                params.join(", ")
            ));
        }
        out.push_str("};\n\n");
    }
    out
}

/// 把 JSON + `.d.ts` 写到 `dir`（返回实际写入的两个路径）。
pub fn export_docs(dir: &Path, capabilities: &[CapabilityDocs]) -> io::Result<[PathBuf; 2]> {
    fs::create_dir_all(dir)?;
    let json_path = dir.join("scene_api.json");
    let ts_path = dir.join("scene_api.d.ts");
    fs::write(&json_path, ze_pod_binding::capabilities_json(capabilities))?;
    fs::write(&ts_path, d_ts(capabilities))?;
    Ok([json_path, ts_path])
}
