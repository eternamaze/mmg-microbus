use super::msgs::{
    ERR_ACTIVE_CTX_DUP, ERR_ACTIVE_MUT_SELF, ERR_ACTIVE_ONLY_CTX, ERR_HANDLE_CTX_DUP,
    ERR_HANDLE_MULTI_ATTR, ERR_HANDLE_MUT_SELF, ERR_HANDLE_NEED_ONE_T, ERR_HANDLE_NO_ARGS,
    ERR_HANDLE_ONLY_ONE_T, ERR_INIT_SIG, ERR_STOP_ASYNC_NOT_ALLOWED, ERR_STOP_CTX_DUP,
    ERR_STOP_MUT_SELF, ERR_STOP_SIG,
};
use quote::quote;
use syn::{ItemImpl, Type};

use super::parse::{
    is_ctx_type, parse_active_kind, parse_handle_attr, parse_msg_arg_ref, ActiveKind,
};

#[derive(Clone)]
pub enum RetCase {
    Unit,
    Some,
    OptionSome,
    ResultUnit,
    ResultSome,
    ResultOption,
    Erased,
    OptionErased,
    VecErased,
    AnyBox,
    AnyArc,
    OptionAnyBox,
    OptionAnyArc,
    ResultAnyBox,
    ResultAnyArc,
}

pub fn analyze_return(sig: &syn::Signature) -> RetCase {
    match &sig.output {
        syn::ReturnType::Default => RetCase::Unit,
        syn::ReturnType::Type(_, ty) => match &**ty {
            syn::Type::Tuple(t) if t.elems.is_empty() => RetCase::Unit,
            syn::Type::Path(tp) => {
                let last = tp
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                if last == "ErasedEvent" {
                    return RetCase::Erased;
                }
                if last == "Vec" {
                    if let Some(syn::PathArguments::AngleBracketed(ab)) =
                        tp.path.segments.last().map(|s| &s.arguments)
                    {
                        if let Some(syn::GenericArgument::Type(syn::Type::Path(inner_tp))) =
                            ab.args.first()
                        {
                            if inner_tp
                                .path
                                .segments
                                .last()
                                .map(|s| s.ident == "ErasedEvent")
                                .unwrap_or(false)
                            {
                                return RetCase::VecErased;
                            }
                        }
                    }
                }
                if last == "Result" {
                    if let Some(seg) = tp.path.segments.last() {
                        if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
                            if let Some(syn::GenericArgument::Type(ok_ty)) = ab.args.first() {
                                if let syn::Type::Tuple(t) = ok_ty {
                                    if t.elems.is_empty() {
                                        return RetCase::ResultUnit;
                                    }
                                }
                                if let syn::Type::Path(ok_tp) = ok_ty {
                                    // Result<Box<dyn Any>> / Result<Arc<dyn Any>>
                                    if let Some(last_ok) = ok_tp.path.segments.last() {
                                        let lname = last_ok.ident.to_string();
                                        if lname == "Box" {
                                            return RetCase::ResultAnyBox;
                                        } else if lname == "Arc" {
                                            return RetCase::ResultAnyArc;
                                        }
                                    }
                                    if ok_tp
                                        .path
                                        .segments
                                        .last()
                                        .map(|s| s.ident == "ErasedEvent")
                                        .unwrap_or(false)
                                    {
                                        return RetCase::Erased; // treat Result<ErasedEvent, _> as Erased side-effect publish
                                    }
                                    if ok_tp
                                        .path
                                        .segments
                                        .last()
                                        .map(|s| s.ident.to_string())
                                        .unwrap_or_default()
                                        == "Option"
                                    {
                                        return RetCase::ResultOption;
                                    }
                                }
                                return RetCase::ResultSome;
                            }
                        }
                    }
                    RetCase::ResultUnit
                } else if last == "Option" {
                    if let Some(syn::PathArguments::AngleBracketed(ab)) =
                        tp.path.segments.last().map(|s| &s.arguments)
                    {
                        if let Some(syn::GenericArgument::Type(syn::Type::Path(inner_tp))) =
                            ab.args.first()
                        {
                            if let Some(last_inner) = inner_tp.path.segments.last() {
                                let lname = last_inner.ident.to_string();
                                if lname == "Box" {
                                    return RetCase::OptionAnyBox;
                                } else if lname == "Arc" {
                                    return RetCase::OptionAnyArc;
                                }
                            }
                            if inner_tp
                                .path
                                .segments
                                .last()
                                .map(|s| s.ident == "ErasedEvent")
                                .unwrap_or(false)
                            {
                                return RetCase::OptionErased;
                            }
                        }
                    }
                    RetCase::OptionSome
                } else {
                    if last == "Box" {
                        return RetCase::AnyBox;
                    }
                    if last == "Arc" {
                        return RetCase::AnyArc;
                    }
                    RetCase::Some
                }
            }
            _ => RetCase::Some,
        },
    }
}

pub struct MethodSpec {
    pub ident: syn::Ident,
    pub msg_ty: Type,
    pub wants_ctx: bool,
    pub ret_case: RetCase,
}
pub struct ActiveSpec {
    pub ident: syn::Ident,
    pub wants_ctx: bool,
    pub ret_case: RetCase,
    pub kind: ActiveKind,
}
pub struct InitSpec {
    pub ident: syn::Ident,
    pub wants_ctx: bool,
    pub ret_case: RetCase,
}
pub struct StopSpec {
    pub ident: syn::Ident,
    pub wants_ctx: bool,
    pub ret_case: RetCase,
}

pub fn collect_handles(item: &ItemImpl) -> (Vec<MethodSpec>, Vec<proc_macro2::TokenStream>) {
    let mut methods = Vec::new();
    let mut errs = Vec::new();
    for it in &item.items {
        if let syn::ImplItem::Fn(m) = it {
            let mut has_handle_attr = false;
            let mut handle_attr_count = 0usize;
            for a in &m.attrs {
                let last = a
                    .path()
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                if last == "handle" {
                    has_handle_attr = true;
                    handle_attr_count += 1;
                    if parse_handle_attr(a) {
                        errs.push(quote! { compile_error!(#ERR_HANDLE_NO_ARGS); });
                    }
                }
            }
            if handle_attr_count > 1 {
                errs.push(quote! { compile_error!(#ERR_HANDLE_MULTI_ATTR); });
            }
            if has_handle_attr {
                if let Some(rcv) = m.sig.receiver() {
                    if rcv.mutability.is_some() {
                        errs.push(
                            syn::Error::new_spanned(&m.sig, ERR_HANDLE_MUT_SELF).to_compile_error(),
                        );
                        continue;
                    }
                }
                let mut wants_ctx = false;
                let mut duplicate_ctx = false;
                let mut candidates: Vec<Type> = Vec::new();
                for arg in &m.sig.inputs {
                    if let syn::FnArg::Typed(pat_ty) = arg {
                        if is_ctx_type(&pat_ty.ty) {
                            if wants_ctx {
                                duplicate_ctx = true;
                            }
                            wants_ctx = true;
                            continue;
                        }
                        if let Some(t) = parse_msg_arg_ref(&pat_ty.ty) {
                            candidates.push(t);
                        }
                    }
                }
                if duplicate_ctx {
                    errs.push(quote! { compile_error!(#ERR_HANDLE_CTX_DUP) });
                }
                let chosen = if candidates.len() == 1 {
                    Some(candidates[0].clone())
                } else if candidates.is_empty() {
                    errs.push(quote! { compile_error!(#ERR_HANDLE_NEED_ONE_T) });
                    None
                } else {
                    errs.push(quote! { compile_error!(#ERR_HANDLE_ONLY_ONE_T) });
                    None
                };
                if let Some(msg_ty) = chosen {
                    methods.push(MethodSpec {
                        ident: m.sig.ident.clone(),
                        msg_ty,
                        wants_ctx,
                        ret_case: analyze_return(&m.sig),
                    });
                }
            }
        }
    }
    (methods, errs)
}

pub fn collect_actives(item: &ItemImpl) -> (Vec<ActiveSpec>, Vec<proc_macro2::TokenStream>) {
    let mut actives = Vec::new();
    let mut errs = Vec::new();
    for it in &item.items {
        if let syn::ImplItem::Fn(m) = it {
            let mut is_active = false;
            let mut active_kind = None;
            for a in &m.attrs {
                if let Some(res) = parse_active_kind(a) {
                    is_active = true;
                    match res {
                        Ok(k) => active_kind = Some(k),
                        Err(e) => errs.push(e.to_compile_error()),
                    }
                }
            }
            if is_active {
                if let Some(rcv) = m.sig.receiver() {
                    if rcv.mutability.is_some() {
                        errs.push(
                            syn::Error::new_spanned(&m.sig, ERR_ACTIVE_MUT_SELF).to_compile_error(),
                        );
                        continue;
                    }
                }
                let mut wants_ctx = false;
                let mut duplicate_ctx = false;
                let mut extra: Vec<Type> = Vec::new();
                for arg in &m.sig.inputs {
                    match arg {
                        syn::FnArg::Receiver(_) => {}
                        syn::FnArg::Typed(p) => {
                            if is_ctx_type(&p.ty) {
                                if wants_ctx {
                                    duplicate_ctx = true;
                                }
                                wants_ctx = true;
                            } else if let Some(t) = parse_msg_arg_ref(&p.ty) {
                                extra.push(t);
                            }
                        }
                    }
                }
                if duplicate_ctx {
                    errs.push(
                        syn::Error::new_spanned(&m.sig, ERR_ACTIVE_CTX_DUP).to_compile_error(),
                    );
                    continue;
                }
                if !extra.is_empty() {
                    errs.push(
                        syn::Error::new_spanned(&m.sig, ERR_ACTIVE_ONLY_CTX).to_compile_error(),
                    );
                    continue;
                }
                actives.push(ActiveSpec {
                    ident: m.sig.ident.clone(),
                    wants_ctx,
                    ret_case: analyze_return(&m.sig),
                    kind: active_kind.unwrap_or(super::parse::ActiveKind::Loop),
                });
            }
        }
    }
    (actives, errs)
}

pub fn handle_init_fn(m: &syn::ImplItemFn) -> (Option<InitSpec>, Option<proc_macro2::TokenStream>) {
    let mut wants_ctx = false;
    let mut invalid_extra = false;
    for arg in &m.sig.inputs {
        match arg {
            syn::FnArg::Receiver(_) => {}
            syn::FnArg::Typed(p) => {
                if is_ctx_type(&p.ty) {
                    if wants_ctx {
                        invalid_extra = true;
                    }
                    wants_ctx = true;
                } else {
                    invalid_extra = true;
                }
            }
        }
    }
    if invalid_extra {
        let e = syn::Error::new_spanned(&m.sig, ERR_INIT_SIG).to_compile_error();
        return (None, Some(e));
    }
    let spec = InitSpec {
        ident: m.sig.ident.clone(),
        wants_ctx,
        ret_case: analyze_return(&m.sig),
    };
    (Some(spec), None)
}

pub fn handle_stop_fn(m: &syn::ImplItemFn) -> (Option<StopSpec>, Vec<proc_macro2::TokenStream>) {
    let mut compile_errors = Vec::new();
    // Enforce: #[stop] must be synchronous (non-async)
    if m.sig.asyncness.is_some() {
        compile_errors
            .push(syn::Error::new_spanned(&m.sig, ERR_STOP_ASYNC_NOT_ALLOWED).to_compile_error());
    }
    if let Some(rcv) = m.sig.receiver() {
        if rcv.mutability.is_some() {
            compile_errors
                .push(syn::Error::new_spanned(&m.sig, ERR_STOP_MUT_SELF).to_compile_error());
            return (None, compile_errors);
        }
    }
    let mut extraneous = Vec::new();
    let mut wants_ctx = false;
    let mut duplicate_ctx = false;
    for arg in &m.sig.inputs {
        if let syn::FnArg::Typed(p) = arg {
            if is_ctx_type(&p.ty) {
                if wants_ctx {
                    duplicate_ctx = true;
                }
                wants_ctx = true;
            } else {
                extraneous.push(p.ty.clone());
            }
        }
    }
    if duplicate_ctx {
        compile_errors.push(syn::Error::new_spanned(&m.sig, ERR_STOP_CTX_DUP).to_compile_error());
    }
    if !extraneous.is_empty() {
        compile_errors.push(syn::Error::new_spanned(&m.sig, ERR_STOP_SIG).to_compile_error());
    }
    if duplicate_ctx || !extraneous.is_empty() {
        return (None, compile_errors);
    }
    let spec = StopSpec {
        ident: m.sig.ident.clone(),
        wants_ctx,
        ret_case: analyze_return(&m.sig),
    };
    (Some(spec), compile_errors)
}

pub fn collect_inits(item: &ItemImpl) -> (Vec<InitSpec>, Vec<proc_macro2::TokenStream>) {
    let mut inits = Vec::new();
    let mut compile_errors = Vec::new();
    for it in &item.items {
        if let syn::ImplItem::Fn(m) = it {
            let has_init = m
                .attrs
                .iter()
                .any(|a| a.path().segments.last().is_some_and(|s| s.ident == "init"));
            if has_init {
                let (spec, err) = handle_init_fn(m);
                if let Some(s) = spec {
                    inits.push(s);
                }
                if let Some(e) = err {
                    compile_errors.push(e);
                }
            }
        }
    }
    (inits, compile_errors)
}

pub fn collect_stops(item: &ItemImpl) -> (Vec<StopSpec>, Vec<proc_macro2::TokenStream>) {
    let mut stops = Vec::new();
    let mut compile_errors = Vec::new();
    for it in &item.items {
        if let syn::ImplItem::Fn(m) = it {
            let has_stop = m
                .attrs
                .iter()
                .any(|a| a.path().segments.last().is_some_and(|s| s.ident == "stop"));
            if has_stop {
                let (spec, mut errs) = handle_stop_fn(m);
                if let Some(s) = spec {
                    stops.push(s);
                }
                compile_errors.append(&mut errs);
            }
        }
    }
    (stops, compile_errors)
}
