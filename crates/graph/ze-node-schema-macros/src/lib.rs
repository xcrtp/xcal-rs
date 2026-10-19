//! Proc-macro support for `zela-node-ui`.
//!
//! Provides [`macro@NodeSpec`] (`#[derive(NodeSpec)]`) which, together with the
//! `#[node(...)]` and `#[property(...)]` helper attributes, generates the full
//! `Node` + `NodeSpec` impls plus a `default_node()` constructor for a plain
//! struct — zero hand-written boilerplate. The generated `apply_property` uses
//! exhaustive name + variant matching (no `downcast`).
//!
//! ## Example
//! ```ignore
//! #[derive(NodeSpec)]
//! #[node(type = "demo.constant.f32", display = "Constant", category = "Source",
//!        outputs = ["f32"], state = "()")]
//! struct ConstantF32 {
//!     #[property(label = "value", range = 0.0..=10.0, step = 0.01)]
//!     value: f32,
//! }
//! // -> impl Node for ConstantF32 { ... }
//! // -> impl NodeSpec for ConstantF32 { ... }
//! // -> impl ConstantF32 { pub fn default_node() -> Box<dyn Node>; pub const CATEGORY: &str; }
//! ```
//!
//! Supported field types (常用子集): `f32`, `f64`,
//! `i32`/`i64`/`u32`/`u64`/`isize`/`usize`, `bool`, `String`, `[f32; 2|3|4]`,
//! `[u8; 4]`. Other property variants (Enum / Range / Function / Dynamic /
//! List) are intentionally out of scope for the derive for now and can be added
//! later without breaking already-generated code (forward-compatible).

use proc_macro::TokenStream;
use proc_macro2::{Ident, Span, TokenStream as TokenStream2};
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Expr, ExprArray, ExprLit, ExprRange, Field, Fields, ItemStruct, Lit, MetaNameValue,
    Token, Type,
};

/// Entry point for `#[derive(NodeSpec)]`.
#[proc_macro_derive(NodeSpec, attributes(node, property))]
pub fn derive_node_spec(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as ItemStruct);
    match expand(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

// ---------------------------------------------------------------------------
// Parsed attribute model
// ---------------------------------------------------------------------------

#[derive(Default)]
struct NodeAttr {
    type_name: Option<String>,
    display: Option<String>,
    category: Option<String>,
    state: Option<String>,
    dirty: Option<String>,
    inputs: Vec<String>,
    outputs: Vec<String>,
    /// Interaction policy overrides (None = keep the permissive default).
    draggable: Option<bool>,
    selectable: Option<bool>,
    resizable: Option<bool>,
}

#[derive(Default)]
struct PropertyAttr {
    label: Option<String>,
    range: Option<(Expr, Expr)>,
    min: Option<Expr>,
    max: Option<Expr>,
    step: Option<Expr>,
    unit: Option<String>,
    default: Option<Expr>,
}

/// Kind of property a field maps to, derived from its Rust type.
enum FieldKind {
    ScalarF32,
    ScalarF64,
    Integer,
    Bool,
    String,
    VecN(u8),
    Color,
}

struct PropField {
    name: String,
    accessor: Ident,
    attr: PropertyAttr,
    kind: FieldKind,
}

// ---------------------------------------------------------------------------
// Expansion
// ---------------------------------------------------------------------------

fn expand(item: &ItemStruct) -> syn::Result<TokenStream2> {
    let node_attr = parse_node_attr(item)?;
    let vis = &item.vis;
    let struct_name = &item.ident;

    let fields = match &item.fields {
        Fields::Named(named) => &named.named,
        _ => {
            return Err(syn::Error::new_spanned(
                &item.ident,
                "NodeSpec derive requires a struct with named fields",
            ));
        }
    };

    let prop_fields = collect_prop_fields(fields)?;

    let type_name = node_attr.type_name.ok_or_else(|| {
        syn::Error::new_spanned(struct_name, "missing #[node(type_name = \"...\")]")
    })?;
    let display = node_attr.display.unwrap_or_else(|| struct_name.to_string());
    let category = node_attr.category.unwrap_or_else(|| "Misc".to_string());
    let state = node_attr.state.unwrap_or_else(|| "()".to_string());
    let dirty = node_attr.dirty.unwrap_or_else(|| "always".to_string());
    let dirty_tokens = dirty_tokens(&dirty)?;

    // Interaction policy overrides (only emitted when explicitly set).
    let mut interaction_overrides: Vec<TokenStream2> = Vec::new();
    if let Some(v) = node_attr.draggable {
        interaction_overrides.push(quote! { meta.interaction.draggable = #v; });
    }
    if let Some(v) = node_attr.selectable {
        interaction_overrides.push(quote! { meta.interaction.selectable = #v; });
    }
    if let Some(v) = node_attr.resizable {
        interaction_overrides.push(quote! { meta.interaction.resizable = #v; });
    }
    let meta_init = if interaction_overrides.is_empty() {
        quote! { ::ze_node::NodeMeta::new(#type_name, #display, 1) }
    } else {
        quote! {
            {
                let mut meta = ::ze_node::NodeMeta::new(#type_name, #display, 1);
                #(#interaction_overrides)*
                meta
            }
        }
    };
    let inputs_init = build_socket_vec(&node_attr.inputs, true);
    let outputs_init = build_socket_vec(&node_attr.outputs, false);

    let meta_static = format_ident("__NODE_SPEC_META_", struct_name);
    let inputs_static = format_ident("__NODE_SPEC_INPUTS_", struct_name);
    let outputs_static = format_ident("__NODE_SPEC_OUTPUTS_", struct_name);

    // ---- properties() body ----
    let entries_len = prop_fields.len();
    let prop_entries: Vec<TokenStream2> = prop_fields
        .iter()
        .map(|pf| {
            let name = &pf.name;
            let ctor = pf.variant_constructor();
            quote! { vec.push((#name.into(), #ctor)); }
        })
        .collect();
    let props_body = quote! {
        let mut vec = ::std::vec::Vec::with_capacity(#entries_len);
        #(#prop_entries)*
        vec
    };

    // ---- apply_property body: single exhaustive match on (name, value) ----
    let mut arms: Vec<TokenStream2> = Vec::new();
    for pf in &prop_fields {
        let name = &pf.name;
        let ok_pat = pf.ok_pattern();
        let ok_body = pf.ok_body();
        let expected = pf.expected_kind_name();
        arms.push(quote! {
            (#name, #ok_pat) => { #ok_body },
        });
        arms.push(quote! {
            (#name, _v) => ::core::result::Result::Err(::ze_node_schema::PropertyError::TypeMismatch {
                expected: #expected.into(),
                got: _v.kind_name().into(),
            }),
        });
    }
    let apply_body = if arms.is_empty() {
        quote! {
            let _ = value;
            ::core::result::Result::Err(::ze_node_schema::PropertyError::UnknownProperty(name.into()))
        }
    } else {
        quote! {
            match (name, value) {
                #(#arms)*
                (_, _v) => ::core::result::Result::Err(
                    ::ze_node_schema::PropertyError::UnknownProperty(name.into())
                ),
            }
        }
    };

    // ---- default_node + Default (field defaults) ----
    let default_inits: Vec<TokenStream2> = fields
        .iter()
        .map(|f| {
            let fname = f.ident.as_ref().unwrap();
            if let Some(pa) = find_property_attr(&f.attrs) {
                if let Some(d) = &pa.default {
                    quote! { #fname: #d }
                } else {
                    quote! { #fname: ::core::default::Default::default() }
                }
            } else {
                quote! { #fname: ::core::default::Default::default() }
            }
        })
        .collect();

    let expanded = quote! {
        impl ::ze_node::Node for #struct_name {
            fn meta(&self) -> &'static ::ze_node::NodeMeta {
                static #meta_static: ::std::sync::OnceLock<::ze_node::NodeMeta> =
                    ::std::sync::OnceLock::new();
                #meta_static.get_or_init(|| #meta_init)
            }
            fn inputs(&self) -> &'static [::ze_node::InputSocket] {
                static #inputs_static: ::std::sync::OnceLock<::std::vec::Vec<::ze_node::InputSocket>> =
                    ::std::sync::OnceLock::new();
                #inputs_static.get_or_init(|| #inputs_init).as_slice()
            }
            fn outputs(&self) -> &'static [::ze_node::OutputSocket] {
                static #outputs_static: ::std::sync::OnceLock<::std::vec::Vec<::ze_node::OutputSocket>> =
                    ::std::sync::OnceLock::new();
                #outputs_static.get_or_init(|| #outputs_init).as_slice()
            }
            fn dirty_policy(&self) -> ::ze_node::DirtyPolicy {
                #dirty_tokens
            }
            fn state_type(&self) -> ::std::boxed::Box<dyn ::ze_node::TypeTag> {
                ::std::boxed::Box::new(::ze_node::NamedTypeTag::new(#state))
            }
            fn as_any(&self) -> &dyn ::std::any::Any { self }
            fn as_any_mut(&mut self) -> &mut dyn ::std::any::Any { self }
        }

        impl ::ze_node_schema::NodeSpec for #struct_name {
            fn properties(
                &self,
            ) -> ::std::vec::Vec<(::std::string::String, ::ze_node_schema::Property)> {
                #props_body
            }
            fn apply_property(
                &mut self,
                name: &str,
                value: ::ze_node_schema::PropertyValue,
            ) -> ::core::result::Result<(), ::ze_node_schema::PropertyError> {
                #apply_body
            }
        }

        impl #struct_name {
            /// Category declared via `#[node(category = ...)]`. Use with
            /// `NodeRegistry::register::<Self>(Self::CATEGORY, Self::default_node)`.
            pub const CATEGORY: &'static str = #category;

            /// Construct a node with default property values, boxed for the
            /// `NodeRegistry` factory.
            #vis fn default_node() -> ::std::boxed::Box<dyn ::ze_node::Node> {
                ::std::boxed::Box::new(Self::default())
            }
        }

        impl ::core::default::Default for #struct_name {
            fn default() -> Self {
                Self { #(#default_inits),* }
            }
        }
    };
    Ok(expanded)
}

// ---------------------------------------------------------------------------
// Per-field codegen
// ---------------------------------------------------------------------------

impl PropField {
    /// `properties()` entry: build the `Property::Variant(Config { ... })`
    /// using `self.<field>` for the current value.
    fn variant_constructor(&self) -> TokenStream2 {
        let label = self.attr.label.clone().unwrap_or_else(|| self.name.clone());
        let field = &self.accessor;
        let (min_expr, max_expr) = match &self.attr.range {
            Some((lo, hi)) => (Some(lo.clone()), Some(hi.clone())),
            None => (self.attr.min.clone(), self.attr.max.clone()),
        };
        let step = &self.attr.step;
        let unit = &self.attr.unit;
        match &self.kind {
            FieldKind::ScalarF32 => {
                let min_t = min_expr
                    .clone()
                    .unwrap_or_else(|| parse_expr("f32::NEG_INFINITY"));
                let max_t = max_expr
                    .clone()
                    .unwrap_or_else(|| parse_expr("f32::INFINITY"));
                let step_t = step_opt(step);
                let unit_t = unit_opt(unit);
                quote! {
                    ::ze_node_schema::Property::Scalar(::ze_node_schema::ScalarConfig {
                        label: ::std::string::String::from(#label),
                        value: self.#field,
                        min: #min_t,
                        max: #max_t,
                        step: #step_t,
                        unit: #unit_t,
                    })
                }
            }
            FieldKind::ScalarF64 => {
                let min_t = min_expr
                    .clone()
                    .unwrap_or_else(|| parse_expr("f64::NEG_INFINITY"));
                let max_t = max_expr
                    .clone()
                    .unwrap_or_else(|| parse_expr("f64::INFINITY"));
                let step_t = step_opt(step);
                let unit_t = unit_opt(unit);
                quote! {
                    ::ze_node_schema::Property::Double(::ze_node_schema::DoubleConfig {
                        label: ::std::string::String::from(#label),
                        value: self.#field,
                        min: #min_t,
                        max: #max_t,
                        step: #step_t,
                        unit: #unit_t,
                    })
                }
            }
            FieldKind::Integer => {
                let min_t = min_expr.clone().unwrap_or_else(|| parse_expr("i64::MIN"));
                let max_t = max_expr.clone().unwrap_or_else(|| parse_expr("i64::MAX"));
                let step_t = step_opt(step);
                let unit_t = unit_opt(unit);
                quote! {
                    ::ze_node_schema::Property::Integer(::ze_node_schema::IntegerConfig {
                        label: ::std::string::String::from(#label),
                        value: self.#field as i64,
                        min: #min_t as i64,
                        max: #max_t as i64,
                        step: #step_t,
                        unit: #unit_t,
                    })
                }
            }
            FieldKind::Bool => quote! {
                ::ze_node_schema::Property::Bool(::ze_node_schema::BoolConfig {
                    label: ::std::string::String::from(#label),
                    value: self.#field,
                })
            },
            FieldKind::String => quote! {
                ::ze_node_schema::Property::String(::ze_node_schema::StringConfig {
                    label: ::std::string::String::from(#label),
                    value: self.#field.clone(),
                    kind: ::ze_node_schema::StringKind::Text,
                })
            },
            FieldKind::VecN(size) => {
                let size_v: u8 = *size;
                let min_t = match &self.attr.min {
                    Some(m) => quote! { ::core::option::Option::Some(#m) },
                    None => quote! { ::core::option::Option::None },
                };
                let max_t = match &self.attr.max {
                    Some(m) => quote! { ::core::option::Option::Some(#m) },
                    None => quote! { ::core::option::Option::None },
                };
                let step_t = step_opt(step);
                quote! {
                    ::ze_node_schema::Property::VecN(::ze_node_schema::VecNConfig {
                        label: ::std::string::String::from(#label),
                        size: #size_v,
                        components: self.#field,
                        min: #min_t,
                        max: #max_t,
                        step: #step_t,
                    })
                }
            }
            FieldKind::Color => quote! {
                ::ze_node_schema::Property::Color(::ze_node_schema::ColorConfig {
                    label: ::std::string::String::from(#label),
                    value: self.#field,
                    with_alpha: true,
                })
            },
        }
    }

    /// The success-match pattern for this field's `PropertyValue` variant.
    fn ok_pattern(&self) -> TokenStream2 {
        match &self.kind {
            FieldKind::ScalarF32 => quote! { ::ze_node_schema::PropertyValue::Scalar(v) },
            FieldKind::ScalarF64 => quote! { ::ze_node_schema::PropertyValue::Double(v) },
            FieldKind::Integer => quote! { ::ze_node_schema::PropertyValue::Integer(v) },
            FieldKind::Bool => quote! { ::ze_node_schema::PropertyValue::Bool(v) },
            FieldKind::String => quote! { ::ze_node_schema::PropertyValue::String(v) },
            FieldKind::VecN(_) => {
                quote! { ::ze_node_schema::PropertyValue::VecN { components: v, .. } }
            }
            FieldKind::Color => quote! { ::ze_node_schema::PropertyValue::Color(v) },
        }
    }

    /// The success-arm body: assign the extracted value to the field.
    fn ok_body(&self) -> TokenStream2 {
        let field = &self.accessor;
        match &self.kind {
            FieldKind::Integer => quote! {
                self.#field = v as _;
                ::core::result::Result::Ok(())
            },
            _ => quote! {
                self.#field = v;
                ::core::result::Result::Ok(())
            },
        }
    }

    fn expected_kind_name(&self) -> &'static str {
        match &self.kind {
            FieldKind::ScalarF32 => "scalar",
            FieldKind::ScalarF64 => "double",
            FieldKind::Integer => "integer",
            FieldKind::Bool => "bool",
            FieldKind::String => "string",
            FieldKind::VecN(_) => "vec_n",
            FieldKind::Color => "color",
        }
    }
}

// ---------------------------------------------------------------------------
// Attribute parsing helpers
// ---------------------------------------------------------------------------

fn parse_node_attr(item: &ItemStruct) -> syn::Result<NodeAttr> {
    let mut out = NodeAttr::default();
    for attr in &item.attrs {
        if !attr.path().is_ident("node") {
            continue;
        }
        let parsed =
            attr.parse_args_with(Punctuated::<MetaNameValue, Token![,]>::parse_terminated)?;
        for nv in parsed {
            let key = nv.path.get_ident().ok_or_else(|| {
                syn::Error::new_spanned(&nv.path, "expected simple identifier in #[node(...)]")
            })?;
            match key.to_string().as_str() {
                "type_name" => out.type_name = Some(expr_to_string(&nv.value)?),
                "display" => out.display = Some(expr_to_string(&nv.value)?),
                "category" => out.category = Some(expr_to_string(&nv.value)?),
                "state" => out.state = Some(expr_to_string(&nv.value)?),
                "dirty" => out.dirty = Some(expr_to_string(&nv.value)?),
                "inputs" => out.inputs = expr_to_str_array(&nv.value)?,
                "outputs" => out.outputs = expr_to_str_array(&nv.value)?,
                "draggable" => out.draggable = Some(expr_to_bool(&nv.value)?),
                "selectable" => out.selectable = Some(expr_to_bool(&nv.value)?),
                "resizable" => out.resizable = Some(expr_to_bool(&nv.value)?),
                other => {
                    return Err(syn::Error::new_spanned(
                        key,
                        format!("unknown #[node] key `{other}`"),
                    ));
                }
            }
        }
    }
    Ok(out)
}

fn find_property_attr(attrs: &[Attribute]) -> Option<PropertyAttr> {
    let mut out = PropertyAttr::default();
    let mut found = false;
    for attr in attrs {
        if !attr.path().is_ident("property") {
            continue;
        }
        found = true;
        let parsed =
            match attr.parse_args_with(Punctuated::<MetaNameValue, Token![,]>::parse_terminated) {
                Ok(p) => p,
                Err(_) => continue,
            };
        for nv in parsed {
            let Some(key) = nv.path.get_ident() else {
                continue;
            };
            match key.to_string().as_str() {
                "label" => out.label = Some(expr_to_string(&nv.value).unwrap_or_default()),
                "range" => out.range = Some(expr_to_range(&nv.value)),
                "min" => out.min = Some(nv.value.clone()),
                "max" => out.max = Some(nv.value.clone()),
                "step" => out.step = Some(nv.value.clone()),
                "unit" => out.unit = Some(expr_to_string(&nv.value).unwrap_or_default()),
                "default" => out.default = Some(nv.value.clone()),
                _ => {}
            }
        }
    }
    found.then_some(out)
}

fn collect_prop_fields(fields: &Punctuated<Field, Token![,]>) -> syn::Result<Vec<PropField>> {
    let mut out = Vec::new();
    for f in fields {
        let Some(attr) = find_property_attr(&f.attrs) else {
            continue;
        };
        let ident = f.ident.as_ref().unwrap();
        let kind = classify_type(&f.ty).ok_or_else(|| {
            syn::Error::new_spanned(
                &f.ty,
                "NodeSpec derive: unsupported property field type; supported: f32, f64, i32/i64/u32/u64/isize/usize, bool, String, [f32;2|3|4], [u8;4]",
            )
        })?;
        out.push(PropField {
            name: ident.to_string(),
            accessor: ident.clone(),
            attr,
            kind,
        });
    }
    Ok(out)
}

fn classify_type(ty: &Type) -> Option<FieldKind> {
    let s = type_to_string(ty);
    match s.as_str() {
        "f32" => Some(FieldKind::ScalarF32),
        "f64" => Some(FieldKind::ScalarF64),
        "i32" | "i64" | "u32" | "u64" | "isize" | "usize" => Some(FieldKind::Integer),
        "bool" => Some(FieldKind::Bool),
        "String" => Some(FieldKind::String),
        _ => {
            if let Type::Array(arr) = ty {
                let elem = type_to_string(&arr.elem);
                let len = expr_to_usize(&arr.len)?;
                match (elem.as_str(), len) {
                    ("f32", 2) => Some(FieldKind::VecN(2)),
                    ("f32", 3) => Some(FieldKind::VecN(3)),
                    ("f32", 4) => Some(FieldKind::VecN(4)),
                    ("u8", 4) => Some(FieldKind::Color),
                    _ => None,
                }
            } else {
                None
            }
        }
    }
}

fn type_to_string(ty: &Type) -> String {
    quote::ToTokens::to_token_stream(ty)
        .to_string()
        .replace(' ', "")
}

fn expr_to_usize(e: &Expr) -> Option<usize> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Int(i), ..
    }) = e
    {
        return i.base10_parse::<usize>().ok();
    }
    None
}

fn expr_to_string(e: &Expr) -> syn::Result<String> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = e
    {
        return Ok(s.value());
    }
    Err(syn::Error::new_spanned(e, "expected a string literal"))
}

fn expr_to_bool(e: &Expr) -> syn::Result<bool> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Bool(b), ..
    }) = e
    {
        return Ok(b.value);
    }
    Err(syn::Error::new_spanned(e, "expected a boolean literal"))
}

fn expr_to_str_array(e: &Expr) -> syn::Result<Vec<String>> {
    let Expr::Array(ExprArray { elems, .. }) = e else {
        return Err(syn::Error::new_spanned(e, "expected an array literal"));
    };
    let mut out = Vec::new();
    for el in elems {
        out.push(expr_to_string(el)?);
    }
    Ok(out)
}

fn expr_to_range(e: &Expr) -> (Expr, Expr) {
    if let Expr::Range(ExprRange { start, end, .. }) = e {
        let lo = start
            .clone()
            .map(|b| *b)
            .unwrap_or_else(|| syn::parse_quote!(::core::default::Default::default()));
        let hi = end
            .clone()
            .map(|b| *b)
            .unwrap_or_else(|| syn::parse_quote!(::core::default::Default::default()));
        return (lo, hi);
    }
    (
        syn::parse_quote!(f32::NEG_INFINITY),
        syn::parse_quote!(f32::INFINITY),
    )
}

fn step_opt(step: &Option<Expr>) -> TokenStream2 {
    match step {
        Some(e) => quote! { ::core::option::Option::Some(#e) },
        None => quote! { ::core::option::Option::None },
    }
}

fn unit_opt(unit: &Option<String>) -> TokenStream2 {
    match unit {
        Some(s) => quote! { ::core::option::Option::Some(::std::string::String::from(#s)) },
        None => quote! { ::core::option::Option::None },
    }
}

fn parse_expr(src: &str) -> Expr {
    syn::parse_str::<Expr>(src).expect("internal: bad default expr")
}

fn build_socket_vec(tags: &[String], is_input: bool) -> TokenStream2 {
    let entries: Vec<TokenStream2> = tags
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let idx = i as u32;
            let name = t;
            if is_input {
                quote! {
                    ::ze_node::InputSocket::new(
                        ::std::boxed::Box::new(::ze_node::NamedTypeTag::new(#name))
                    ).with_index(#idx)
                }
            } else {
                quote! {
                    ::ze_node::OutputSocket::new(
                        ::std::boxed::Box::new(::ze_node::NamedTypeTag::new(#name))
                    ).with_index(#idx)
                }
            }
        })
        .collect();
    quote! { ::std::vec![ #(#entries),* ] }
}

fn dirty_tokens(s: &str) -> syn::Result<TokenStream2> {
    match s {
        "always" => Ok(quote! { ::ze_node::DirtyPolicy::Always }),
        "never" => Ok(quote! { ::ze_node::DirtyPolicy::Never }),
        "on_input_change" | "on-input" | "oninput" => {
            Ok(quote! { ::ze_node::DirtyPolicy::OnInputChange })
        }
        other => Err(syn::Error::new(
            Span::call_site(),
            format!("unknown dirty policy `{other}` (use always|never|on_input_change)"),
        )),
    }
}

fn format_ident(prefix: &str, base: &Ident) -> Ident {
    // SCREAMING_SNAKE_CASE for static variables (Rust convention `non_upper_case_globals`).
    let upper = base
        .to_string()
        .to_uppercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>();
    Ident::new(&format!("{prefix}{upper}"), base.span())
}
