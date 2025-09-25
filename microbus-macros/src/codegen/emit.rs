use quote::{quote, format_ident};
use syn::{ItemImpl, ItemStruct};

use super::analyze::{ActiveSpec, InitSpec, MethodSpec, RetCase, StopSpec};
use super::parse::ActiveKind;

pub fn gen_ret_case_tokens(
    phase: &str,
    call_core: &proc_macro2::TokenStream,
    rc: &RetCase,
    abort_on_error: bool,
    ctx_ident: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    match rc {
        RetCase::Unit => quote! { let _ = #call_core.await; },
        RetCase::ResultUnit => {
            if abort_on_error {
                quote! { if let Err(e)=#call_core.await { tracing::error!(error=?e,#phase); mmg_microbus::component::__startup_mark_failed(&#ctx_ident); return Err(e);} }
            } else {
                quote! { if let Err(e)=#call_core.await { tracing::warn!(error=?e,#phase); } }
            }
        }
        RetCase::Some => quote! {{ let __v = #call_core.await; mmg_microbus::component::__publish_auto(&#ctx_ident,__v).await; }},
        RetCase::OptionSome => quote! {{ if let Some(__v)=#call_core.await { mmg_microbus::component::__publish_auto(&#ctx_ident,__v).await; } }},
        RetCase::ResultSome => {
            if abort_on_error {
                quote! { match #call_core.await { Ok(v)=> mmg_microbus::component::__publish_auto(&#ctx_ident,v).await, Err(e)=>{tracing::error!(error=?e,#phase); mmg_microbus::component::__startup_mark_failed(&#ctx_ident); return Err(e);} } }
            } else {
                quote! { match #call_core.await { Ok(v)=> mmg_microbus::component::__publish_auto(&#ctx_ident,v).await, Err(e)=>{tracing::warn!(error=?e,#phase);} } }
            }
        }
        RetCase::ResultOption => {
            if abort_on_error {
                quote! { match #call_core.await { Ok(opt)=> if let Some(v)=opt { mmg_microbus::component::__publish_auto(&#ctx_ident,v).await }, Err(e)=>{tracing::error!(error=?e,#phase); mmg_microbus::component::__startup_mark_failed(&#ctx_ident); return Err(e);} } }
            } else {
                quote! { match #call_core.await { Ok(opt)=> if let Some(v)=opt { mmg_microbus::component::__publish_auto(&#ctx_ident,v).await }, Err(e)=>{tracing::warn!(error=?e,#phase);} } }
            }
        }
    }
}

pub fn build_init_stop_calls(
    inits: &[InitSpec],
    stops: &[StopSpec],
) -> (Vec<proc_macro2::TokenStream>, Vec<proc_macro2::TokenStream>) {
    let mut init_calls = Vec::new();
    for i in inits { let ident = &i.ident; let core = if i.wants_ctx { quote! { this.#ident(&ctx) } } else { quote! { this.#ident() } }; let expr = gen_ret_case_tokens("init returned error", &core, &i.ret_case, true, &quote! {ctx}); init_calls.push(quote! { { #expr } }); }
    let mut stop_calls = Vec::new();
    for s in stops { let ident = &s.ident; let core = if s.wants_ctx { quote! { this.#ident(&ctx) } } else { quote! { this.#ident() } }; let expr = gen_ret_case_tokens("stop returned error", &core, &s.ret_case, false, &quote! {ctx}); stop_calls.push(quote! {{ #expr }}); }
    (init_calls, stop_calls)
}

pub struct RunParts {
    pub init_calls: Vec<proc_macro2::TokenStream>,
    pub stop_calls: Vec<proc_macro2::TokenStream>,
    pub sub_decls: Vec<proc_macro2::TokenStream>,
    pub handle_spawns: Vec<proc_macro2::TokenStream>,
    pub active_spawns: Vec<proc_macro2::TokenStream>,
    pub once_calls: Vec<proc_macro2::TokenStream>,
    pub compile_errors: Vec<proc_macro2::TokenStream>,
}

pub fn build_handle_parts(methods: &[MethodSpec]) -> (Vec<proc_macro2::TokenStream>, Vec<proc_macro2::TokenStream>) {
    let mut sub_decls = Vec::new();
    let mut handle_spawns = Vec::new();
    for (idx, ms) in methods.iter().enumerate() {
        let ty = &ms.msg_ty; let ident = &ms.ident; let sub_var = format_ident!("__sub_any_{}", idx);
        sub_decls.push(quote! { let mut #sub_var = mmg_microbus::component::__subscribe_any_auto::<#ty>(&ctx); });
        let core = if ms.wants_ctx { quote! { this.#ident(&ctx_c, &*env) } } else { quote! { this.#ident(&*env) } };
        let expr = gen_ret_case_tokens("handle returned error", &core, &ms.ret_case, false, &quote! {ctx_c});
        handle_spawns.push(quote! { let this_c = this.clone(); let ctx_c = ctx.__fork(); let mut sub = #sub_var; let __jh = tokio::spawn(async move { loop { tokio::select! { _ = mmg_microbus::component::__recv_stop(&ctx_c) => { break; } msg = sub.recv() => { match msg { Some(env) => { let this=&this_c; { #expr } } None => break, } } } } }); __workers.push(__jh); });
    }
    (sub_decls, handle_spawns)
}

pub fn build_active_parts(actives: &[ActiveSpec]) -> (Vec<proc_macro2::TokenStream>, Vec<proc_macro2::TokenStream>) {
    let mut active_spawns = Vec::new();
    let mut once_calls = Vec::new();
    for a in actives { let ident = &a.ident; if a.kind == ActiveKind::Once { let core = if a.wants_ctx { quote! { this.#ident(&ctx) } } else { quote! { this.#ident() } }; let expr = gen_ret_case_tokens("active returned error", &core, &a.ret_case, false, &quote! {ctx}); once_calls.push(expr); } else { let core_spawn = if a.wants_ctx { quote! { this.#ident(&ctx_c) } } else { quote! { this.#ident() } }; let expr_spawn = gen_ret_case_tokens("active returned error", &core_spawn, &a.ret_case, false, &quote! {ctx_c}); active_spawns.push(quote! { let this_c = this.clone(); let ctx_c = ctx.__fork(); let __jh = tokio::spawn(async move { loop { tokio::select! { _ = mmg_microbus::component::__recv_stop(&ctx_c) => break, _ = async { let this=&this_c; { #expr_spawn } } => {} } } }); __workers.push(__jh); }); } }
    (active_spawns, once_calls)
}

pub fn gen_component_run(self_ty: &syn::Type, parts: &RunParts, item: &ItemImpl) -> proc_macro2::TokenStream {
    let RunParts { init_calls, stop_calls, sub_decls, handle_spawns, active_spawns, once_calls, compile_errors } = parts;
    let gen_run = quote! { #[async_trait::async_trait] impl mmg_microbus::component::Component for #self_ty { async fn run(self: Box<Self>, mut ctx: mmg_microbus::component::ComponentContext) -> mmg_microbus::error::Result<()> { let mut this=*self; #( #init_calls )* let this=std::sync::Arc::new(this); #( #sub_decls )* mmg_microbus::component::__startup_arrive_and_wait(&ctx).await; { #( #once_calls )* } let mut __workers:Vec<tokio::task::JoinHandle<()>>=Vec::new(); #( #handle_spawns )* #( #active_spawns )* mmg_microbus::component::__recv_stop(&ctx).await; #( #stop_calls )* Ok(()) } } };
    let mut errs_ts = proc_macro2::TokenStream::new(); for e in compile_errors { errs_ts.extend(e.clone()); }
    quote! { #item #gen_run #errs_ts }
}

pub fn component_for_struct(item: &ItemStruct, _args: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let struct_ident = &item.ident; let factory_ident = format_ident!("__{}Factory", struct_ident); let default_assert_ident = format_ident!("__AssertDefaultFor{}", struct_ident);
    quote! { #item trait #default_assert_ident { fn __assert_default(){ let _ = <#struct_ident as Default>::default(); } } #[doc(hidden)] #[derive(Default)] struct #factory_ident; #[async_trait::async_trait] impl mmg_microbus::component::ComponentFactory for #factory_ident { fn type_name(&self)->&'static str { std::any::type_name::<#struct_ident>() } async fn build(&self,_bus: mmg_microbus::bus::BusHandle)-> mmg_microbus::error::Result<Box<dyn mmg_microbus::component::Component>> { Ok(Box::new(<#struct_ident as Default>::default())) } } #[doc(hidden)] const _: () = { fn __create_factory_for() -> Box<dyn mmg_microbus::component::ComponentFactory> { Box::new(#factory_ident::default()) } inventory::submit! { mmg_microbus::component::__RegisteredFactory { create: __create_factory_for } }; }; }
}
