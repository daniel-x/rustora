// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors.

//! `#[derive(Persist)]`: the save game serialization of `rustora_engine::persist`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Type};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Only `PersistState`: the interior mutable fields, written and read in place.
    State,
    /// `Persist` by value, plus `PersistState`.
    Value,
    /// A value shared through `Rc`, with identity kept across the save: `#[persist(shared)]`.
    Shared,
    /// Like `Shared`, but loaded through the registry of `dyn Activity` or `dyn Effect`.
    Dyn(DynKind),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DynKind {
    Activity,
    Effect,
}

#[derive(Default)]
struct FieldAttrs {
    skip: bool,
    me: bool,
    in_place: bool,
}

fn container_mode(input: &DeriveInput) -> syn::Result<(Mode, bool)> {
    let mut mode = Mode::State;
    let mut owned = false;
    for attr in &input.attrs {
        if !attr.path().is_ident("persist") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("owned") {
                owned = true;
                return Ok(());
            }
            mode = if meta.path.is_ident("value") {
                Mode::Value
            } else if meta.path.is_ident("shared") {
                Mode::Shared
            } else if meta.path.is_ident("activity") {
                Mode::Dyn(DynKind::Activity)
            } else if meta.path.is_ident("effect") {
                Mode::Dyn(DynKind::Effect)
            } else {
                return Err(meta.error("unknown persist container attribute"));
            };
            Ok(())
        })?;
    }
    Ok((mode, owned))
}

fn field_attrs(field: &syn::Field) -> syn::Result<FieldAttrs> {
    let mut a = FieldAttrs::default();
    for attr in &field.attrs {
        if !attr.path().is_ident("persist") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                a.skip = true;
            } else if meta.path.is_ident("me") {
                a.me = true;
            } else if meta.path.is_ident("in_place") {
                a.in_place = true;
            } else {
                return Err(meta.error("unknown persist field attribute"));
            }
            Ok(())
        })?;
    }
    Ok(a)
}

/// True for `Rc<Cell<..>>` and `Rc<RefCell<..>>`, shared cells that are saved before the other fields.
fn is_shared_cell(ty: &Type) -> bool {
    let Type::Path(p) = ty else { return false };
    let Some(seg) = p.path.segments.last() else { return false };
    if seg.ident != "Rc" {
        return false;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return false;
    };
    args.args
        .iter()
        .any(|a| matches!(a, syn::GenericArgument::Type(t) if is_interior_mutable(t)))
}

/// True for `Cell<..>` and `RefCell<..>`, the fields that must be saved in place.
fn is_interior_mutable(ty: &Type) -> bool {
    match ty {
        Type::Path(p) => p.path.segments.last().is_some_and(|s| s.ident == "Cell" || s.ident == "RefCell"),
        _ => false,
    }
}

pub fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let (mode, owned) = container_mode(input)?;
    let name = &input.ident;
    let p = quote! { ::rustora_engine::persist };
    let mut generics = input.generics.clone();
    if mode != Mode::State {
        for tp in generics.type_params_mut() {
            tp.bounds.push(syn::parse_quote!(#p::Persist));
            tp.bounds.push(syn::parse_quote!('static));
        }
    }
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let state = state_impl(input)?;
    let mut state = quote! {
        impl #impl_generics #p::PersistState for #name #ty_generics #where_clause {
            #state
        }
    };
    if owned {
        state.extend(quote! {
            impl #impl_generics #p::OwnedState for #name #ty_generics #where_clause {}
        });
    }
    if mode == Mode::State {
        return Ok(state);
    }

    let (save, load) = value_bodies(input, mode != Mode::Value)?;
    let mut out = quote! {
        #state
        impl #impl_generics #p::Persist for #name #ty_generics #where_clause {
            fn save(&self, w: &mut #p::SaveWriter) {
                #save
            }
            fn load(r: &mut #p::SaveReader) -> Self {
                let me = ::std::rc::Weak::<Self>::new();
                let me = &me;
                let _ = me;
                #load
            }
        }
    };
    if mode == Mode::Value {
        return Ok(out);
    }
    out.extend(quote! {
        impl #impl_generics #p::PersistShared for #name #ty_generics #where_clause {
            fn load_shared(r: &mut #p::SaveReader, me: &::std::rc::Weak<Self>) -> Self {
                let _ = me;
                #load
            }
        }
    });
    out.extend(match mode {
        Mode::Shared => quote! {
            impl #impl_generics #p::RcPersist for #name #ty_generics #where_clause {
                fn save_rc(this: &::std::rc::Rc<Self>, w: &mut #p::SaveWriter) {
                    w.shared_direct(this);
                }
                fn load_rc(r: &mut #p::SaveReader) -> ::std::rc::Rc<Self> {
                    r.shared_direct::<Self>()
                }
                fn load_weak(r: &mut #p::SaveReader) -> ::std::rc::Weak<Self> {
                    r.shared_direct_weak::<Self>()
                }
            }
        },
        Mode::Dyn(kind) => {
            let d = match kind {
                DynKind::Activity => quote! { dyn ::rustora_engine::activity::Activity },
                DynKind::Effect => quote! { dyn ::rustora_engine::effect::Effect },
            };
            quote! {
                impl #impl_generics #p::RcPersist for #name #ty_generics #where_clause {
                    fn save_rc(this: &::std::rc::Rc<Self>, w: &mut #p::SaveWriter) {
                        let d: ::std::rc::Rc<#d> = this.clone();
                        <#d as #p::RcPersist>::save_rc(&d, w);
                    }
                    fn load_rc(r: &mut #p::SaveReader) -> ::std::rc::Rc<Self> {
                        r.shared_concrete::<Self, #d>()
                    }
                    fn load_weak(r: &mut #p::SaveReader) -> ::std::rc::Weak<Self> {
                        r.shared_concrete_weak::<Self, #d>()
                    }
                }
            }
        }
        _ => unreachable!(),
    });
    Ok(out)
}

/// The `PersistState` methods: interior mutable fields are required to persist, other fields persist in place
/// when their type implements `PersistState` and are skipped otherwise.
fn state_impl(input: &DeriveInput) -> syn::Result<TokenStream> {
    let p = quote! { ::rustora_engine::persist };
    let mut saves = Vec::new();
    let mut loads = Vec::new();
    if let Data::Enum(data) = &input.data {
        let name = &input.ident;
        let mut save_arms = Vec::new();
        let mut load_arms = Vec::new();
        for (vi, v) in data.variants.iter().enumerate() {
            let vname = &v.ident;
            let idx = vi as u32;
            let mut binds = Vec::new();
            let mut vs = Vec::new();
            let mut vl = Vec::new();
            for (i, f) in v.fields.iter().enumerate() {
                let a = field_attrs(f)?;
                let bind = format_ident!("f{}", i);
                match &f.ident {
                    Some(id) => binds.push(quote! { #id: #bind }),
                    None => binds.push(quote! { #bind }),
                }
                if a.skip || a.me {
                    continue;
                }
                if is_interior_mutable(&f.ty) {
                    vs.push(quote! { #p::PersistState::save_state(#bind, w); });
                    vl.push(quote! { #p::PersistState::load_state(#bind, r); });
                } else {
                    vs.push(quote! { (&#p::Probe(#bind)).probe_save(w); });
                    vl.push(quote! { (&#p::Probe(#bind)).probe_load(r); });
                }
            }
            let pattern = match &v.fields {
                Fields::Named(_) => quote! { #name::#vname { #(#binds),* } },
                Fields::Unnamed(_) => quote! { #name::#vname ( #(#binds),* ) },
                Fields::Unit => quote! { #name::#vname },
            };
            save_arms.push(quote! {
                #[allow(unused_variables)]
                #pattern => {
                    w.u32(#idx);
                    #(#vs)*
                }
            });
            load_arms.push(quote! {
                #[allow(unused_variables)]
                #pattern => {
                    if r.u32() != #idx {
                        r.fail(format!("the variant of {} differs in the loaded world", stringify!(#name)));
                    }
                    #(#vl)*
                }
            });
        }
        saves.push(quote! { match self { #(#save_arms)* } });
        loads.push(quote! { match self { #(#load_arms)* } });
    }
    if let Data::Struct(data) = &input.data {
        let mut fields: Vec<(usize, &syn::Field)> = data.fields.iter().enumerate().collect();
        fields.sort_by_key(|(_, f)| !is_shared_cell(&f.ty));
        for (i, f) in fields {
            let a = field_attrs(f)?;
            if a.skip || a.me {
                continue;
            }
            let access = match &f.ident {
                Some(id) => quote! { self.#id },
                None => {
                    let idx = syn::Index::from(i);
                    quote! { self.#idx }
                }
            };
            if a.in_place {
                saves.push(quote! { #p::PersistState::save_state(&*#access.borrow(), w); });
                loads.push(quote! { #p::PersistState::load_state(&*#access.borrow(), r); });
            } else if is_interior_mutable(&f.ty) {
                saves.push(quote! { #p::PersistState::save_state(&#access, w); });
                loads.push(quote! { #p::PersistState::load_state(&#access, r); });
            } else {
                saves.push(quote! { (&#p::Probe(&#access)).probe_save(w); });
                loads.push(quote! { (&#p::Probe(&#access)).probe_load(r); });
            }
        }
    }
    Ok(quote! {
        #[allow(unused_imports)]
        fn save_state(&self, w: &mut #p::SaveWriter) {
            use #p::{ProbeNone as _, ProbeState as _};
            #(#saves)*
        }
        #[allow(unused_imports)]
        fn load_state(&self, r: &mut #p::SaveReader) {
            use #p::{ProbeNone as _, ProbeState as _};
            #(#loads)*
        }
    })
}

/// The bodies of `Persist::save` and of the loaders, which see `r` and `me`.
fn value_bodies(input: &DeriveInput, shared: bool) -> syn::Result<(TokenStream, TokenStream)> {
    let name = &input.ident;
    match &input.data {
        Data::Struct(data) => {
            let (pattern, construct) = fields_code(&data.fields, quote! { #name }, shared)?;
            let save_fields = save_fields(&data.fields)?;
            Ok((
                quote! {
                    let #pattern = self;
                    #save_fields
                },
                construct,
            ))
        }
        Data::Enum(data) => {
            let mut save_arms = Vec::new();
            let mut load_arms = Vec::new();
            for (i, v) in data.variants.iter().enumerate() {
                let vname = &v.ident;
                let idx = i as u32;
                let (pattern, construct) = fields_code(&v.fields, quote! { #name::#vname }, shared)?;
                let save_fields = save_fields(&v.fields)?;
                save_arms.push(quote! {
                    #pattern => {
                        w.u32(#idx);
                        #save_fields
                    }
                });
                load_arms.push(quote! { #idx => #construct, });
            }
            let ty = name.to_string();
            Ok((
                quote! {
                    #[allow(unused_variables)]
                    match self {
                        #(#save_arms)*
                    }
                },
                quote! {
                    match r.u32() {
                        #(#load_arms)*
                        v => r.fail(format!("invalid variant {v} of {}", #ty)),
                    }
                },
            ))
        }
        Data::Union(_) => Err(syn::Error::new_spanned(input, "Persist cannot be derived for unions")),
    }
}

/// The binding pattern of the fields and the expression that constructs them from the reader.
fn fields_code(fields: &Fields, path: TokenStream, shared: bool) -> syn::Result<(TokenStream, TokenStream)> {
    let p = quote! { ::rustora_engine::persist };
    let mut binds = Vec::new();
    let mut values = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let a = field_attrs(f)?;
        let bind = format_ident!("f{}", i);
        let value = if a.me {
            if !shared {
                return Err(syn::Error::new_spanned(f, "#[persist(me)] needs a shared container"));
            }
            quote! { me.clone() }
        } else if a.skip {
            quote! { ::std::default::Default::default() }
        } else {
            quote! { #p::Persist::load(r) }
        };
        match &f.ident {
            Some(id) => {
                binds.push(quote! { #id: #bind });
                values.push(quote! { #id: #value });
            }
            None => {
                binds.push(quote! { #bind });
                values.push(value);
            }
        }
    }
    Ok(match fields {
        Fields::Named(_) => (quote! { #path { #(#binds),* } }, quote! { #path { #(#values),* } }),
        Fields::Unnamed(_) => (quote! { #path ( #(#binds),* ) }, quote! { #path ( #(#values),* ) }),
        Fields::Unit => (quote! { #path }, quote! { #path }),
    })
}

fn save_fields(fields: &Fields) -> syn::Result<TokenStream> {
    let p = quote! { ::rustora_engine::persist };
    let mut out = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let a = field_attrs(f)?;
        let bind = format_ident!("f{}", i);
        if a.skip || a.me {
            out.push(quote! { let _ = #bind; });
        } else {
            out.push(quote! { #p::Persist::save(#bind, w); });
        }
    }
    Ok(quote! { #(#out)* })
}
