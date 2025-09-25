use quote::{format_ident, quote};
use syn::{ItemImpl, ItemStruct};

use super::analyze::{InitSpec, StopSpec};
use super::emit_ret::gen_ret_case_tokens;

// 分离：初始化 / 停止 钩子调用列表生成
pub fn build_init_stop_calls(
    inits: &[InitSpec],
    stops: &[StopSpec],
) -> (Vec<proc_macro2::TokenStream>, Vec<proc_macro2::TokenStream>) {
    let mut init_calls = Vec::new();
    for i in inits {
        let ident = &i.ident;
        let core = if i.wants_ctx {
            quote! { this.#ident(&ctx) }
        } else {
            quote! { this.#ident() }
        };
        let expr = gen_ret_case_tokens(
            "init returned error",
            &core,
            &i.ret_case,
            true,
            &quote! {ctx},
        );
        init_calls.push(quote! { { #expr } });
    }
    let mut stop_calls = Vec::new();
    for s in stops {
        let ident = &s.ident;
        let core = if s.wants_ctx {
            quote! { this.#ident(&ctx) }
        } else {
            quote! { this.#ident() }
        };
        let expr = match &s.ret_case {
            super::analyze::RetCase::Unit => quote! { let _ = #core; },
            super::analyze::RetCase::ResultUnit => {
                quote! { match #core { Ok(()) => {}, Err(e) => { tracing::warn!(error=?e, "stop returned error"); } } }
            }
            super::analyze::RetCase::Some => {
                quote! { { let __v = #core; mmg_microbus::component::__publish_auto(&ctx, __v).await; } }
            }
            super::analyze::RetCase::OptionSome => {
                quote! { { if let Some(__v) = #core { mmg_microbus::component::__publish_auto(&ctx, __v).await; } } }
            }
            super::analyze::RetCase::ResultSome => {
                quote! { match #core { Ok(v) => { mmg_microbus::component::__publish_auto(&ctx, v).await; }, Err(e) => { tracing::warn!(error=?e, "stop returned error"); } } }
            }
            super::analyze::RetCase::ResultOption => {
                quote! { match #core { Ok(opt) => { if let Some(v) = opt { mmg_microbus::component::__publish_auto(&ctx, v).await; } }, Err(e) => { tracing::warn!(error=?e, "stop returned error"); } } }
            }
        };
        stop_calls.push(quote! { { #expr } });
    }
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

// 生成 run impl 的最终组装：保持线性可读
pub fn gen_component_run(
    self_ty: &syn::Type,
    parts: &RunParts,
    item: &ItemImpl,
) -> proc_macro2::TokenStream {
    let RunParts {
        init_calls,
        stop_calls,
        sub_decls,
        handle_spawns,
        active_spawns,
        once_calls,
        compile_errors,
    } = parts;
    // run 本体：阶段顺序：init -> 订阅声明 -> startup barrier -> once -> workers -> 等待 stop -> 立刻调用 stop 钩子（不等待 worker）
    let run_impl = quote! {
        #[async_trait::async_trait]
        impl mmg_microbus::component::Component for #self_ty {
            async fn run(self: Box<Self>, mut ctx: mmg_microbus::component::ComponentContext) -> mmg_microbus::error::Result<()> {
                let mut this=*self; #( #init_calls )* let this=std::sync::Arc::new(this);
                #( #sub_decls )*
                mmg_microbus::component::__startup_arrive_and_wait(&ctx).await;
                { #( #once_calls )* }
                let mut __workers:Vec<tokio::task::JoinHandle<()>>=Vec::new();
                #( #handle_spawns )*
                #( #active_spawns )*
                mmg_microbus::component::__recv_stop(&ctx).await;
                // 同步停机契约：收到 stop 后立即执行 stop 钩子，不等待任何 worker 结束
                #( #stop_calls )*
                Ok(())
            }
        }
    };
    let mut errs_ts = proc_macro2::TokenStream::new();
    for e in compile_errors {
        errs_ts.extend(e.clone());
    }
    quote! { #item #run_impl #errs_ts }
}

// struct 派生入口（维持原始语义）
pub fn component_for_struct(
    item: &ItemStruct,
    _args: proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let struct_ident = &item.ident;
    let factory_ident = format_ident!("__{}Factory", struct_ident);
    let default_assert_ident = format_ident!("__AssertDefaultFor{}", struct_ident);
    quote! {
        #item
        trait #default_assert_ident { fn __assert_default(){ let _ = <#struct_ident as Default>::default(); } }
        #[doc(hidden)] #[derive(Default)] struct #factory_ident;
        #[async_trait::async_trait]
        impl mmg_microbus::component::ComponentFactory for #factory_ident {
            fn type_name(&self)->&'static str { std::any::type_name::<#struct_ident>() }
            async fn build(&self,_bus: mmg_microbus::bus::BusHandle)-> mmg_microbus::error::Result<Box<dyn mmg_microbus::component::Component>> { Ok(Box::new(<#struct_ident as Default>::default())) }
        }
        #[doc(hidden)] const _: () = {
            fn __create_factory_for() -> Box<dyn mmg_microbus::component::ComponentFactory> { Box::new(#factory_ident::default()) }
            inventory::submit! { mmg_microbus::component::__RegisteredFactory { create: __create_factory_for } };
        };
    }
}
