//! `#[derive(rustique::Cast)]`: `FromValue` and `From<T> for Value` for enums,
//! newtypes and JSON documents.

use std::collections::HashSet;

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Fields, LitStr};

use crate::snake;

/// Storage chosen by the container's `#[cast(...)]` attribute.
#[derive(PartialEq)]
enum Storage {
    /// Unit enum stored as text, newtype stored as its field.
    Default,
    /// `#[cast(int)]`: unit enum stored as its discriminant.
    Int,
    /// `#[cast(json)]`: any serde type stored as a JSON document.
    Json,
}

pub fn expand(input: DeriveInput) -> syn::Result<TokenStream2> {
    let storage = parse_storage(&input.attrs)?;
    let ident = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let mut where_clause = where_clause
        .cloned()
        .unwrap_or_else(|| syn::parse_quote!(where));

    let (decode, encode, oracle_null) = match (&input.data, &storage) {
        (_, Storage::Json) => {
            where_clause.predicates.push(syn::parse_quote!(
                #ident #ty_generics: ::rustique::__private::serde::Serialize
                    + ::rustique::__private::serde::de::DeserializeOwned
            ));
            json(ident)
        }
        (Data::Enum(data), _) => enumeration(ident, data, &storage)?,
        (Data::Struct(data), Storage::Default) => {
            let inner = newtype_field(&data.fields).ok_or_else(|| {
                syn::Error::new_spanned(
                    ident,
                    "#[derive(Cast)] needs a unit-only enum, a one-field tuple struct, \
                     or `#[cast(json)]`",
                )
            })?;
            where_clause.predicates.push(syn::parse_quote!(
                #inner: ::rustique::FromValue + ::core::convert::Into<::rustique::Value>
            ));
            newtype(&inner)
        }
        (_, Storage::Int) => {
            return Err(syn::Error::new_spanned(
                ident,
                "`#[cast(int)]` needs a unit-only enum",
            ));
        }
        (Data::Union(_), _) => {
            return Err(syn::Error::new_spanned(
                ident,
                "#[derive(Cast)] does not support unions",
            ));
        }
    };

    Ok(quote! {
        impl #impl_generics ::rustique::FromValue for #ident #ty_generics #where_clause {
            fn from_value(value: ::rustique::Value) -> ::core::result::Result<Self, ::rustique::ValueError> {
                #decode
            }
            #oracle_null
        }

        impl #impl_generics ::core::convert::From<#ident #ty_generics> for ::rustique::Value #where_clause {
            fn from(value: #ident #ty_generics) -> Self {
                #encode
            }
        }
    })
}

/// `#[cast(int)]` / `#[cast(json)]` on the type.
fn parse_storage(attrs: &[Attribute]) -> syn::Result<Storage> {
    let mut storage = Storage::Default;
    for attr in attrs.iter().filter(|a| a.path().is_ident("cast")) {
        attr.parse_nested_meta(|meta| {
            let next = if meta.path.is_ident("int") {
                Storage::Int
            } else if meta.path.is_ident("json") {
                Storage::Json
            } else {
                return Err(meta.error("unknown cast option; expected `int` or `json`"));
            };
            if storage != Storage::Default {
                return Err(meta.error("only one of `int` and `json` is allowed"));
            }
            storage = next;
            Ok(())
        })?;
    }
    Ok(storage)
}

/// `#[cast(rename = "...")]` on a variant.
fn parse_rename(attrs: &[Attribute]) -> syn::Result<Option<LitStr>> {
    let mut rename = None;
    for attr in attrs.iter().filter(|a| a.path().is_ident("cast")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                rename = Some(meta.value()?.parse()?);
                Ok(())
            } else {
                Err(meta.error("unknown variant option; expected `rename`"))
            }
        })?;
    }
    Ok(rename)
}

fn unknown(found: &str) -> TokenStream2 {
    quote! {
        ::rustique::ValueError { expected: ::core::any::type_name::<Self>(), found: #found }
    }
}

fn enumeration(
    ident: &syn::Ident,
    data: &syn::DataEnum,
    storage: &Storage,
) -> syn::Result<(TokenStream2, TokenStream2, TokenStream2)> {
    let mut variants = Vec::new();
    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for variant in &data.variants {
        if !matches!(variant.fields, Fields::Unit) {
            return Err(syn::Error::new_spanned(
                variant,
                "#[derive(Cast)] enums need unit variants; use `#[cast(json)]` for data",
            ));
        }
        let rename = parse_rename(&variant.attrs)?;
        if *storage == Storage::Int {
            if let Some(rename) = rename {
                return Err(syn::Error::new_spanned(
                    rename,
                    "`rename` has no effect with `#[cast(int)]`",
                ));
            }
        } else {
            let name = rename.unwrap_or_else(|| {
                LitStr::new(&snake(&variant.ident.to_string()), variant.ident.span())
            });
            if !seen.insert(name.value()) {
                return Err(syn::Error::new_spanned(
                    &variant.ident,
                    format!("two variants are stored as {:?}", name.value()),
                ));
            }
            names.push(name);
        }
        variants.push(&variant.ident);
    }

    Ok(if *storage == Storage::Int {
        let unknown = unknown("unknown discriminant");
        (
            quote! {
                let n = <i64 as ::rustique::FromValue>::from_value(value)?;
                #(if n == #ident::#variants as i64 {
                    return ::core::result::Result::Ok(#ident::#variants);
                })*
                ::core::result::Result::Err(#unknown)
            },
            quote! { ::rustique::Value::I64(value as i64) },
            quote! {},
        )
    } else {
        let unknown = unknown("unknown variant");
        (
            quote! {
                match <::std::string::String as ::rustique::FromValue>::from_value(value)?.as_str() {
                    #(#names => ::core::result::Result::Ok(#ident::#variants),)*
                    _ => ::core::result::Result::Err(#unknown),
                }
            },
            quote! {
                ::rustique::Value::from(match value { #(#ident::#variants => #names,)* })
            },
            quote! {},
        )
    })
}

fn newtype_field(fields: &Fields) -> Option<syn::Type> {
    match fields {
        Fields::Unnamed(fields) if fields.unnamed.len() == 1 => Some(fields.unnamed[0].ty.clone()),
        _ => None,
    }
}

fn newtype(inner: &syn::Type) -> (TokenStream2, TokenStream2, TokenStream2) {
    (
        quote! { <#inner as ::rustique::FromValue>::from_value(value).map(Self) },
        quote! { ::core::convert::Into::into(value.0) },
        quote! {
            fn from_oracle_null() -> ::core::option::Option<Self> {
                <#inner as ::rustique::FromValue>::from_oracle_null().map(Self)
            }
        },
    )
}

fn json(ident: &syn::Ident) -> (TokenStream2, TokenStream2, TokenStream2) {
    let message = format!("{ident} cannot be serialized to JSON");
    (
        quote! {
            // Straight from the text, without a `serde_json::Value` tree in between.
            ::rustique::__private::parse_json(value, "JSON of another shape")
        },
        quote! {
            ::rustique::Value::Json(::rustique::__private::serde_json::to_string(&value).expect(#message))
        },
        quote! {},
    )
}
