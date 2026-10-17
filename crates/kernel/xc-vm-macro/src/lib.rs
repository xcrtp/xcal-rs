//! `#[capability]`：把一个"函数集合" struct 变成运行时协商出的 typed 句柄。
//!
//! # 输入
//!
//! ```ignore
//! #[capability(namespace = "xcal.math", version = "1", description = "数学函数")]
//! struct MathVm<'a> {
//!     sin: fn(&'a dyn Vm, f32) -> f32,
//!     #[vm_type(param0 = "xcal.Path2D", ret = "xcal.Vec2")]
//!     sample: fn(&'a dyn Vm, Path2D, f32) -> Vec2,
//!     // 方法语义：次参为 `Handle` ⇒ receiver（C 风格 `obj_method(vm, obj, args)`）
//!     bounds: fn(&'a dyn Vm, Handle) -> f32,
//! }
//! ```
//!
//! 每个字段就是一个函数：字段名 = 函数名；形参（**除第一个 `&dyn Vm` 外**）与返回值
//! 就是最终方法签名。第一个形参由宏剥掉（VM 由句柄自己持有）。
//!
//! # 生成
//!
//! 1. struct 本身：`__vm: &'a dyn Vm` + 各函数的裸 fn 指针；
//! 2. inherent 方法：`pub fn sin(&self, x: f32) -> f32` —— 调用 = 一次间接寻址；
//!    receiver 字段生成 `pub fn bounds(&self, receiver: Handle) -> f32`；
//! 3. `impl Capability<'a>`：`signatures()`（声明逐条对应）、`meta()`（可选）与
//!    `from_negotiated()`。
//!
//! # 类型名
//!
//! 未显式标注时生成 `<T as ::xc_vm::IntoVmTypeName>::NAME`：
//! 类型未实现该 trait ⇒ **编译错误**（这就是"缺失即编译错误"的落点）。
//! `#[vm_type = "..."]` 覆盖返回类型；`#[vm_type(paramN = "...", ret = "...")]`
//! 精确覆盖第 N 个参数与返回类型（优先级高于自动推断）。
#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::{Field, Fields, Ident, ItemStruct, Lifetime, LitStr, ReturnType, Type, Visibility};

/// capability 的 VM 借用字段名（带 `__` 前缀，避免与用户字段冲突）。
const VM_FIELD: &str = "__vm";

/// `#[capability(...)]` 的参数。
#[derive(Default)]
struct MetaArgs {
    namespace: String,
    version: Option<String>,
    description: Option<String>,
}

struct FieldSpec {
    ident: Ident,
    vis: Visibility,
    /// 保留的非 `#[vm_type]` 属性（文档等）。
    attrs: Vec<syn::Attribute>,
    /// 规范化后的裸 fn 指针类型（第一个 `&dyn Vm` 形参由宏补上）。
    norm_ty: TokenStream2,
    /// 用户声明的形参（已剥掉第一个 `&dyn Vm`）。
    params: Vec<Type>,
    /// 返回类型（缺省为 `()`）。
    ret: Type,
    /// `#[vm_type(paramN = "...")]` 的显式类型名覆盖。
    param_names: BTreeMap<usize, String>,
    /// `#[vm_type(... = "...")]` / `#[vm_type = "..."]` 的返回类型名覆盖。
    ret_name: Option<String>,
}

/// `#[capability]` / `#[capability(namespace = "...", version = "...", description = "...")]`。
#[proc_macro_attribute]
pub fn capability(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut args = MetaArgs::default();
    if !attr.is_empty() {
        let parser = syn::meta::parser(|meta| {
            if meta.path.is_ident("namespace") {
                let lit: LitStr = meta.value()?.parse()?;
                args.namespace = lit.value();
                Ok(())
            } else if meta.path.is_ident("version") {
                let lit: LitStr = meta.value()?.parse()?;
                args.version = Some(lit.value());
                Ok(())
            } else if meta.path.is_ident("description") {
                let lit: LitStr = meta.value()?.parse()?;
                args.description = Some(lit.value());
                Ok(())
            } else {
                Err(meta.error("未知参数（支持 namespace / version / description = \"...\"）"))
            }
        });
        if let Err(e) = parser.parse(attr) {
            return e.to_compile_error().into();
        }
    }

    let input: ItemStruct = syn::parse_macro_input!(item as ItemStruct);
    match expand(&args, input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(args: &MetaArgs, input: ItemStruct) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let vis = &input.vis;
    let generics = &input.generics;
    let where_clause = &generics.where_clause;

    // capability 句柄必须恰好持有一个 VM 借用生命周期。
    let mut lifetimes = generics.lifetimes();
    let lt: Lifetime = lifetimes
        .next()
        .ok_or_else(|| {
            syn::Error::new(
                name.span(),
                "capability 需要恰好一个生命周期参数（如 `struct MathVm<'a>`）",
            )
        })?
        .lifetime
        .clone();
    if lifetimes.next().is_some() {
        return Err(syn::Error::new(
            name.span(),
            "capability 只能有一个生命周期参数（VM 借用）",
        ));
    }
    if generics.type_params().next().is_some() || generics.const_params().next().is_some() {
        return Err(syn::Error::new(
            name.span(),
            "capability 暂不支持类型参数 / 常量参数",
        ));
    }

    let Fields::Named(named) = &input.fields else {
        return Err(syn::Error::new(
            name.span(),
            "capability 只支持具名 struct（每个字段是一个函数）",
        ));
    };

    let mut specs = Vec::with_capacity(named.named.len());
    for field in &named.named {
        specs.push(parse_field(field)?);
    }
    if specs.is_empty() {
        return Err(syn::Error::new(
            name.span(),
            "capability 至少需要一个函数字段",
        ));
    }

    let (impl_generics, ty_generics, _) = generics.split_for_impl();
    let namespace_lit = LitStr::new(&args.namespace, name.span());
    let input_attrs = &input.attrs;
    let vm_ident = Ident::new(VM_FIELD, name.span());

    let version_expr = match &args.version {
        Some(v) => {
            let lit = LitStr::new(v, name.span());
            quote!(::core::option::Option::Some(#lit))
        }
        None => quote!(::core::option::Option::None),
    };
    let description_expr = match &args.description {
        Some(d) => {
            let lit = LitStr::new(d, name.span());
            quote!(::core::option::Option::Some(#lit))
        }
        None => quote!(::core::option::Option::None),
    };

    // ── struct 本身：__vm + 各函数的裸 fn 指针 ───────────────────────────
    let struct_fields = specs.iter().map(|s| {
        let (ident, vis, attrs, norm_ty) = (&s.ident, &s.vis, &s.attrs, &s.norm_ty);
        quote! {
            #(#attrs)*
            #vis #ident: #norm_ty
        }
    });

    // ── inherent 方法：用户写的签名就是最终签名 ─────────────────────────
    let methods = specs.iter().map(|s| {
        let ident = &s.ident;
        let ret = &s.ret;
        // 方法语义：次参是 `Handle` ⇒ 它就是 receiver（C 风格 obj_method(vm, obj, ..)）。
        let has_receiver = s.params.first().is_some_and(is_handle_type);
        let mut free_index = 0usize;
        let arg_names: Vec<Ident> = s
            .params
            .iter()
            .enumerate()
            .map(|(i, _)| {
                if i == 0 && has_receiver {
                    Ident::new("receiver", s.ident.span())
                } else {
                    let name = format_ident!("arg{}", free_index);
                    free_index += 1;
                    name
                }
            })
            .collect();
        let arg_tys = &s.params;
        quote! {
            pub fn #ident(&self, #(#arg_names: #arg_tys),*) -> #ret {
                (self.#ident)(self.#vm_ident, #(#arg_names),*)
            }
        }
    });

    // ── signatures()：声明逐条对应（const 数组，顺序 = 字段顺序）────────
    let sig_entries = specs.iter().map(|s| {
        let fname = LitStr::new(&s.ident.to_string(), s.ident.span());
        let param_exprs: Vec<TokenStream2> = s
            .params
            .iter()
            .enumerate()
            .map(|(i, ty)| match s.param_names.get(&i) {
                Some(n) => {
                    let lit = LitStr::new(n, ty.span());
                    quote!(#lit)
                }
                None => quote!(<#ty as ::xc_vm::IntoVmTypeName>::NAME),
            })
            .collect();
        let ret_expr = match &s.ret_name {
            Some(n) => {
                let lit = LitStr::new(n, s.ret.span());
                quote!(#lit)
            }
            None => {
                let ret = &s.ret;
                quote!(<#ret as ::xc_vm::IntoVmTypeName>::NAME)
            }
        };
        quote! {
            ::xc_vm::FnSig {
                namespace: #namespace_lit,
                name: #fname,
                params: &[#(#param_exprs),*],
                ret: #ret_expr,
            }
        }
    });

    // ── from_negotiated()：按序取回裸 fn 指针并写入字段 ──────────────────
    let inits = specs.iter().enumerate().map(|(i, s)| {
        let ident = &s.ident;
        let norm_ty = &s.norm_ty;
        quote! {
            #ident: *resolved.get(#i)?.callable.downcast_ref::<#norm_ty>()?
        }
    });

    Ok(quote! {
        #(#input_attrs)*
        #vis struct #name #generics #where_clause {
            #vm_ident: &#lt dyn ::xc_vm::Vm,
            #(#struct_fields),*
        }

        impl #impl_generics #name #ty_generics #where_clause {
            #(#methods)*

            /// 句柄持有的 VM（不可变借用；有状态 VM 的状态由其内部可变性管理）。
            pub fn vm(&self) -> &#lt dyn ::xc_vm::Vm {
                self.#vm_ident
            }
        }

        impl #impl_generics ::xc_vm::Capability<#lt> for #name #ty_generics #where_clause {
            fn signatures() -> &'static [::xc_vm::FnSig] {
                const SIGS: &[::xc_vm::FnSig] = &[#(#sig_entries),*];
                SIGS
            }

            fn meta() -> ::xc_vm::CapabilityMeta {
                ::xc_vm::CapabilityMeta::new(#version_expr, #description_expr)
            }

            fn from_negotiated(
                vm: &#lt dyn ::xc_vm::Vm,
                resolved: &[::xc_vm::Resolved<'_>],
            ) -> ::core::option::Option<Self> {
                ::core::option::Option::Some(Self {
                    #vm_ident: vm,
                    #(#inits),*
                })
            }
        }
    })
}

fn parse_field(field: &Field) -> syn::Result<FieldSpec> {
    let ident = field
        .ident
        .clone()
        .ok_or_else(|| syn::Error::new(field.span(), "capability 只支持具名字段"))?;

    let bare_fn = match &field.ty {
        Type::BareFn(f) => f,
        other => {
            return Err(syn::Error::new(
                other.span(),
                "capability 字段必须是裸函数指针类型，如 `fn(&'a dyn Vm, f32) -> f32`",
            ));
        }
    };

    let mut inputs = bare_fn.inputs.iter();
    let first = inputs.next().ok_or_else(|| {
        syn::Error::new(bare_fn.span(), "函数指针至少需要第一个形参 `&'a dyn Vm`")
    })?;
    if !matches!(&first.ty, Type::Reference(r) if matches!(&*r.elem, Type::TraitObject(_))) {
        return Err(syn::Error::new(
            first.ty.span(),
            "第一个形参必须是 VM 的 trait object 引用（如 `&'a dyn Vm`）",
        ));
    }

    let params: Vec<Type> = inputs.map(|a| a.ty.clone()).collect();
    let ret = match &bare_fn.output {
        ReturnType::Type(_, ty) => (**ty).clone(),
        ReturnType::Default => syn::parse_quote!(()),
    };

    let (param_names, ret_name) = parse_vm_type_attrs(&field.attrs)?;

    for i in param_names.keys() {
        if *i >= params.len() {
            return Err(syn::Error::new(
                ident.span(),
                format!(
                    "`#[vm_type(param{i} = ...)]` 越界：函数只有 {} 个参数",
                    params.len()
                ),
            ));
        }
    }

    let norm_ty = quote!(fn(&dyn ::xc_vm::Vm, #(#params),*) -> #ret);
    let attrs = field
        .attrs
        .iter()
        .filter(|a| !a.path().is_ident("vm_type"))
        .cloned()
        .collect();

    Ok(FieldSpec {
        ident,
        vis: field.vis.clone(),
        attrs,
        norm_ty,
        params,
        ret,
        param_names,
        ret_name,
    })
}

fn parse_vm_type_attrs(
    attrs: &[syn::Attribute],
) -> syn::Result<(BTreeMap<usize, String>, Option<String>)> {
    let mut params = BTreeMap::new();
    let mut ret = None;

    for attr in attrs {
        if !attr.path().is_ident("vm_type") {
            continue;
        }
        match &attr.meta {
            // `#[vm_type = "xcal.Path2D"]` ⇒ 只覆盖返回类型。
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => ret = Some(s.value()),
                other => {
                    return Err(syn::Error::new(
                        other.span(),
                        "`#[vm_type = ...]` 需要字符串字面量",
                    ));
                }
            },
            // `#[vm_type(param0 = "...", ret = "...")]`
            syn::Meta::List(_) => {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("ret") {
                        let s: LitStr = meta.value()?.parse()?;
                        ret = Some(s.value());
                        return Ok(());
                    }
                    let Some(id) = meta.path.get_ident() else {
                        return Err(meta.error("未知参数（支持 paramN = \"...\" / ret = \"...\"）"));
                    };
                    let text = id.to_string();
                    let Some(index) = text.strip_prefix("param") else {
                        return Err(meta.error("未知参数（支持 paramN = \"...\" / ret = \"...\"）"));
                    };
                    let index: usize = index
                        .parse()
                        .map_err(|_| meta.error("参数名应为 param0 / param1 / ..."))?;
                    let s: LitStr = meta.value()?.parse()?;
                    params.insert(index, s.value());
                    Ok(())
                })?;
            }
            syn::Meta::Path(_) => {
                return Err(syn::Error::new(
                    attr.span(),
                    "`#[vm_type]` 需要参数（如 `#[vm_type(ret = \"...\")]`）",
                ));
            }
        }
    }

    Ok((params, ret))
}

/// 次参是否为 `Handle`（方法语义的 receiver）。
///
/// 语法级判断（取类型路径最后一段）：`Handle` / `xc_vm::Handle` 都识别；
/// 类型别名不会被识别（文档已注明）。
fn is_handle_type(ty: &Type) -> bool {
    match ty {
        Type::Path(path) => {
            path.qself.is_none()
                && path
                    .path
                    .segments
                    .last()
                    .is_some_and(|segment| segment.ident == "Handle")
        }
        _ => false,
    }
}
