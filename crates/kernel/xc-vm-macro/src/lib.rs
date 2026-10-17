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
use syn::{
    Data, DeriveInput, Field, Fields, Ident, ItemStruct, Lifetime, LitStr, ReturnType, Type,
    Visibility,
};

/// capability 的 VM 借用字段名（带 `__` 前缀，避免与用户字段冲突）。
const VM_FIELD: &str = "__vm";

/// `#[capability(...)]` 的参数。
#[derive(Default)]
struct MetaArgs {
    namespace: String,
    version: Option<String>,
    description: Option<String>,
    /// 生成 `calls()`（类型擦除绑定入口）。默认关闭：
    /// 并非每个 capability 的字段类型都实现 `VmType`（既有测试就有 `Big`）。
    bindings: bool,
}

struct FieldSpec {
    ident: Ident,
    vis: Visibility,
    /// 字段上的 `///` 文档（拼成一段自由文本；缺失时为空串）。
    description: String,
    /// 保留的非 `#[vm_type]` 属性（文档等）。
    attrs: Vec<syn::Attribute>,
    /// 规范化后的裸 fn 指针类型（第一个 `&dyn Vm` 形参由宏补上）。
    norm_ty: TokenStream2,
    /// 用户声明的形参（已剥掉第一个 `&dyn Vm`）。
    params: Vec<Type>,
    /// 形参名（裸 fn 类型里可以写 `x: f32`）；缺失时绑定层回退到 `pN`。
    param_idents: Vec<Option<String>>,
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
            } else if meta.path.is_ident("bindings") {
                args.bindings = true;
                Ok(())
            } else {
                Err(meta.error(
                    "未知参数（支持 namespace / version / description = \"...\" / bindings）",
                ))
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

    // ── signatures() / docs()：声明逐条对应（两个并行数组，顺序 = 字段顺序）──
    let sig_entries = specs.iter().map(|s| {
        let fname = LitStr::new(&s.ident.to_string(), s.ident.span());
        let param_exprs = param_name_exprs(s);
        let ret_expr = ret_name_expr(s);
        quote! {
            ::xc_vm::FnSig {
                namespace: #namespace_lit,
                name: #fname,
                params: &[#(#param_exprs),*],
                ret: #ret_expr,
            }
        }
    });

    // 文档条目：`description` 来自字段上的 `///`（已编译成 `#[doc]` 属性）。
    let doc_entries = specs.iter().map(|s| {
        let fname = LitStr::new(&s.ident.to_string(), s.ident.span());
        let description = LitStr::new(&s.description, s.ident.span());
        let param_exprs = param_name_exprs(s);
        let ret_expr = ret_name_expr(s);
        quote! {
            ::xc_vm::FnDoc {
                namespace: #namespace_lit,
                name: #fname,
                description: #description,
                params: &[#(#param_exprs),*],
                ret: #ret_expr,
            }
        }
    });

    // 缺文档告警：有参数但没写 `///` ⇒ 编译期 warning（`//` 在编译期已丢失）。
    // stable 上 proc-macro 不能直接发 warning，这里用"使用一个 deprecated 常量"实现。
    let missing_doc_warnings = specs.iter().filter_map(|s| {
        if s.params.is_empty() || !s.description.is_empty() {
            return None;
        }
        let ident = format_ident!("__XC_VM_MISSING_DOC_{}_{}", name, s.ident);
        let msg = format!(
            "capability 函数 `{}` 缺少 `///` 文档（只有 `///` 会在编译期保留）",
            s.ident
        );
        Some(quote! {
            #[deprecated(note = #msg)]
            #[allow(non_upper_case_globals)]
            const #ident: () = ();
            const _: () = #ident;
        })
    });

    // 并行数组的**编译期长度断言**：`docs()` 与 `signatures()` 必须逐条对应。
    let sigs_const = format_ident!("__XC_VM_SIGS_{}", name);
    let docs_const = format_ident!("__XC_VM_DOCS_{}", name);

    // ── from_negotiated()：按序取回裸 fn 指针并写入字段 ──────────────────
    let inits = specs.iter().enumerate().map(|(i, s)| {
        let ident = &s.ident;
        let norm_ty = &s.norm_ty;
        quote! {
            #ident: *resolved.get(#i)?.callable.downcast_ref::<#norm_ty>()?
        }
    });

    // ── 绑定生成（`bindings`）：FnIo + 类型擦除调用 + `calls()` ─────────
    let io_const = format_ident!("__XC_VM_IO_{}", name);
    let calls_const = format_ident!("__XC_VM_CALLS_{}", name);
    let (binding_defs, binding_impl) = if args.bindings {
        let io_entries = specs.iter().map(|s| {
            let params = s.params.iter().enumerate().map(|(i, ty)| {
                let pname = match s.param_idents.get(i).and_then(|n| n.as_deref()) {
                    Some(n) => LitStr::new(n, ty.span()),
                    None => LitStr::new(&format!("p{}", i + 1), ty.span()),
                };
                quote! {
                    ::xc_vm::ParamSpec {
                        name: #pname,
                        ty: <#ty as ::xc_vm::VmType>::SPEC,
                    }
                }
            });
            let ret = &s.ret;
            quote! {
                ::xc_vm::FnIo {
                    params: &[#(#params),*],
                    ret: <#ret as ::xc_vm::VmType>::SPEC,
                }
            }
        });

        let call_fns = specs.iter().enumerate().map(|(idx, s)| {
            let fn_ident = format_ident!("__xc_vm_call_{}_{}", name, s.ident);
            let norm_ty = &s.norm_ty;
            let ret = &s.ret;
            let n = s.params.len();
            let arg_inits = s.params.iter().enumerate().map(|(i, ty)| {
                let arg_ident = format_ident!("__arg{}", i);
                quote! {
                    let #arg_ident = <#ty as ::xc_vm::VmType>::from_vm_value(
                        args.get(#i).ok_or_else(|| ::xc_vm::VmError::CallFailed(
                            ::std::format!("参数个数不足：需要 {} 个，实际 {}", #n, args.len())
                        ))?
                    ).map_err(|e| ::xc_vm::VmError::CallFailed(e.to_string()))?;
                }
            });
            let arg_idents: Vec<Ident> =
                (0..n).map(|i| format_ident!("__arg{}", i)).collect();
            let call = if arg_idents.is_empty() {
                quote!(__f(vm))
            } else {
                quote!(__f(vm, #(#arg_idents),*))
            };
            quote! {
                #[allow(non_snake_case)]
                fn #fn_ident(
                    vm: &dyn ::xc_vm::Vm,
                    args: &[::xc_vm::VmValue],
                ) -> ::core::result::Result<::xc_vm::VmValue, ::xc_vm::VmError> {
                    let __name = #sigs_const[#idx].qualified();
                    let __callable = vm.resolve(&__name).ok_or_else(|| {
                        ::xc_vm::VmError::UnknownFunction(__name.to_string())
                    })?;
                    let __f = *__callable.downcast_ref::<#norm_ty>().ok_or_else(|| {
                        ::xc_vm::VmError::CallFailed(::std::format!(
                            "`{}` 的 typed 签名不匹配", __name
                        ))
                    })?;
                    #(#arg_inits)*
                    ::core::result::Result::Ok(
                        <#ret as ::xc_vm::VmType>::to_vm_value(&#call)
                    )
                }
            }
        });
        let call_idents: Vec<Ident> = specs
            .iter()
            .map(|s| format_ident!("__xc_vm_call_{}_{}", name, s.ident))
            .collect();
        let calls_entries = call_idents.iter().enumerate().map(|(idx, f)| {
            quote! {
                ::xc_vm::ErasedFn {
                    signature: #sigs_const[#idx],
                    io: #io_const[#idx],
                    call: #f,
                }
            }
        });

        (
            quote! {
                #(#call_fns)*

                #[doc(hidden)]
                #[allow(non_upper_case_globals)]
                const #io_const: &[::xc_vm::FnIo] = &[#(#io_entries),*];
                #[doc(hidden)]
                #[allow(non_upper_case_globals)]
                const #calls_const: &[::xc_vm::ErasedFn] = &[#(#calls_entries),*];

                const _: () = ::core::assert!(#io_const.len() == #sigs_const.len());
                const _: () = ::core::assert!(#calls_const.len() == #sigs_const.len());
            },
            quote! {
                fn calls() -> &'static [::xc_vm::ErasedFn] {
                    #calls_const
                }
            },
        )
    } else {
        (quote! {}, quote! {})
    };

    Ok(quote! {
        #(#input_attrs)*
        #vis struct #name #generics #where_clause {
            #vm_ident: &#lt dyn ::xc_vm::Vm,
            #(#struct_fields),*
        }

        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        const #sigs_const: &[::xc_vm::FnSig] = &[#(#sig_entries),*];
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        const #docs_const: &[::xc_vm::FnDoc] = &[#(#doc_entries),*];

        // 编译期断言：`docs()` 与 `signatures()` 是两个并行数组，长度必须一致
        // （错位在编译期炸，不等到运行时）。
        const _: () = ::core::assert!(
            #sigs_const.len() == #docs_const.len(),
            "`docs()` 与 `signatures()` 必须逐条对应（长度不等）"
        );

        #(#missing_doc_warnings)*

        #binding_defs

        impl #impl_generics #name #ty_generics #where_clause {
            #(#methods)*

            /// 句柄持有的 VM（不可变借用；有状态 VM 的状态由其内部可变性管理）。
            pub fn vm(&self) -> &#lt dyn ::xc_vm::Vm {
                self.#vm_ident
            }
        }

        impl #impl_generics ::xc_vm::Capability<#lt> for #name #ty_generics #where_clause {
            fn signatures() -> &'static [::xc_vm::FnSig] {
                #sigs_const
            }

            fn meta() -> ::xc_vm::CapabilityMeta {
                ::xc_vm::CapabilityMeta::new(#version_expr, #description_expr)
            }

            fn docs() -> &'static [::xc_vm::FnDoc] {
                #docs_const
            }

            #binding_impl

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

/// 参数类型名的生成表达式（显式标注优先，否则走 `IntoVmTypeName::NAME`）。
fn param_name_exprs(s: &FieldSpec) -> Vec<TokenStream2> {
    s.params
        .iter()
        .enumerate()
        .map(|(i, ty)| match s.param_names.get(&i) {
            Some(n) => {
                let lit = LitStr::new(n, ty.span());
                quote!(#lit)
            }
            None => quote!(<#ty as ::xc_vm::IntoVmTypeName>::NAME),
        })
        .collect()
}

/// 返回类型名的生成表达式（显式标注优先，否则走 `IntoVmTypeName::NAME`）。
fn ret_name_expr(s: &FieldSpec) -> TokenStream2 {
    match &s.ret_name {
        Some(n) => {
            let lit = LitStr::new(n, s.ret.span());
            quote!(#lit)
        }
        None => {
            let ret = &s.ret;
            quote!(<#ret as ::xc_vm::IntoVmTypeName>::NAME)
        }
    }
}

/// 从字段属性里取 `///` 文档（多个 `#[doc]` 行拼成一段，逐行 trim）。
///
/// `//` 普通注释在编译期已丢失，只有 `///` / `//!` 会变成 `#[doc = "..."]`。
fn doc_text(attrs: &[syn::Attribute]) -> String {
    let mut lines = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) = &nv.value
            {
                lines.push(s.value().trim().to_string());
            }
        }
    }
    lines.join("\n").trim().to_string()
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

    // 形参名（`fn(&'a dyn Vm, x: f32)` 里的 `x`）进 schema，文档才不是 p1..pN。
    let mut params = Vec::new();
    let mut param_idents = Vec::new();
    for arg in inputs {
        params.push(arg.ty.clone());
        param_idents.push(arg.name.as_ref().map(|(ident, _)| ident.to_string()));
    }
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
        description: doc_text(&field.attrs),
        attrs,
        norm_ty,
        params,
        param_idents,
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

// ─── #[derive(VmType)]：类型定义 → TypeSpec（一处定义，不手写第二份）──────

/// 从 struct / enum 定义生成 `VmType`（`SPEC` + 双向转换）与 `IntoVmTypeName`。
///
/// 支持：具名结构体、枚举（unit 变体 / 具名字段变体）。
/// 不支持泛型 / 元组风格字段（场景无消费方，计划 §8）。
#[proc_macro_derive(VmType, attributes(vm_type))]
pub fn derive_vm_type(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match expand_vm_type(&input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// 具名字段：`(标识符, 类型, 字段名字面量)`。
type NamedField = (Ident, Type, LitStr);

fn expand_vm_type(input: &DeriveInput) -> syn::Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new(
            input.ident.span(),
            "VmType 不支持泛型参数（场景无消费方）",
        ));
    }
    let name = &input.ident;
    let type_name = vm_type_name(&input.attrs, name)?;
    let type_name_lit = LitStr::new(&type_name, name.span());

    let (spec_body, to_body, from_body) = match &input.data {
        Data::Struct(data) => {
            let Fields::Named(named) = &data.fields else {
                return Err(syn::Error::new(
                    ident_span_or(name.span(), &data.fields),
                    "VmType 只支持具名结构体（字段名要进 schema）",
                ));
            };
            let fields = named_fields(&named.named)?;
            let field_defs = fields.iter().map(|(_, ty, fname)| {
                quote! { ::xc_vm::Field { name: #fname, ty: <#ty as ::xc_vm::VmType>::SPEC } }
            });
            let to_fields = fields.iter().map(|(ident, _, fname)| {
                quote! { (#fname, ::xc_vm::VmType::to_vm_value(&self.#ident)) }
            });
            let from_fields = fields.iter().map(|(ident, ty, fname)| {
                quote! {
                    #ident: <#ty as ::xc_vm::VmType>::from_vm_value(
                        ::xc_vm::field(__fields, #fname)?
                    )?
                }
            });
            (
                quote! {
                    &::xc_vm::TypeSpec::Struct {
                        name: #type_name_lit,
                        fields: &[#(#field_defs),*],
                    }
                },
                quote! {
                    ::xc_vm::VmValue::Struct(
                        #type_name_lit,
                        ::std::vec![#(#to_fields),*],
                    )
                },
                quote! {
                    let __fields = ::xc_vm::expect_struct(__value, #type_name_lit)?;
                    ::core::result::Result::Ok(Self {
                        #(#from_fields),*
                    })
                },
            )
        }
        Data::Enum(data) => {
            let variants = named_variants(data)?;
            let variant_defs = variants.iter().map(|(_, vname, fields)| {
                let field_defs = fields.iter().map(|(_, ty, fname)| {
                    quote! { ::xc_vm::Field { name: #fname, ty: <#ty as ::xc_vm::VmType>::SPEC } }
                });
                quote! {
                    ::xc_vm::Variant {
                        name: #vname,
                        fields: &[#(#field_defs),*],
                    }
                }
            });
            let to_arms = variants.iter().map(|(vid, vname, fields)| {
                if fields.is_empty() {
                    quote! {
                        Self::#vid => ::xc_vm::VmValue::Enum(
                            #type_name_lit,
                            #vname,
                            ::std::vec![],
                        )
                    }
                } else {
                    let idents: Vec<&Ident> = fields.iter().map(|(i, _, _)| i).collect();
                    let pairs = fields.iter().map(|(ident, _, fname)| {
                        quote! { (#fname, ::xc_vm::VmType::to_vm_value(#ident)) }
                    });
                    quote! {
                        Self::#vid { #(#idents),* } => ::xc_vm::VmValue::Enum(
                            #type_name_lit,
                            #vname,
                            ::std::vec![#(#pairs),*],
                        )
                    }
                }
            });
            let from_arms = variants.iter().map(|(vid, vname, fields)| {
                if fields.is_empty() {
                    quote! { #vname => ::core::result::Result::Ok(Self::#vid) }
                } else {
                    let inits = fields.iter().map(|(ident, ty, fname)| {
                        quote! {
                            #ident: <#ty as ::xc_vm::VmType>::from_vm_value(
                                ::xc_vm::field(__fields, #fname)?
                            )?
                        }
                    });
                    quote! {
                        #vname => ::core::result::Result::Ok(Self::#vid {
                            #(#inits),*
                        })
                    }
                }
            });
            (
                quote! {
                    &::xc_vm::TypeSpec::Enum {
                        name: #type_name_lit,
                        variants: &[#(#variant_defs),*],
                    }
                },
                quote! {
                    match self {
                        #(#to_arms),*
                    }
                },
                quote! {
                    let (__variant, __fields) = ::xc_vm::expect_enum(__value, #type_name_lit)?;
                    match __variant {
                        #(#from_arms,)*
                        __other => ::core::result::Result::Err(
                            ::xc_vm::VmValueError::new(::std::format!(
                                "未知变体 `{__other}`（{}）", #type_name_lit
                            ))
                        ),
                    }
                },
            )
        }
        Data::Union(_) => {
            return Err(syn::Error::new(
                name.span(),
                "VmType 不支持 union",
            ));
        }
    };

    Ok(quote! {
        impl ::xc_vm::VmType for #name {
            const SPEC: &'static ::xc_vm::TypeSpec = #spec_body;

            fn to_vm_value(&self) -> ::xc_vm::VmValue {
                #to_body
            }

            fn from_vm_value(
                __value: &::xc_vm::VmValue,
            ) -> ::core::result::Result<Self, ::xc_vm::VmValueError> {
                #from_body
            }
        }

        impl ::xc_vm::IntoVmTypeName for #name {
            const NAME: ::xc_vm::VmTypeName = #type_name_lit;
        }
    })
}

fn ident_span_or(fallback: proc_macro2::Span, fields: &Fields) -> proc_macro2::Span {
    fields.span().resolved_at(fallback)
}

fn vm_type_name(attrs: &[syn::Attribute], ident: &Ident) -> syn::Result<String> {
    for attr in attrs {
        if !attr.path().is_ident("vm_type") {
            continue;
        }
        let mut found = None;
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                let lit: LitStr = meta.value()?.parse()?;
                found = Some(lit.value());
                Ok(())
            } else {
                Err(meta.error("VmType 的 `#[vm_type(...)]` 只支持 name = \"...\""))
            }
        })?;
        if let Some(name) = found {
            return Ok(name);
        }
    }
    Ok(ident.to_string())
}

fn named_fields(fields: &syn::punctuated::Punctuated<Field, syn::Token![,]>) -> syn::Result<Vec<NamedField>> {
    fields
        .iter()
        .map(|field| {
            let ident = field
                .ident
                .clone()
                .ok_or_else(|| syn::Error::new(field.span(), "VmType 只支持具名字段"))?;
            let fname = LitStr::new(&ident.to_string(), ident.span());
            Ok((ident, field.ty.clone(), fname))
        })
        .collect()
}

fn named_variants(
    data: &syn::DataEnum,
) -> syn::Result<Vec<(Ident, LitStr, Vec<NamedField>)>> {
    data.variants
        .iter()
        .map(|variant| {
            let vname = LitStr::new(&variant.ident.to_string(), variant.ident.span());
            match &variant.fields {
                Fields::Named(named) => Ok((
                    variant.ident.clone(),
                    vname,
                    named_fields(&named.named)?,
                )),
                Fields::Unit => Ok((variant.ident.clone(), vname, Vec::new())),
                Fields::Unnamed(_) => Err(syn::Error::new(
                    variant.span(),
                    "VmType 的枚举变体只支持 unit 或具名字段（元组变体的字段没有名字）",
                )),
            }
        })
        .collect()
}
