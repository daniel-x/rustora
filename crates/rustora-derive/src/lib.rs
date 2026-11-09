// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! Derive macros of rustora.
//!
//! `#[derive(FieldLoad)]` ports OpenRA's reflection based `FieldLoader.Load`. Field attributes:
//! - `#[field(name = "HP")]`: YAML key (default: the field name converted to PascalCase)
//! - `#[field(require)]`: error if the key is missing
//! - `#[field(ignore)]`: never loaded
//! - `#[field(flatten)]`: a nested `FieldLoad` struct (C# base class fields)
//! - `#[field(loader = "path")]`: custom `fn(&MiniYaml) -> Result<T, LoadError>` called with the whole node
//! - `#[field(from_key)]`: parse the value from the node value itself
//! - `#[field(allow_empty)]`: keep empty entries when splitting arrays

mod persist;

use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, LitStr, parse_macro_input};

#[derive(Default)]
struct FieldAttrs {
    name: Option<String>,
    require: bool,
    ignore: bool,
    flatten: bool,
    loader: Option<syn::Path>,
    from_key: bool,
    allow_empty: bool,
}

fn pascal_case(s: &str) -> String {
    s.split('_')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

fn parse_attrs(field: &syn::Field) -> syn::Result<FieldAttrs> {
    let mut a = FieldAttrs::default();
    for attr in &field.attrs {
        if !attr.path().is_ident("field") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                a.name = Some(meta.value()?.parse::<LitStr>()?.value());
            } else if meta.path.is_ident("require") {
                a.require = true;
            } else if meta.path.is_ident("ignore") {
                a.ignore = true;
            } else if meta.path.is_ident("flatten") {
                a.flatten = true;
            } else if meta.path.is_ident("from_key") {
                a.from_key = true;
            } else if meta.path.is_ident("allow_empty") {
                a.allow_empty = true;
            } else if meta.path.is_ident("loader") {
                let s = meta.value()?.parse::<LitStr>()?;
                a.loader = Some(s.parse()?);
            } else {
                return Err(meta.error("unknown field attribute"));
            }
            Ok(())
        })?;
    }
    Ok(a)
}

#[proc_macro_derive(FieldLoad, attributes(field))]
pub fn derive_field_load(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(input: &DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(input, "FieldLoad can only be derived for structs"));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(input, "FieldLoad needs named fields"));
    };

    let mut loads = Vec::new();
    let mut names = Vec::new();
    for f in &fields.named {
        let a = parse_attrs(f)?;
        if a.ignore {
            continue;
        }
        let ident = f.ident.as_ref().unwrap();
        let ty = &f.ty;
        if a.flatten {
            loads.push(quote! { ::rustora_core::fieldloader::FieldLoad::load_fields(&mut self.#ident, my, md, missing)?; });
            names.push(quote! { out.extend(<#ty as ::rustora_core::fieldloader::FieldLoad>::field_names()); });
            continue;
        }
        let yaml_name = a.name.clone().unwrap_or_else(|| pascal_case(&ident.to_string()));
        names.push(quote! { out.push(#yaml_name); });
        let require = a.require;
        let allow_empty = a.allow_empty;
        let load = if let Some(loader) = &a.loader {
            quote! {
                if !#require || md.contains_key(#yaml_name) {
                    self.#ident = #loader(my)?;
                } else {
                    missing.push(#yaml_name.to_string());
                }
            }
        } else if a.from_key {
            quote! {
                if !#require || md.contains_key(#yaml_name) {
                    self.#ident = ::rustora_core::fieldloader::get_value_yaml::<#ty>(#yaml_name, my, #allow_empty)?;
                } else {
                    missing.push(#yaml_name.to_string());
                }
            }
        } else {
            quote! {
                match md.get(#yaml_name) {
                    Some(y) => self.#ident = ::rustora_core::fieldloader::get_value_yaml::<#ty>(#yaml_name, y, #allow_empty)?,
                    None => if #require { missing.push(#yaml_name.to_string()); },
                }
            }
        };
        loads.push(load);
    }

    Ok(quote! {
        impl #impl_generics ::rustora_core::fieldloader::FieldLoad for #name #ty_generics #where_clause {
            fn load_fields(
                &mut self,
                my: &::rustora_core::miniyaml::MiniYaml,
                md: &::rustora_core::fieldloader::NodeMap<'_>,
                missing: &mut Vec<String>,
            ) -> Result<(), ::rustora_core::fieldloader::LoadError> {
                #(#loads)*
                Ok(())
            }

            fn field_names() -> Vec<&'static str> {
                let mut out = Vec::new();
                #(#names)*
                out
            }
        }
    })
}

/// `#[derive(Persist)]` implements the save game traits of `rustora_engine::persist`.
///
/// Without a container attribute it implements `PersistState`: the `Cell` and `RefCell` fields are saved and loaded
/// in place, other fields only when their type implements `PersistState`. Container attributes add by value
/// serialization:
/// - `#[persist(value)]`: `Persist` by value.
/// - `#[persist(shared)]`: like `value`, and `Rc`s of the type keep their identity (`RcPersist`).
/// - `#[persist(activity)]`, `#[persist(effect)]`: like `shared`, loaded through the `dyn Activity` or `dyn Effect`
///   registry.
///
/// `#[persist(owned)]` additionally marks `Rc`s of the type as owned by their holder, which writes them in place.
///
/// Field attributes: `#[persist(skip)]` is not saved and loads as `Default`; `#[persist(me)]` is the `Weak` self
/// reference of a shared type; `#[persist(in_place)]` on a `RefCell` writes its content in place
/// instead of replacing it.
#[proc_macro_derive(Persist, attributes(persist))]
pub fn derive_persist(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match persist::expand(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}
