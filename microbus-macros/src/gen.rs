//! 实现分离：此文件承载所有宏生成与内部逻辑，`lib.rs` 仅做入口与导出。
//! 设计要点：
//! - 不在 `lib.rs` 写具体实现，满足项目“接口声明与实现分离”约束。
//! - 所有内部 helper 保持 crate 私有（pub(crate)），仅暴露属性宏包装函数。

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Attribute, Ident, Item, ItemImpl, ItemStruct, Type};

pub(crate) fn component_entry(args: TokenStream, input: TokenStream) -> TokenStream {
    let args_ts = proc_macro2::TokenStream::from(args);
    let item_any = parse_macro_input!(input as Item);
    match item_any {
        Item::Struct(item) => component_for_struct(item, args_ts),
        Item::Impl(item) => component_for_impl(item),
        other => syn::Error::new_spanned(other, "#[component] only supports struct or impl blocks")
            .to_compile_error()
            .into(),
    }
}

fn component_for_struct(item: ItemStruct, _args: proc_macro2::TokenStream) -> TokenStream {
    let struct_ident = &item.ident;
    let factory_ident = format_ident!("__{}Factory", struct_ident);
    // 显式 Default 约束：若目标类型未实现 Default，这里生成的伪 trait 绑定会触发编译期错误，提供清晰信息。
    let default_assert_ident = format_ident!("__AssertDefaultFor{}", struct_ident);
    let expanded = quote! {
        #item
        // 编译期断言：类型必须实现 Default
        #[allow(non_camel_case_types)]
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

fn component_for_impl(item: ItemImpl) -> TokenStream {
    let self_ty = item.self_ty.clone();
    generate_run_impl_inner(item, &self_ty)
}

// === 以下内容复制自原 lib.rs 中实现（保持语义不变），仅做分离 ===

#[derive(Clone)]
enum RetCase {
    Unit,
    Some,
    OptionSome,
    ResultUnit,
    ResultSome,
    ResultOption,
}

/// 解析函数返回类型，归类到六种 RetCase。
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
                .map(|s| s.ident == "ComponentContext")
                .unwrap_or(false);
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
        syn::Meta::List(list) => {
            if list.tokens.is_empty() {
                return Some(Ok(ActiveKind::Loop));
            }
            let content = list.tokens.to_string();
            if content.trim() == "once" {
                Some(Ok(ActiveKind::Once))
            } else {
                Some(Err(syn::Error::new_spanned(
                    &list.tokens,
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

pub(crate) fn generate_run_impl_inner(item: ItemImpl, self_ty: &syn::Type) -> TokenStream {
    let mut methods: Vec<MethodSpec> = Vec::new();
    let mut actives: Vec<ActiveSpec> = Vec::new();
    let mut compile_errors: Vec<proc_macro2::TokenStream> = Vec::new();

    for it in &item.items {
        if let syn::ImplItem::Fn(m) = it {
            // #[handle]
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
                        compile_errors.push(quote!{ compile_error!("#[handle] does not accept any arguments in this model"); });
                    }
                }
            }
            if handle_attr_count > 1 {
                compile_errors.push(quote!{ compile_error!("a method can only have one #[handle(...)] attribute"); });
            }
            if has_handle_attr {
                let mut wants_ctx = false;
                let mut duplicate_ctx = false;
                let mut candidates: Vec<(Option<Ident>, Type)> = Vec::new();
                for arg in &m.sig.inputs {
                    if let syn::FnArg::Typed(pat_ty) = arg {
                        if is_ctx_type(&pat_ty.ty) {
                            if wants_ctx { duplicate_ctx = true; }
                            wants_ctx = true;
                            continue;
                        }
                        if let Some(t) = parse_msg_arg_ref(&pat_ty.ty) {
                            let name = get_param_ident(&pat_ty.pat);
                            candidates.push((name, t));
                        }
                    }
                }
                if duplicate_ctx { compile_errors.push(quote!{ compile_error!("#[handle] allows at most one &ComponentContext parameter") }); }
                let chosen = if candidates.len() == 1 {
                    Some(candidates[0].1.clone())
                } else if candidates.is_empty() {
                    compile_errors.push(quote!{ compile_error!("#[handle] requires exactly one &T parameter (message payload)") });
                    None
                } else {
                    compile_errors.push(quote!{ compile_error!("#[handle] allows only one &T parameter; remove extras") });
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
            // #[active]
            let mut is_active = false;
            let mut active_kind = None;
            for a in &m.attrs {
                if let Some(res) = parse_active_kind(a) {
                    is_active = true;
                    match res {
                        Ok(k) => active_kind = Some(k),
                        Err(e) => {
                            return e.to_compile_error().into();
                        }
                    }
                }
            }
            if is_active {
                if let Some(rcv) = m.sig.receiver() {
                    if rcv.mutability.is_some() {
                        return syn::Error::new_spanned(&m.sig, "#[active] method cannot take &mut self; use interior mutability if needed").to_compile_error().into();
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
                                if wants_ctx { duplicate_ctx = true; }
                                wants_ctx = true;
                            } else if let Some(t) = parse_msg_arg_ref(&p.ty) {
                                extra.push(t);
                            }
                        }
                    }
                }
                if duplicate_ctx {
                    return syn::Error::new_spanned(&m.sig, "#[active] allows at most one &ComponentContext parameter").to_compile_error().into();
                }
                if !extra.is_empty() {
                    return syn::Error::new_spanned(&m.sig, "#[active] method can only take &ComponentContext as parameter; other &T parameters are not allowed").to_compile_error().into();
                }
                actives.push(ActiveSpec {
                    ident: m.sig.ident.clone(),
                    wants_ctx,
                    ret_case: analyze_return(&m.sig),
                    kind: active_kind.unwrap_or(ActiveKind::Loop),
                });
            }
            // #[init]/#[stop]
        }
    }

    // second pass for init/stop (need separate collection to keep logic clear)
    let mut inits: Vec<InitSpec> = Vec::new();
    let mut stops: Vec<StopSpec> = Vec::new();
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
                        syn::FnArg::Receiver(_) => {},
                        syn::FnArg::Typed(p) => {
                            if is_ctx_type(&p.ty) {
                                if wants_ctx { invalid_extra = true; } // 第二个 ctx 视为错误
                                wants_ctx = true;
                            } else {
                                invalid_extra = true; // 非 ctx 参数
                            }
                        }
                    }
                }
                if invalid_extra { compile_errors.push(syn::Error::new_spanned(&m.sig, "#[init] only allows optional &ComponentContext").to_compile_error()); }
                inits.push(InitSpec { ident: m.sig.ident.clone(), wants_ctx, ret_case: analyze_return(&m.sig) });
            }
            if has_stop {
                let mut extraneous = Vec::new();
                let mut wants_ctx = false;
                let mut duplicate_ctx = false;
                for arg in &m.sig.inputs {
                    if let syn::FnArg::Typed(p) = arg {
                        if is_ctx_type(&p.ty) {
                            if wants_ctx { duplicate_ctx = true; }
                            wants_ctx = true;
                        } else {
                            extraneous.push(p.ty.clone());
                        }
                    }
                }
                if duplicate_ctx { compile_errors.push(syn::Error::new_spanned(&m.sig, "#[stop] allows at most one &ComponentContext parameter").to_compile_error()); }
                if !extraneous.is_empty() {
                    compile_errors.push(syn::Error::new_spanned(&m.sig, "#[stop] method must take only self or optionally &self plus &ComponentContext").to_compile_error());
                }
                if !duplicate_ctx && extraneous.is_empty() {
                    stops.push(StopSpec { ident: m.sig.ident.clone(), wants_ctx, ret_case: analyze_return(&m.sig) });
                }
            }
        }
    }

    // codegen sections
    // ---- Helper: 生成针对不同 RetCase 的调用表达式（保持原语义与日志文案） ----
    fn gen_ret_case_tokens(
        phase: &str,
        call_core: proc_macro2::TokenStream,
        rc: &RetCase,
    ) -> proc_macro2::TokenStream {
        match rc {
            RetCase::Unit => quote! { let _ = #call_core.await; },
            RetCase::ResultUnit => {
                quote! { if let Err(e)=#call_core.await { tracing::warn!(error=?e, #phase); } }
            }
            RetCase::Some => {
                quote! { { let __v = #call_core.await; mmg_microbus::component::__publish_auto(&ctx, __v).await; } }
            }
            RetCase::OptionSome => {
                quote! { { if let Some(__v)=#call_core.await { mmg_microbus::component::__publish_auto(&ctx, __v).await; } } }
            }
            RetCase::ResultSome => {
                quote! { match #call_core.await { Ok(v)=> mmg_microbus::component::__publish_auto(&ctx, v).await, Err(e)=> tracing::warn!(error=?e, #phase) } }
            }
            RetCase::ResultOption => {
                quote! { match #call_core.await { Ok(opt)=> if let Some(v)=opt { mmg_microbus::component::__publish_auto(&ctx, v).await }, Err(e)=> tracing::warn!(error=?e, #phase) } }
            }
        }
    }

    let mut init_calls = Vec::new();
    for i in &inits {
        let ident = &i.ident;
        let call_core = if i.wants_ctx { quote! { this.#ident(&ctx) } } else { quote! { this.#ident() } };
        let call_expr = gen_ret_case_tokens("init returned error", call_core, &i.ret_case);
        init_calls.push(quote! { { #call_expr } });
    }
    let mut stop_calls = Vec::new();
    for s in &stops {
        let ident = &s.ident;
        let call_core = if s.wants_ctx {
            quote! { this.#ident(&ctx) }
        } else {
            quote! { this.#ident() }
        };
        let call_expr = gen_ret_case_tokens("stop returned error", call_core, &s.ret_case);
        stop_calls.push(quote! { { #call_expr } });
    }

    let mut sub_decls = Vec::new();
    let mut select_arms = Vec::new();
    let mut active_arms = Vec::new();
    for (idx, ms) in methods.iter().enumerate() {
        let ty = &ms.msg_ty;
        let method_ident = &ms.ident;
        let sub_var = format_ident!("__sub_any_{}", idx);
        sub_decls.push(quote! { let mut #sub_var = mmg_microbus::component::__subscribe_any_auto::<#ty>(&ctx).await; });
        let call_core = if ms.wants_ctx {
            quote! { this.#method_ident(&ctx, &*env) }
        } else {
            quote! { this.#method_ident(&*env) }
        };
        let call_expr = gen_ret_case_tokens("handle returned error", call_core, &ms.ret_case);
        select_arms.push(quote! { msg = #sub_var.recv() => { match msg { Some(env) => { { #call_expr } } None => { break; } } } });
    }

    let mut once_calls = Vec::new();
    let mut loop_call_bodies = Vec::new();
    for a in &actives {
        let method_ident = &a.ident;
        let call_core = if a.wants_ctx {
            quote! { this.#method_ident(&ctx) }
        } else {
            quote! { this.#method_ident() }
        };
        let call_expr = gen_ret_case_tokens("active returned error", call_core, &a.ret_case);
        if a.kind == ActiveKind::Once {
            once_calls.push(call_expr);
        } else {
            loop_call_bodies.push(call_expr);
        }
    }
    if !loop_call_bodies.is_empty() {
        active_arms.push(quote! { _ = async {} => { #( #loop_call_bodies )* } });
    }

    let gen_run = quote! {
        #[allow(unreachable_code)]
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

    let expanded = quote! { #item #gen_run #( #compile_errors )* };
    expanded.into()
}
