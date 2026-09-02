use proc_macro::TokenStream;
use syn::parse_macro_input;

pub(crate) mod details {

    #[derive(Debug)]
    struct AstList<T, U>(pub Vec<T>, std::marker::PhantomData<U>);
    impl<T: Parse, U: Parse> syn::parse::Parse for AstList<T, U> {
        fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
            let mut attrs = Vec::new();
            loop {
                attrs.push(input.parse()?);
                if input.is_empty() {
                    break;
                }
                input.parse::<U>()?;
            }
            Ok(Self(attrs, std::marker::PhantomData))
        }
    }
    impl<T: Parse, U: Parse> Deref for AstList<T, U> {
        type Target = Vec<T>;
        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    use quote::{ToTokens, quote};
    use std::ops::Deref;
    use syn::{Token, parse::Parse};
    pub fn derive_scene_object(input: syn::DeriveInput) -> proc_macro2::TokenStream {
        println!("attr count {}", input.attrs.iter().count());
        let ident = input.ident.clone();
        let attr = input
            .attrs
            .into_iter()
            .filter(|a| a.path().is_ident("with"))
            .filter_map(|attr| {
                if let syn::Meta::List(meta_list) = attr.meta {
                    Some(meta_list)
                } else {
                    None
                }
            })
            .flat_map(|i| {
                let attrs: AstList<syn::Ident, Token![,]> =
                    syn::parse2(i.tokens.to_token_stream().into()).unwrap();
                attrs
                    .iter()
                    .map(|a| match a.to_string().as_str() {
                        "Sizeable" => quote! {
                                    fn bound_box(&self) -> Result<&dyn #a> {
                                        Ok(self)
                                    }
                        },
                        "Positionable" => quote! {
                                 fn position(&self) -> Result<&dyn #a> {
                                     Ok(self)
                                 }
                        },
                        "Strokeable" => quote! {
                                    fn stroke(&self) -> Result<&dyn #a> {
                                        Ok(self)
                                    }
                        },
                        "Fillable" => quote! {
                                    fn fill(&self) -> Result<&dyn #a> {
                                        Ok(self)
                                    }
                        },
                        "Moveable" => quote! {
                            fn shift(&mut self) -> Result<&dyn #a> {
                                Ok(self)
                            }
                        },
                        "Transformable" => quote! {
                            fn transform(&self) -> Result<&dyn #a> {
                                Ok(self)
                            }
                            fn transform_mut(&mut self) -> Result<&mut dyn #a> {
                                Ok(self)
                            }
                        },
                        _ => quote! {},
                    })
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        quote! {

        }
    }
}

#[proc_macro_derive(SceneObject, attributes(with))]
pub fn derive_scene_object(input: TokenStream) -> TokenStream {
    details::derive_scene_object(parse_macro_input!(input as syn::DeriveInput)).into()
}

#[cfg(test)]
mod tests {
    use proc_macro::TokenStream;
    use std::str::FromStr;
    use syn::parse_quote;

    #[allow(unused_imports)]
    use super::details::*;

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
        println!("{}", res.to_string());
    }
}
