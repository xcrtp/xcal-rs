use proc_macro::TokenStream;
use syn::parse_macro_input;

pub(crate) mod transform2d {
    use proc_macro2::TokenStream;
    use quote::quote;
    use syn::{Data, Fields, parse_quote};

    /// 字段转换模式
    #[derive(Clone, Copy, PartialEq)]
    enum FieldKind {
        /// 默认：调用 `Transform2DInto::$method(self.field, camera)`
        Default,
        /// `#[transform(skip)]`：直接 move 字段（不调用任何转换）
        Skip,
        /// `#[transform(iter)]`：`self.field.into_iter().map(|v| v.$method(camera)).collect()`
        Iter,
    }

    /// 解析字段上的 `#[transform(...)]` 属性，确定转换模式。
    /// `skip` 与 `iter` 互斥；其它未知参数会返回编译错误。
    fn parse_field_kind(attrs: &[syn::Attribute]) -> syn::Result<FieldKind> {
        let mut skip = false;
        let mut iter = false;
        for attr in attrs {
            if !attr.path().is_ident("transform") {
                continue;
            }
            attr.parse_nested_meta(|m| {
                if m.path.is_ident("skip") {
                    skip = true;
                    Ok(())
                } else if m.path.is_ident("iter") {
                    iter = true;
                    Ok(())
                } else {
                    Err(m.error("unknown argument, expected `skip` or `iter`"))
                }
            })?;
        }
        if skip && iter {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                "`skip` and `iter` are mutually exclusive on the same field",
            ));
        }
        Ok(if skip {
            FieldKind::Skip
        } else if iter {
            FieldKind::Iter
        } else {
            FieldKind::Default
        })
    }

    /// 为结构体自动派生 [`xcal_core::transform::Transform2DInto`]（消耗 `self` 版本）。
    ///
    /// 对每个字段，按 `#[transform(...)]` 属性生成对应的初始化表达式：
    /// - 默认（无属性）：`field.to_logical_into(camera)` / `field.to_screen_into(camera)`
    /// - `#[transform(skip)]`：`self.field`（直接 move，无转换）
    /// - `#[transform(iter)]`：`self.field.into_iter().map(|v| v.to_logical_into(camera)).collect()`
    ///
    /// 对应的 `Transform2D`（非消耗版本）由 blanket impl 自动提供
    /// （要求类型实现 `Clone`）。
    pub fn derive_transform2d_into(input: syn::DeriveInput) -> TokenStream {
        let name = &input.ident;

        // 仅支持 struct（不支持 enum / union）
        let (fields, is_named) = match &input.data {
            Data::Struct(data) => match &data.fields {
                Fields::Named(named) => (named.named.iter().collect::<Vec<_>>(), true),
                Fields::Unnamed(unnamed) => (unnamed.unnamed.iter().collect::<Vec<_>>(), false),
                Fields::Unit => {
                    let (impl_g, ty_g, where_g) = input.generics.split_for_impl();
                    return quote! {
                        impl #impl_g xcal_core::transform::Transform2DInto for #name #ty_g #where_g {
                            fn to_logical_into(self, _: &xcal_core::types::Camera2D) -> Self { Self }
                            fn to_screen_into(self, _: &xcal_core::types::Camera2D) -> Self { Self }
                        }
                    };
                }
            },
            _ => panic!("Transform2DInto can only be derived for structs"),
        };

        // 预解析字段属性；遇到未知参数或 `skip`+`iter` 冲突直接返回编译错误。
        let mut kinds = Vec::with_capacity(fields.len());
        for f in &fields {
            match parse_field_kind(&f.attrs) {
                Ok(k) => kinds.push(k),
                Err(e) => return e.to_compile_error(),
            };
        }

        // 构造 where 子句：
        // - Default 字段：`FieldType: Transform2DInto`
        // - Skip 字段：无约束（直接 move）
        // - Iter 字段：无显式约束（由调用点检查；为泛型结构体时用户需自行补充约束）
        let mut generics = input.generics.clone();
        let where_clause = generics
            .where_clause
            .get_or_insert_with(|| parse_quote!(where));
        for (f, &kind) in fields.iter().zip(&kinds) {
            if let FieldKind::Default = kind {
                let ty = &f.ty;
                where_clause
                    .predicates
                    .push(parse_quote!(#ty: xcal_core::transform::Transform2DInto));
            }
        }
        let (impl_generics, ty_generics, where_clauses) = generics.split_for_impl();

        macro_rules! build_fields {
            ($method:ident) => {
                fields.iter().enumerate().map(|(i, f)| {
                    let idx = syn::Index::from(i);
                    let kind = kinds[i];
                    match &f.ident {
                        Some(ident) => match kind {
                            FieldKind::Default => quote! {
                                #ident: xcal_core::transform::Transform2DInto::$method(self.#ident, camera)
                            },
                            FieldKind::Skip => quote! { #ident: self.#ident },
                            FieldKind::Iter => quote! {
                                #ident: ::core::iter::IntoIterator::into_iter(self.#ident)
                                    .map(|v| xcal_core::transform::Transform2DInto::$method(v, camera))
                                    .collect()
                            },
                        },
                        None => match kind {
                            FieldKind::Default => quote! {
                                xcal_core::transform::Transform2DInto::$method(self.#idx, camera)
                            },
                            FieldKind::Skip => quote! { self.#idx },
                            FieldKind::Iter => quote! {
                                ::core::iter::IntoIterator::into_iter(self.#idx)
                                    .map(|v| xcal_core::transform::Transform2DInto::$method(v, camera))
                                    .collect()
                            },
                        },
                    }
                })
            };
        }

        let logical_fields = build_fields!(to_logical_into);
        let screen_fields = build_fields!(to_screen_into);

        let (constructor_logical, constructor_screen) = if is_named {
            (
                quote! { Self { #(#logical_fields,)* } },
                quote! { Self { #(#screen_fields,)* } },
            )
        } else {
            (
                quote! { Self ( #(#logical_fields,)* ) },
                quote! { Self ( #(#screen_fields,)* ) },
            )
        };

        quote! {
            impl #impl_generics xcal_core::transform::Transform2DInto for #name #ty_generics #where_clauses {
                fn to_logical_into(self, camera: &xcal_core::types::Camera2D) -> Self {
                    #constructor_logical
                }
                fn to_screen_into(self, camera: &xcal_core::types::Camera2D) -> Self {
                    #constructor_screen
                }
            }
        }
    }
}

pub(crate) mod details {
    pub fn derive_scene_object(input: syn::DeriveInput) -> proc_macro2::TokenStream {
        let _ = input;
        proc_macro2::TokenStream::new()
    }
}

#[proc_macro_derive(SceneObject, attributes(with))]
pub fn derive_scene_object(input: TokenStream) -> TokenStream {
    details::derive_scene_object(parse_macro_input!(input as syn::DeriveInput)).into()
}

#[proc_macro_derive(Transform2DInto, attributes(transform))]
pub fn derive_transform2d_into(input: TokenStream) -> TokenStream {
    transform2d::derive_transform2d_into(parse_macro_input!(input as syn::DeriveInput)).into()
}

#[cfg(test)]
#[allow(unused_imports)]
mod tests {
    use proc_macro::TokenStream;
    use std::str::FromStr;
    use syn::parse_quote;

    #[allow(unused_imports)]
    use super::details::*;
    #[allow(unused_imports)]
    use super::transform2d::*;

    #[test]
    fn parse_attr() {
        let input: syn::DeriveInput = parse_quote! {
            #[derive(
                Clone, derive_new::new, derive_builder::Builder, xcal_macro::SceneObject,
            )]
            #[with(Positionable,Positionable)]
            struct Circle {
                pos: Point3,
                r: f32,
            }
        };

        let res = derive_scene_object(input);
        println!("{res}");
    }

    #[test]
    fn parse_transform2d_into_named() {
        let input: syn::DeriveInput = parse_quote! {
            struct Rectangle {
                #[transform(skip)]
                color: u32,
                #[transform(iter)]
                points: Vec<u64>,
                pos: u64,
            }
        };

        let res = derive_transform2d_into(input);
        println!("named struct:\n{res}");
    }

    #[test]
    fn parse_transform2d_into_tuple() {
        let input: syn::DeriveInput = parse_quote! {
            struct Pair(u64, u64);
        };

        let res = derive_transform2d_into(input);
        println!("tuple struct:\n{res}");
    }

    #[test]
    fn parse_transform2d_into_unit() {
        let input: syn::DeriveInput = parse_quote! {
            struct Marker;
        };

        let res = derive_transform2d_into(input);
        println!("unit struct:\n{res}");
    }

    #[test]
    fn parse_transform2d_into_generic() {
        let input: syn::DeriveInput = parse_quote! {
            struct View<T> {
                #[transform(skip)]
                label: String,
                inner: T,
            }
        };

        let res = derive_transform2d_into(input);
        println!("generic struct:\n{res}");
    }
}
