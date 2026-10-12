//! `#[derive(rustique::MorphTo)]`: an enum of models as a `morph_to` target.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Fields};

pub fn expand(input: DeriveInput) -> syn::Result<TokenStream2> {
    let ident = &input.ident;
    let message = "#[derive(MorphTo)] needs an enum of one-model tuple variants: `Post(Post)`";
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(ident, message));
    };
    if !input.generics.params.is_empty() || data.variants.is_empty() {
        return Err(syn::Error::new_spanned(ident, message));
    }
    let mut variants = Vec::new();
    let mut models = Vec::new();
    for variant in &data.variants {
        let model = match &variant.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => &fields.unnamed[0].ty,
            _ => return Err(syn::Error::new_spanned(variant, message)),
        };
        variants.push(&variant.ident);
        models.push(model);
    }
    Ok(quote! {
        impl ::rustique::MorphTo for #ident {
            async fn load_morph<E: ::rustique::Executor>(
                db: &E,
                alias: &str,
                keys: ::std::vec::Vec<::rustique::Value>,
                nested: ::rustique::__private::Nested,
            ) -> ::rustique::Result<::std::vec::Vec<(::rustique::Value, Self)>> {
                #(if alias == <#models as ::rustique::Model>::MORPH_ALIAS {
                    return ::rustique::__private::morph_models::<#models, Self, E>(
                        db, keys, nested, Self::#variants,
                    ).await;
                })*
                ::core::result::Result::Err(::rustique::Error::UnknownMorphType(alias.to_owned()))
            }

            fn targets() -> ::std::vec::Vec<::rustique::__private::MorphTarget> {
                ::std::vec![#(::rustique::__private::MorphTarget {
                    alias: <#models as ::rustique::Model>::MORPH_ALIAS,
                    table: <#models as ::rustique::Model>::TABLE,
                    key: <#models as ::rustique::Model>::PRIMARY_KEY,
                    query: || ::rustique::Query::into_builder(<#models as ::rustique::Model>::query()),
                }),*]
            }
        }
    })
}
