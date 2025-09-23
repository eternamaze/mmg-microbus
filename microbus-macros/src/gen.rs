use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Attribute, Ident, Item, ItemImpl, ItemStruct, Type};

pub fn component_entry(args: TokenStream, input: TokenStream) -> TokenStream {
    let args_ts = proc_macro2::TokenStream::from(args);
    let item_any = parse_macro_input!(input as Item);
    match item_any {
        Item::Struct(item) => component_for_struct(&item, args_ts),
        Item::Impl(item) => component_for_impl(&item),
        other => syn::Error::new_spanned(other, "#[component] only supports struct or impl blocks")
            .to_compile_error()
            .into(),
    }
}

fn component_for_struct(item: &ItemStruct, _args: proc_macro2::TokenStream) -> TokenStream {
    let struct_ident = &item.ident;
    let factory_ident = format_ident!("__{}Factory", struct_ident);
    // 显式 Default 约束：若目标类型未实现 Default，这里生成的伪 trait 绑定会触发编译期错误，提供清晰信息。
    let default_assert_ident = format_ident!("__AssertDefaultFor{}", struct_ident);
    let expanded = quote! {
        #item
    // 编译期断言：类型必须实现 Default
        trait #default_assert_ident { fn __assert_default() { let _ = <#struct_ident as Default>::default(); } }
        #[doc(hidden)]
        #[derive(Default)]
        struct #factory_ident;
        #[async_trait::async_trait]
        impl mmg_microbus::component::ComponentFactory for #factory_ident {
            fn type_name(&self) -> &'static str { std::any::type_name::<#struct_ident>() }
            async fn build(&self, _bus: mmg_microbus::bus::BusHandle) -> mmg_microbus::error::Result<Box<dyn mmg_microbus::component::Component>> { Ok(Box::new(<#struct_ident as Default>::default())) }
        }
        #[doc(hidden)]
        const _: () = {
            fn __create_factory_for() -> Box<dyn mmg_microbus::component::ComponentFactory> { Box::new(#factory_ident::default()) }
            inventory::submit! { mmg_microbus::component::__RegisteredFactory { create: __create_factory_for } };
        };
    };
    expanded.into()
}

fn component_for_impl(item: &ItemImpl) -> TokenStream {
    let self_ty = item.self_ty.clone();
    generate_run_impl_inner(item, &self_ty)
}

// === 语义辅助：返回值分类 ===

#[derive(Clone)]
enum RetCase {
    Unit,
    Some,
    OptionSome,
    ResultUnit,
    ResultSome,
    ResultOption,
}

/// 解析函数返回类型，归类到六种 `RetCase`。
fn analyze_return(sig: &syn::Signature) -> RetCase {
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
                    RetCase::OptionSome
                } else {
                    RetCase::Some
                }
            }
            _ => RetCase::Some,
        },
    }
}

fn is_ctx_type(ty: &syn::Type) -> bool {
    if let syn::Type::Reference(r) = ty {
        if let syn::Type::Path(tp) = &*r.elem {
            return tp
                .path
                .segments
                .last()
                .is_some_and(|s| s.ident == "ComponentContext");
        }
    }
    false
}
fn parse_msg_arg_ref(ty: &syn::Type) -> Option<Type> {
    if let syn::Type::Reference(r) = ty {
        if let syn::Type::Path(tp) = &*r.elem {
            return Some(Type::Path(tp.clone()));
        }
    }
    None
}
fn get_param_ident(p: &syn::Pat) -> Option<Ident> {
    if let syn::Pat::Ident(pi) = p {
        Some(pi.ident.clone())
    } else {
        None
    }
}

struct MethodSpec {
    ident: syn::Ident,
    msg_ty: Type,
    wants_ctx: bool,
    ret_case: RetCase,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum ActiveKind {
    Loop,
    Once,
}
struct ActiveSpec {
    ident: syn::Ident,
    wants_ctx: bool,
    ret_case: RetCase,
    kind: ActiveKind,
}
struct InitSpec {
    ident: syn::Ident,
    wants_ctx: bool,
    ret_case: RetCase,
}
struct StopSpec {
    ident: syn::Ident,
    wants_ctx: bool,
    ret_case: RetCase,
}

fn parse_handle_attr(a: &Attribute) -> bool {
    a.meta.require_path_only().is_err()
}
fn parse_active_kind(a: &Attribute) -> Option<syn::Result<ActiveKind>> {
    let last = a
        .path()
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default();
    if last.as_str() != "active" {
        return None;
    }
    match &a.meta {
        syn::Meta::Path(_) => Some(Ok(ActiveKind::Loop)),
        syn::Meta::List(list_meta) => {
            if list_meta.tokens.is_empty() {
                return Some(Ok(ActiveKind::Loop));
            }
            let content = list_meta.tokens.to_string();
            if content.trim() == "once" {
                Some(Ok(ActiveKind::Once))
            } else {
                Some(Err(syn::Error::new_spanned(
                    &list_meta.tokens,
                    "#[active] only supports (once)",
                )))
            }
        }
        syn::Meta::NameValue(nv) => Some(Err(syn::Error::new_spanned(
            nv,
            "#[active] does not take name-value arguments",
        ))),
    }
}

// Helper 提前到模块级，避免 items_after_statements 与参数过度移动
fn gen_ret_case_tokens(
    phase: &str,
    call_core: &proc_macro2::TokenStream,
    rc: &RetCase,
    abort_on_error: bool,
) -> proc_macro2::TokenStream {
    match rc {
        RetCase::Unit => quote! { let _ = #call_core.await; },
        RetCase::ResultUnit => {
            if abort_on_error {
                quote! { if let Err(e)=#call_core.await { tracing::error!(error=?e, #phase); mmg_microbus::component::__startup_mark_failed(&ctx); return Err(e); } }
            } else {
                quote! { if let Err(e)=#call_core.await { tracing::warn!(error=?e, #phase); } }
            }
        }
        RetCase::Some => {
            quote! { { let __v = #call_core.await; mmg_microbus::component::__publish_auto(&ctx, __v).await; } }
        }
        RetCase::OptionSome => {
            quote! { { if let Some(__v)=#call_core.await { mmg_microbus::component::__publish_auto(&ctx, __v).await; } } }
        }
        RetCase::ResultSome => {
            if abort_on_error {
                quote! { match #call_core.await { Ok(v)=> mmg_microbus::component::__publish_auto(&ctx, v).await, Err(e)=> { tracing::error!(error=?e, #phase); mmg_microbus::component::__startup_mark_failed(&ctx); return Err(e); } } }
            } else {
                quote! { match #call_core.await { Ok(v)=> mmg_microbus::component::__publish_auto(&ctx, v).await, Err(e)=> { tracing::warn!(error=?e, #phase); } } }
            }
        }
        RetCase::ResultOption => {
            if abort_on_error {
                quote! { match #call_core.await { Ok(opt)=> if let Some(v)=opt { mmg_microbus::component::__publish_auto(&ctx, v).await }, Err(e)=> { tracing::error!(error=?e, #phase); mmg_microbus::component::__startup_mark_failed(&ctx); return Err(e); } } }
            } else {
                quote! { match #call_core.await { Ok(opt)=> if let Some(v)=opt { mmg_microbus::component::__publish_auto(&ctx, v).await }, Err(e)=> { tracing::warn!(error=?e, #phase); } } }
            }
        }
    }
}

// === 拆分的小函数（降低复杂度） ===
fn collect_handles(item: &ItemImpl) -> (Vec<MethodSpec>, Vec<proc_macro2::TokenStream>) {
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
                        errs.push(quote! { compile_error!("#[handle] does not accept any arguments in this model"); });
                    }
                }
            }
            if handle_attr_count > 1 {
                errs.push(quote! { compile_error!("a method can only have one #[handle(...)] attribute"); });
            }
            if has_handle_attr {
                let mut wants_ctx = false;
                let mut duplicate_ctx = false;
                let mut candidates: Vec<(Option<Ident>, Type)> = Vec::new();
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
                            let name = get_param_ident(&pat_ty.pat);
                            candidates.push((name, t));
                        }
                    }
                }
                if duplicate_ctx {
                    errs.push(quote! { compile_error!("#[handle] allows at most one &ComponentContext parameter") });
                }
                let chosen = if candidates.len() == 1 {
                    Some(candidates[0].1.clone())
                } else if candidates.is_empty() {
                    errs.push(quote! { compile_error!("#[handle] requires exactly one &T parameter (message payload)") });
                    None
                } else {
                    errs.push(quote! { compile_error!("#[handle] allows only one &T parameter; remove extras") });
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

fn collect_actives(item: &ItemImpl) -> (Vec<ActiveSpec>, Vec<proc_macro2::TokenStream>) {
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
                        errs.push(syn::Error::new_spanned(&m.sig, "#[active] method cannot take &mut self; use interior mutability if needed").to_compile_error());
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
                        syn::Error::new_spanned(
                            &m.sig,
                            "#[active] allows at most one &ComponentContext parameter",
                        )
                        .to_compile_error(),
                    );
                    continue;
                }
                if !extra.is_empty() {
                    errs.push(syn::Error::new_spanned(&m.sig, "#[active] method can only take &ComponentContext as parameter; other &T parameters are not allowed").to_compile_error());
                    continue;
                }
                actives.push(ActiveSpec {
                    ident: m.sig.ident.clone(),
                    wants_ctx,
                    ret_case: analyze_return(&m.sig),
                    kind: active_kind.unwrap_or(ActiveKind::Loop),
                });
            }
        }
    }
    (actives, errs)
}

fn collect_inits_stops(
    item: &ItemImpl,
) -> (Vec<InitSpec>, Vec<StopSpec>, Vec<proc_macro2::TokenStream>) {
    let mut inits = Vec::new();
    let mut stops = Vec::new();
    let mut compile_errors = Vec::new();
    for it in &item.items {
        if let syn::ImplItem::Fn(m) = it {
            let mut has_init = false;
            let mut has_stop = false;
            for a in &m.attrs {
                let last = a
                    .path()
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                if last == "init" {
                    has_init = true;
                }
                if last == "stop" {
                    has_stop = true;
                }
            }
            if has_init {
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
                    compile_errors.push(
                        syn::Error::new_spanned(
                            &m.sig,
                            "#[init] only allows optional &ComponentContext",
                        )
                        .to_compile_error(),
                    );
                }
                inits.push(InitSpec {
                    ident: m.sig.ident.clone(),
                    wants_ctx,
                    ret_case: analyze_return(&m.sig),
                });
            }
            if has_stop {
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
                    compile_errors.push(
                        syn::Error::new_spanned(
                            &m.sig,
                            "#[stop] allows at most one &ComponentContext parameter",
                        )
                        .to_compile_error(),
                    );
                }
                if !extraneous.is_empty() {
                    compile_errors.push(syn::Error::new_spanned(&m.sig, "#[stop] method must take only self or optionally &self plus &ComponentContext").to_compile_error());
                }
                if !duplicate_ctx && extraneous.is_empty() {
                    stops.push(StopSpec {
                        ident: m.sig.ident.clone(),
                        wants_ctx,
                        ret_case: analyze_return(&m.sig),
                    });
                }
            }
        }
    }
    (inits, stops, compile_errors)
}

fn build_init_stop_calls(
    inits: &[InitSpec],
    stops: &[StopSpec],
) -> (Vec<proc_macro2::TokenStream>, Vec<proc_macro2::TokenStream>) {
    let mut init_calls = Vec::new();
    for i in inits {
        let ident = &i.ident;
        let call_core = if i.wants_ctx {
            quote! { this.#ident(&ctx) }
        } else {
            quote! { this.#ident() }
        };
        let call_expr = gen_ret_case_tokens("init returned error", &call_core, &i.ret_case, true);
        init_calls.push(quote! { { #call_expr } });
    }
    let mut stop_calls = Vec::new();
    for s in stops {
        let ident = &s.ident;
        let call_core = if s.wants_ctx {
            quote! { this.#ident(&ctx) }
        } else {
            quote! { this.#ident() }
        };
        let call_expr = gen_ret_case_tokens("stop returned error", &call_core, &s.ret_case, false);
        stop_calls.push(quote! { { #call_expr } });
    }
    (init_calls, stop_calls)
}

struct RunParts {
    init_calls: Vec<proc_macro2::TokenStream>,
    stop_calls: Vec<proc_macro2::TokenStream>,
    sub_decls: Vec<proc_macro2::TokenStream>,
    select_arms: Vec<proc_macro2::TokenStream>,
    active_arms: Vec<proc_macro2::TokenStream>,
    once_calls: Vec<proc_macro2::TokenStream>,
    compile_errors: Vec<proc_macro2::TokenStream>,
}

fn gen_component_run(self_ty: &syn::Type, parts: &RunParts, item: &ItemImpl) -> TokenStream {
    let init_calls = &parts.init_calls;
    let stop_calls = &parts.stop_calls;
    let sub_decls = &parts.sub_decls;
    let select_arms = &parts.select_arms;
    let active_arms = &parts.active_arms;
    let once_calls = &parts.once_calls;
    let gen_run = quote! {
        #[async_trait::async_trait]
        impl mmg_microbus::component::Component for #self_ty {
            async fn run(self: Box<Self>, mut ctx: mmg_microbus::component::ComponentContext) -> mmg_microbus::error::Result<()> {
                let mut this = *self;
                #( #init_calls )*
                #( #sub_decls )*
                mmg_microbus::component::__startup_arrive_and_wait(&ctx).await;
                { #( #once_calls )* }
                tokio::task::yield_now().await;
                loop { tokio::select! { #( #select_arms )* #( #active_arms )* _ = mmg_microbus::component::__recv_stop(&ctx) => { break; } } }
                #( #stop_calls )*
                Ok(())
            }
        }
    };
    let mut compile_errors = proc_macro2::TokenStream::new();
    for e in &parts.compile_errors {
        compile_errors.extend(e.clone());
    }
    let expanded = quote! { #item #gen_run #compile_errors };
    expanded.into()
}

fn generate_run_impl_inner(item: &ItemImpl, self_ty: &syn::Type) -> TokenStream {
    let (methods, mut errs_h) = collect_handles(item);
    let (actives, mut errs_a) = collect_actives(item);
    let mut compile_errors: Vec<proc_macro2::TokenStream> = Vec::new();
    compile_errors.append(&mut errs_h);
    compile_errors.append(&mut errs_a);
    let (inits, stops, mut errs2) = collect_inits_stops(item);
    compile_errors.append(&mut errs2);
    let (init_calls, stop_calls) = build_init_stop_calls(&inits, &stops);
    // build select arms and active arms, also get once_calls
    let mut sub_decls = Vec::new();
    let mut select_arms = Vec::new();
    let mut active_arms = Vec::new();
    let mut once_calls = Vec::new();
    for (idx, ms) in methods.iter().enumerate() {
        let ty = &ms.msg_ty;
        let method_ident = &ms.ident;
        let sub_var = format_ident!("__sub_any_{}", idx);
        sub_decls.push(quote! { let mut #sub_var = mmg_microbus::component::__subscribe_any_auto::<#ty>(&ctx); });
        let call_core = if ms.wants_ctx {
            quote! { this.#method_ident(&ctx, &*env) }
        } else {
            quote! { this.#method_ident(&*env) }
        };
        let call_expr =
            gen_ret_case_tokens("handle returned error", &call_core, &ms.ret_case, false);
        select_arms.push(quote! { msg = #sub_var.recv() => { match msg { Some(env) => { { #call_expr } } None => { break; } } } });
    }
    for a in &actives {
        let method_ident = &a.ident;
        let call_core = if a.wants_ctx {
            quote! { this.#method_ident(&ctx) }
        } else {
            quote! { this.#method_ident() }
        };
        let call_expr =
            gen_ret_case_tokens("active returned error", &call_core, &a.ret_case, false);
        if a.kind == ActiveKind::Once {
            once_calls.push(call_expr);
        } else {
            active_arms.push(quote! { _ = async {} => { #call_expr } });
        }
    }
    let parts = RunParts {
        init_calls,
        stop_calls,
        sub_decls,
        select_arms,
        active_arms,
        once_calls,
        compile_errors,
    };
    gen_component_run(self_ty, &parts, item)
}
