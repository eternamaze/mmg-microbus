use quote::{format_ident, quote};

use super::analyze::MethodSpec;
use super::emit_ret::gen_ret_case_tokens;

// handle 方法的订阅声明与 worker 生成
pub fn build_handle_parts(
    methods: &[MethodSpec],
) -> (Vec<proc_macro2::TokenStream>, Vec<proc_macro2::TokenStream>) {
    let mut sub_decls = Vec::new();
    let mut handle_spawns = Vec::new();
    for (idx, ms) in methods.iter().enumerate() {
        let ty = &ms.msg_ty;
        let ident = &ms.ident;
        let sub_var = format_ident!("__sub_any_{}", idx);
        // 订阅声明
        sub_decls.push(quote! { let mut #sub_var = mmg_microbus::component::__subscribe_any_auto::<#ty>(&ctx); });

        // 核心调用表达式 (区分是否需要 ctx)
        let core = if ms.wants_ctx {
            quote! { this.#ident(&ctx_c, &*env) }
        } else {
            quote! { this.#ident(&*env) }
        };
        let expr = gen_ret_case_tokens(
            "handle returned error",
            &core,
            &ms.ret_case,
            false,
            &quote! {ctx_c},
        );

        // 通用 worker 模板：停机 select + 消息循环
        let spawn_token = quote! {
            let this_c = this.clone();
            let ctx_c = ctx.__fork();
            let mut sub = #sub_var;
            let __jh = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = mmg_microbus::component::__recv_stop(&ctx_c) => { break; }
                        msg = sub.recv() => {
                            match msg {
                                Some(env) => { let this=&this_c; { #expr } }
                                None => break,
                            }
                        }
                    }
                }
            });
            __workers.push(__jh);
        };
        handle_spawns.push(spawn_token);
    }
    (sub_decls, handle_spawns)
}
