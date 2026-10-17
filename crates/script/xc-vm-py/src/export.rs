// export.rs — **Python 侧文档生成**：`.pyi` 按 capability 的命名空间路径落盘。
//
// 与 `xc-vm-lua` 的 `lua_ls` 对称：JSON 真源在 `xc-vm-binding`，
// 本文件只负责把同一份 schema 翻译成 Python 类型存根（不依赖 pyo3）。
//
// ```text
// xcal.scene   → xcal/scene.pyi
// xcal.content → xcal/content.pyi
// xcal.python  → xcal/python.pyi
//                xcal/__init__.pyi（Handle + 共享类型 + 子模块再导出）
// ```
//
// 这样 `import xcal.scene` / `from xcal import scene` 与普通 Python 模块一致。

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use xc_vm::{ScalarKind, TypeSpec};
use xc_vm_binding::{BindingPlan, CapabilityDocs, short_name};

/// Python 关键字（不能直接作形参名）；仅对我们 domain 里出现的做最小映射。
fn py_ident(name: &str) -> String {
    match name {
        "from" => "from_".to_string(),
        other => other.to_string(),
    }
}

fn parent_of(full: &str) -> &str {
    full.rsplit_once('.').map(|(parent, _)| parent).unwrap_or("")
}

fn root_of(path: &str) -> &str {
    path.split('.').next().unwrap_or(path)
}

/// `a.b.c` → `["a", "a.b", "a.b.c"]`。
fn ancestors(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = path;
    loop {
        out.push(current.to_string());
        match current.rsplit_once('.') {
            Some((parent, _)) => current = parent,
            None => break,
        }
    }
    out.reverse();
    out
}

/// capability 模块的父包链（不含模块自身）。
fn package_ancestors(module: &str) -> Vec<String> {
    let mut packages = ancestors(module);
    packages.pop();
    packages
}

/// capability 的模块路径：优先取函数签名里的 namespace（如 `xcal.scene`）。
fn capability_path(capability: &CapabilityDocs) -> String {
    capability
        .calls
        .first()
        .map(|item| item.signature.namespace)
        .filter(|namespace| !namespace.is_empty())
        .unwrap_or(capability.name)
        .to_string()
}

/// 收集一个类型（含嵌套字段）引用到的命名类型。
fn collect_named(spec: &'static TypeSpec, into: &mut BTreeSet<&'static str>) {
    match spec {
        TypeSpec::Struct { name, fields } => {
            into.insert(name);
            for field in *fields {
                collect_named(field.ty, into);
            }
        }
        TypeSpec::Enum { name, variants } => {
            into.insert(name);
            for variant in *variants {
                for field in variant.fields {
                    collect_named(field.ty, into);
                }
            }
        }
        TypeSpec::Array { elem } => collect_named(elem, into),
        TypeSpec::Scalar(_) | TypeSpec::Opaque(_) => {}
    }
}

/// 只收集签名**直接引用**的命名类型（不钻进字段；字段类型由声明处导入）。
fn collect_top(spec: &'static TypeSpec, into: &mut BTreeSet<&'static str>) {
    match spec {
        TypeSpec::Struct { name, .. } | TypeSpec::Enum { name, .. } => {
            into.insert(name);
        }
        TypeSpec::Array { elem } => collect_top(elem, into),
        TypeSpec::Scalar(_) | TypeSpec::Opaque(_) => {}
    }
}

/// 组件构造器在运行时是**函数**（返回 `Handle`），不是类；
/// 它们没有可引用的 Python 类型名，作为形参类型时退化为 `Any`。
fn py_type(spec: &'static TypeSpec, components: &BTreeSet<&'static str>) -> String {
    match spec {
        TypeSpec::Scalar(kind) => match kind {
            ScalarKind::F32 | ScalarKind::F64 => "float".to_string(),
            ScalarKind::I32 | ScalarKind::U32 | ScalarKind::U8 => "int".to_string(),
            ScalarKind::Bool => "bool".to_string(),
            ScalarKind::String => "str".to_string(),
        },
        TypeSpec::Struct { name, .. } if *name == "()" => "None".to_string(),
        TypeSpec::Struct { name, .. } if components.contains(name) => "Any".to_string(),
        TypeSpec::Struct { name, .. } | TypeSpec::Enum { name, .. } => {
            short_name(name).to_string()
        }
        TypeSpec::Array { elem } => format!("List[{}]", py_type(elem, components)),
        TypeSpec::Opaque(_) => "Handle".to_string(),
    }
}

fn params(fields: &'static [xc_vm::Field], components: &BTreeSet<&'static str>) -> String {
    fields
        .iter()
        .map(|field| {
            format!(
                "{}: {}",
                py_ident(field.name),
                py_type(field.ty, components)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn push_description(out: &mut String, text: &str) {
    for line in text.lines() {
        out.push_str("# ");
        out.push_str(line);
        out.push('\n');
    }
}

fn push_imports(out: &mut String, imports: &BTreeMap<String, BTreeSet<String>>) {
    for (home, names) in imports {
        out.push_str(&format!(
            "from {home} import {}\n",
            names.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
}

fn render_type(
    out: &mut String,
    full_name: &'static str,
    spec: &'static TypeSpec,
    components: &BTreeSet<&'static str>,
) {
    let name = short_name(full_name);
    match spec {
        TypeSpec::Struct { fields, .. } => {
            if components.contains(full_name) {
                out.push_str(&format!(
                    "def {name}({}) -> Handle: ...\n",
                    params(fields, components)
                ));
            } else {
                out.push_str(&format!("class {name}:\n"));
                if fields.is_empty() {
                    out.push_str("    ...\n");
                    return;
                }
                for field in *fields {
                    out.push_str(&format!(
                        "    {}: {}\n",
                        py_ident(field.name),
                        py_type(field.ty, components)
                    ));
                }
                out.push_str(&format!(
                    "    def __init__(self, {}) -> None: ...\n",
                    params(fields, components)
                ));
            }
        }
        TypeSpec::Enum { variants, .. } => {
            out.push_str(&format!("class {name}:\n"));
            if variants.is_empty() {
                out.push_str("    ...\n");
                return;
            }
            for variant in *variants {
                if variant.fields.is_empty() {
                    out.push_str(&format!("    {}: {}\n", variant.name, name));
                } else {
                    out.push_str(&format!(
                        "    @staticmethod\n    def {}({}) -> {}: ...\n",
                        variant.name,
                        params(variant.fields, components),
                        name
                    ));
                }
            }
        }
        _ => {}
    }
}

/// 生成按 capability 路径组织的 `.pyi` 文件树。
///
/// key 是相对路径（`/` 分隔），例如 `xcal/__init__.pyi` / `xcal/scene.pyi`。
pub fn pyi_files(capabilities: &[CapabilityDocs]) -> BTreeMap<String, String> {
    let types = xc_vm_binding::reachable_types(capabilities);

    // 组件类型（运行时是函数，不产生类）。
    let mut components: BTreeSet<&'static str> = BTreeSet::new();
    for capability in capabilities {
        let plan = BindingPlan::new(capability.calls);
        for (&full_name, &spec) in &plan.types {
            if matches!(spec, TypeSpec::Struct { .. }) && plan.component_ctor(full_name).is_some()
            {
                components.insert(full_name);
            }
        }
    }

    // capability 模块路径 → 文档。
    let mut modules: BTreeMap<String, &CapabilityDocs> = BTreeMap::new();
    for capability in capabilities {
        modules.insert(capability_path(capability), capability);
    }

    let default_root = modules
        .keys()
        .next()
        .map(|module| root_of(module).to_string())
        .unwrap_or_else(|| "xcal".to_string());

    // 类型 → 所属包（类型名去掉最后一段）。
    let mut type_home: BTreeMap<&'static str, String> = BTreeMap::new();
    for &full_name in types.keys() {
        let parent = parent_of(full_name);
        type_home.insert(
            full_name,
            if parent.is_empty() {
                default_root.clone()
            } else {
                parent.to_string()
            },
        );
    }

    // 所有需要生成 `__init__.pyi` 的包。
    let mut packages: BTreeSet<String> = BTreeSet::new();
    for module in modules.keys() {
        for package in package_ancestors(module) {
            packages.insert(package);
        }
    }
    for home in type_home.values() {
        for package in ancestors(home) {
            packages.insert(package);
        }
    }

    // 包 → 直接子模块 / 子包（用于 `from . import x as x`）。
    let mut children: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for module in modules.keys() {
        if let Some((parent, name)) = module.rsplit_once('.') {
            children
                .entry(parent.to_string())
                .or_default()
                .insert(name.to_string());
        }
    }
    for package in &packages {
        if let Some((parent, name)) = package.rsplit_once('.') {
            children
                .entry(parent.to_string())
                .or_default()
                .insert(name.to_string());
        }
    }

    let mut files = BTreeMap::new();

    // ── 包 __init__.pyi：Handle + 共享类型 + 子模块再导出 ───────────────
    for package in &packages {
        let mut out = String::from(
            "# 由 xcal-app 的 export_scene_api 生成；与 scene_api.json 同源。\n\
             from typing import Any, List\n\n",
        );
        let root = root_of(package);
        if package == root {
            out.push_str(
                "class Handle:\n    \"\"\"不透明句柄（组件 / 对象 / 曲线 / 动作）。\"\"\"\n\n",
            );
        } else {
            out.push_str(&format!("from {root} import Handle\n\n"));
        }

        let package_types: Vec<(&'static str, &'static TypeSpec)> = types
            .iter()
            .filter(|(full_name, _)| type_home.get(*full_name) == Some(package))
            .map(|(&full_name, &spec)| (full_name, spec))
            .collect();

        let mut references = BTreeSet::new();
        for (_, spec) in &package_types {
            collect_named(spec, &mut references);
        }
        let mut imports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for name in references {
            if name == "()" || !types.contains_key(name) {
                continue;
            }
            let home = &type_home[name];
            if home != package {
                imports
                    .entry(home.clone())
                    .or_default()
                    .insert(short_name(name).to_string());
            }
        }
        push_imports(&mut out, &imports);

        if let Some(kids) = children.get(package) {
            for child in kids {
                out.push_str(&format!("from . import {child} as {child}\n"));
            }
        }
        out.push('\n');

        for (full_name, spec) in package_types {
            render_type(&mut out, full_name, spec, &components);
            out.push('\n');
        }
        files.insert(format!("{}/__init__.pyi", package.replace('.', "/")), out);
    }

    // ── capability 模块：函数 + 类型再导出 ─────────────────────────────
    for (path, capability) in &modules {
        let mut out = String::from(
            "# 由 xcal-app 的 export_scene_api 生成；与 scene_api.json 同源。\n\
             from typing import Any, List\n",
        );
        let root = root_of(path);
        let needs_handle = capability.calls.iter().any(|item| {
            item.io
                .params
                .iter()
                .any(|param| matches!(param.ty, TypeSpec::Opaque(_)))
                || matches!(item.io.ret, TypeSpec::Opaque(_))
        });
        if needs_handle {
            out.push_str(&format!("from {root} import Handle\n"));
        }

        let mut references = BTreeSet::new();
        for item in capability.calls {
            for param in item.io.params {
                collect_top(param.ty, &mut references);
            }
            collect_top(item.io.ret, &mut references);
        }
        let mut imports: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for name in references {
            // 组件类型在签名里映射为 `Any`，不需要导入（且运行时是函数）。
            if name == "()" || !types.contains_key(name) || components.contains(name) {
                continue;
            }
            imports
                .entry(type_home[name].clone())
                .or_default()
                .insert(short_name(name).to_string());
        }
        push_imports(&mut out, &imports);
        out.push('\n');

        for (index, item) in capability.calls.iter().enumerate() {
            if let Some(doc) = capability.docs.get(index) {
                push_description(&mut out, doc.description);
            }
            let params: Vec<String> = item
                .io
                .params
                .iter()
                .map(|param| {
                    format!(
                        "{}: {}",
                        py_ident(param.name),
                        py_type(param.ty, &components)
                    )
                })
                .collect();
            out.push_str(&format!(
                "def {}({}) -> {}: ...\n\n",
                item.signature.name,
                params.join(", "),
                py_type(item.io.ret, &components)
            ));
        }
        files.insert(format!("{}.pyi", path.replace('.', "/")), out);
    }

    files
}

/// 把 `.pyi` 文件树写到 `dir`（返回实际写入的路径）。
pub fn export_docs(dir: &Path, capabilities: &[CapabilityDocs]) -> io::Result<Vec<PathBuf>> {
    let files = pyi_files(capabilities);
    let mut written = Vec::with_capacity(files.len());
    for (relative, content) in files {
        let path = dir.join(&relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, content)?;
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PythonApi;
    use xc_vm::{Capability, Handle};
    use xc_vm_macro::{VmType, capability};

    #[derive(Clone, Debug, PartialEq, VmType)]
    #[vm_type(name = "xcal.Dot")]
    struct Dot {
        x: f32,
        y: f32,
    }

    #[allow(dead_code)] // capability 的 inherent 方法不全部调用。
    #[capability(namespace = "xcal.scene", bindings)]
    struct MiniScene<'a> {
        /// 创建一个点组件。
        dot: fn(&'a dyn xc_vm::Vm, shape: Dot) -> Handle,
    }

    fn files() -> BTreeMap<String, String> {
        pyi_files(&[
            CapabilityDocs {
                name: "scene",
                calls: MiniScene::calls(),
                docs: MiniScene::docs(),
                meta: MiniScene::meta(),
            },
            CapabilityDocs {
                name: "python",
                calls: PythonApi::calls(),
                docs: PythonApi::docs(),
                meta: PythonApi::meta(),
            },
        ])
    }

    #[test]
    fn pyi_paths_follow_capability_namespaces() {
        let files = files();
        assert!(files.contains_key("xcal/__init__.pyi"));
        assert!(files.contains_key("xcal/scene.pyi"));
        assert!(files.contains_key("xcal/python.pyi"));
    }

    #[test]
    fn pyi_root_declares_handle_and_types() {
        let files = files();
        let root = &files["xcal/__init__.pyi"];
        assert!(root.contains("class Handle:"));
        assert!(root.contains("def Dot("), "组件构造器应是函数");
        assert!(root.contains("from . import scene as scene"));
        assert!(root.contains("from . import python as python"));
    }

    #[test]
    fn pyi_capability_module_reexports_types() {
        let files = files();
        let scene = &files["xcal/scene.pyi"];
        assert!(scene.contains("from xcal import Handle"));
        assert!(scene.contains("def dot("));
        assert!(scene.contains("-> Handle: ..."));
        // 组件类型映射为 Any，不产生 `from xcal import Dot`。
        assert!(!scene.contains("import Dot"));
    }
}
