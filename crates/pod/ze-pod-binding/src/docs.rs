//! **语言无关**的文档导出：JSON 是唯一真源。
//!
//! 各语言适配器从同一份 [`CapabilityDocs`] 生成自己的注解（LuaLS / stub …），
//! 不再各自维护一份类型表。
//!
//! 本模块零外部依赖（手写 JSON），保证 `cargo tree -p ze-pod-binding` 只有
//! `ze-pod` + 自身。

use std::collections::BTreeMap;
use std::fmt::Write;

use ze_pod::{CapabilityMeta, ErasedFn, FnDoc, ScalarKind, TypeSpec};

use crate::plan::collect_types;

/// 一个 capability 的导出视图。
#[derive(Clone, Copy)]
pub struct CapabilityDocs {
    pub name: &'static str,
    pub calls: &'static [ErasedFn],
    pub docs: &'static [FnDoc],
    pub meta: CapabilityMeta,
}

/// 所有能力可达的类型（递归收集、按名字去重）。
pub fn reachable_types(
    capabilities: &[CapabilityDocs],
) -> BTreeMap<&'static str, &'static TypeSpec> {
    let mut types = BTreeMap::new();
    for capability in capabilities {
        for item in capability.calls {
            for param in item.io.params {
                collect_types(param.ty, &mut types);
            }
            collect_types(item.io.ret, &mut types);
        }
    }
    types
}

fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

fn opt_str(value: Option<&'static str>) -> String {
    match value {
        Some(text) => format!("\"{}\"", escape(text)),
        None => "null".to_string(),
    }
}

fn scalar_name(kind: ScalarKind) -> &'static str {
    match kind {
        ScalarKind::F32 => "f32",
        ScalarKind::F64 => "f64",
        ScalarKind::I32 => "i32",
        ScalarKind::U32 => "u32",
        ScalarKind::U64 => "u64",
        ScalarKind::U8 => "u8",
        ScalarKind::Bool => "bool",
        ScalarKind::String => "String",
    }
}

/// 类型**引用**（命名类型只给名字，定义在顶层 `types` 表里）。
fn spec_ref(spec: &'static TypeSpec) -> String {
    match spec {
        TypeSpec::Scalar(kind) => {
            format!("{{\"kind\":\"scalar\",\"name\":\"{}\"}}", scalar_name(*kind))
        }
        TypeSpec::Struct { name, .. } => {
            format!("{{\"kind\":\"struct\",\"name\":\"{}\"}}", escape(name))
        }
        TypeSpec::Enum { name, .. } => {
            format!("{{\"kind\":\"enum\",\"name\":\"{}\"}}", escape(name))
        }
        TypeSpec::Array { elem } => {
            format!("{{\"kind\":\"array\",\"elem\":{}}}", spec_ref(elem))
        }
        TypeSpec::Opaque(name) => {
            format!("{{\"kind\":\"opaque\",\"name\":\"{}\"}}", escape(name))
        }
    }
}

fn fields_json(fields: &'static [ze_pod::Field]) -> String {
    let items: Vec<String> = fields
        .iter()
        .map(|field| {
            format!(
                "{{\"name\":\"{}\",\"type\":{}}}",
                escape(field.name),
                spec_ref(field.ty)
            )
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// 类型**定义**（只用于顶层 `types` 表）。
fn spec_def(spec: &'static TypeSpec) -> String {
    match spec {
        TypeSpec::Struct { name, fields } => format!(
            "{{\"kind\":\"struct\",\"name\":\"{}\",\"fields\":{}}}",
            escape(name),
            fields_json(fields)
        ),
        TypeSpec::Enum { name, variants } => {
            let items: Vec<String> = variants
                .iter()
                .map(|variant| {
                    format!(
                        "{{\"name\":\"{}\",\"fields\":{}}}",
                        escape(variant.name),
                        fields_json(variant.fields)
                    )
                })
                .collect();
            format!(
                "{{\"kind\":\"enum\",\"name\":\"{}\",\"variants\":[{}]}}",
                escape(name),
                items.join(",")
            )
        }
        other => spec_ref(other),
    }
}

/// 导出 JSON（`api` + `capabilities` + `types`）。
pub fn capabilities_json(capabilities: &[CapabilityDocs]) -> String {
    let types = reachable_types(capabilities);
    let mut out = String::new();
    out.push_str("{\n  \"api\": \"xcal.script\",\n  \"capabilities\": {\n");

    for (cap_index, capability) in capabilities.iter().enumerate() {
        if cap_index > 0 {
            out.push_str(",\n");
        }
        let _ = writeln!(out, "    \"{}\": {{", escape(capability.name));
        let _ = writeln!(
            out,
            "      \"version\": {},\n      \"description\": {},\n      \"functions\": [",
            opt_str(capability.meta.version),
            opt_str(capability.meta.description)
        );
        for (index, item) in capability.calls.iter().enumerate() {
            if index > 0 {
                out.push_str(",\n");
            }
            let description = capability
                .docs
                .get(index)
                .map(|doc| doc.description)
                .unwrap_or("");
            out.push_str("        {\n");
            let _ = writeln!(
                out,
                "          \"qualified\": \"{}\",",
                escape(&item.signature.qualified().to_string())
            );
            let _ = writeln!(
                out,
                "          \"namespace\": \"{}\",\n          \"name\": \"{}\",",
                escape(item.signature.namespace),
                escape(item.signature.name)
            );
            let _ = writeln!(
                out,
                "          \"description\": \"{}\",",
                escape(description)
            );
            out.push_str("          \"params\": [");
            for (param_index, param) in item.io.params.iter().enumerate() {
                if param_index > 0 {
                    out.push(',');
                }
                let _ = write!(
                    out,
                    "{{\"name\":\"{}\",\"type\":{}}}",
                    escape(param.name),
                    spec_ref(param.ty)
                );
            }
            out.push_str("],\n");
            let _ = writeln!(
                out,
                "          \"returns\": {}",
                spec_ref(item.io.ret)
            );
            out.push_str("        }");
        }
        out.push_str("\n      ]\n    }");
    }

    out.push_str("\n  },\n  \"types\": [\n");
    for (index, (_, spec)) in types.iter().enumerate() {
        if index > 0 {
            out.push_str(",\n");
        }
        let _ = write!(out, "    {}", spec_def(spec));
    }
    out.push_str("\n  ]\n}\n");
    out
}


