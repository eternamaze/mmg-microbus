use quote::quote;

use super::analyze::RetCase;

// 单一职责：根据返回值分类生成处理 token
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
        RetCase::Some => {
            quote! {{ let __v = #call_core.await; mmg_microbus::component::__publish_auto(&#ctx_ident,__v).await; }}
        }
        RetCase::OptionSome => {
            quote! {{ if let Some(__v)=#call_core.await { mmg_microbus::component::__publish_auto(&#ctx_ident,__v).await; } }}
        }
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
