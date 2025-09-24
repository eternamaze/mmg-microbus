use quote::quote;

use super::analyze::ActiveSpec;
use super::emit_ret::gen_ret_case_tokens;
use super::parse::ActiveKind;

// active 方法（loop / once）生成
pub fn build_active_parts(
    actives: &[ActiveSpec],
) -> (Vec<proc_macro2::TokenStream>, Vec<proc_macro2::TokenStream>) {
    let mut active_spawns = Vec::new();
    let mut once_calls = Vec::new();
    for a in actives {
        let ident = &a.ident;
        match a.kind {
            ActiveKind::Once => {
                let core = if a.wants_ctx {
                    quote! { this.#ident(&ctx) }
                } else {
                    quote! { this.#ident() }
                };
                let expr = gen_ret_case_tokens(
                    "active returned error",
                    &core,
                    &a.ret_case,
                    false,
                    &quote! {ctx},
                );
                once_calls.push(expr);
            }
            ActiveKind::Loop => {
                let core_spawn = if a.wants_ctx {
                    quote! { this.#ident(&ctx_c) }
                } else {
                    quote! { this.#ident() }
                };
                let expr_spawn = gen_ret_case_tokens(
                    "active returned error",
                    &core_spawn,
                    &a.ret_case,
                    false,
                    &quote! {ctx_c},
                );
                let spawn_token = quote! {
                    let this_c = this.clone();
                    let ctx_c = ctx.__fork();
                    let __jh = tokio::spawn(async move {
                        loop {
                            tokio::select! {
                                _ = mmg_microbus::component::__recv_stop(&ctx_c) => break,
                                _ = async { let this=&this_c; { #expr_spawn } } => {}
                            }
                        }
                    });
                    __workers.push(__jh);
                };
                active_spawns.push(spawn_token);
            }
        }
    }
    (active_spawns, once_calls)
}
