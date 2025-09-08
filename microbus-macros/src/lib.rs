use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::Ident;
use syn::{parse_macro_input, Attribute, Item, ItemImpl, ItemStruct, LitStr, Token, Type};

// 仅保留强类型“方法即订阅”路径。

// removed: component_factory; single path is #[component] on struct + impl

#[proc_macro_attribute]
pub fn component(args: TokenStream, input: TokenStream) -> TokenStream {
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
    // Check fields; require `id` field
    let mut has_id = false;
    let mut init_fields: Vec<proc_macro2::TokenStream> = Vec::new();
    if let syn::Fields::Named(fields) = &item.fields {
        for f in &fields.named {
            if let Some(fid) = &f.ident {
                if fid == "id" {
                    has_id = true;
                    init_fields.push(quote! { id: id });
                } else {
                    // 其余字段统一用 Default::default() 初始化
                    let name = fid;
                    init_fields.push(quote! { #name: Default::default() });
                }
            }
        }
    }
    if !has_id {
        return syn::Error::new_spanned(
            &item,
            "#[component] requires a named field `id: ComponentId`",
        )
        .to_compile_error()
        .into();
    }
    let factory_ident = format_ident!("__{}Factory", struct_ident);
    let build_body = quote! { Ok(Box::new(#struct_ident { #( #init_fields ),* })) };
    let expanded = quote! {
        #item
        #[doc(hidden)]
        #[derive(Default)]
        struct #factory_ident;
        #[async_trait::async_trait]
        impl mmg_microbus::component::ComponentFactory for #factory_ident {
            fn kind_id(&self) -> mmg_microbus::bus::KindId { mmg_microbus::bus::KindId::of::<#struct_ident>() }
            fn type_name(&self) -> &'static str { std::any::type_name::<#struct_ident>() }
            async fn build(&self, id: mmg_microbus::bus::ComponentId, _bus: mmg_microbus::bus::BusHandle) -> anyhow::Result<Box<dyn mmg_microbus::component::Component>> { #build_body }
        }
        impl mmg_microbus::component::RegisteredComponent for #struct_ident {
            fn kind_id() -> mmg_microbus::bus::KindId { mmg_microbus::bus::KindId::of::<#struct_ident>() }
            fn type_name() -> &'static str { std::any::type_name::<#struct_ident>() }
            fn factory() -> mmg_microbus::component::DynFactory { std::sync::Arc::new(#factory_ident::default()) }
        }
    };
    expanded.into()
}

fn component_for_impl(item: ItemImpl) -> TokenStream {
    let self_ty = item.self_ty.clone();
    generate_run_impl_inner(item, &self_ty)
}

// 移除旧的 #[configure] 宏：配置改为在 handle 签名中以 &CfgType 参数注入

// --- 方法即订阅（强类型，&T-only） + 生命周期钩子 ---
// 用法（新）：
//   #[mmg_microbus::component]
//   impl MyComp {
//       #[mmg_microbus::handle]
//       async fn on_tick(&mut self, ctx: &mmg_microbus::component::ComponentContext, tick: &Tick) -> anyhow::Result<()> { Ok(()) }
//   }
// 说明：仅支持签名 (&ComponentContext, &T)，必须显式标注 #[handle]；#[handle] 仅用于实例过滤（instance/instances），不再支持 #[handle(T)] 旧语法。

#[proc_macro_attribute]
pub fn handle(_args: TokenStream, input: TokenStream) -> TokenStream {
    // 标记型属性：保持方法体不变，由 #[component] 标注的 impl 扩展阶段统一解析。
    input
}

/// 标记初始化函数，由框架在组件 run 进入主循环前调用（一次）。
#[proc_macro_attribute]
pub fn init(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

/// 标记停止函数，由框架在组件退出前调用（一次）。
#[proc_macro_attribute]
pub fn stop(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

fn generate_run_impl_inner(item: ItemImpl, self_ty: &syn::Type) -> TokenStream {
    // 解析 #[handle(...)]：只允许 instance/instances 字段
    #[derive(Default, Clone)]
    struct HandleAttr { instance_list: Vec<LitStr> }
    struct HandleArgs { instance_list: Vec<LitStr> }
    impl Parse for HandleArgs {
        fn parse(input: ParseStream) -> syn::Result<Self> {
            let mut instance_list: Vec<LitStr> = Vec::new();
            while !input.is_empty() {
                if input.peek(Token![,]) { let _c: Token![,] = input.parse()?; if input.is_empty() { break; } }
                if !(input.peek(Ident) && input.peek2(Token![=])) {
                    return Err(syn::Error::new(input.span(), "unknown or misplaced argument in #[handle(...)]"));
                }
                let key: Ident = input.parse()?;
                let _eq: Token![=] = input.parse()?;
                if key == "instance" {
                    let lit: LitStr = input.parse()?; instance_list.push(lit);
                } else if key == "instances" {
                    let content; syn::bracketed!(content in input);
                    while !content.is_empty() {
                        let lit: LitStr = content.parse()?; instance_list.push(lit);
                        if content.peek(Token![,]) { let _c: Token![,] = content.parse()?; }
                    }
                } else {
                    return Err(syn::Error::new(key.span(), "unknown key in #[handle(..)]"));
                }
                if input.peek(Token![,]) { let _c: Token![,] = input.parse()?; }
            }
            Ok(HandleArgs { instance_list })
        }
    }
    fn parse_handle_attr(a: &Attribute) -> Vec<LitStr> {
        a.parse_args::<HandleArgs>().map(|h| h.instance_list).unwrap_or_default()
    }

    // 收集处理方法规范
    struct MethodSpec { ident: syn::Ident, msg_ty: Type, wants_ctx: bool, instance_tokens: Vec<proc_macro2::TokenStream>, ret_case: RetCase }
    #[derive(Clone)]
    enum RetCase { Unit, ResultUnit, Some, ResultSome }
    fn analyze_return(sig: &syn::Signature) -> RetCase {
        match &sig.output {
            syn::ReturnType::Default => RetCase::Unit,
            syn::ReturnType::Type(_, ty) => match &**ty {
                syn::Type::Tuple(t) if t.elems.is_empty() => RetCase::Unit,
                syn::Type::Path(tp) => {
                    let last = tp.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
                    if last == "Result" {
                        if let Some(seg) = tp.path.segments.last() {
                            if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
                                if let Some(syn::GenericArgument::Type(ok_ty)) = ab.args.first() {
                                    if let syn::Type::Tuple(t) = ok_ty { if t.elems.is_empty() { return RetCase::ResultUnit; } }
                                    return RetCase::ResultSome;
                                }
                            }
                        }
                        RetCase::ResultUnit
                    } else { RetCase::Some }
                }
                _ => RetCase::Some,
            }
        }
    }
    fn is_ctx_type(ty: &syn::Type) -> bool {
        if let syn::Type::Reference(r) = ty { if let syn::Type::Path(tp) = &*r.elem { return tp.path.segments.last().map(|s| s.ident == "ComponentContext").unwrap_or(false); } }
        false
    }
    fn parse_msg_arg_ref(ty: &syn::Type) -> Option<Type> {
        if let syn::Type::Reference(r) = ty { if let syn::Type::Path(tp) = &*r.elem { return Some(Type::Path(tp.clone())); } }
        None
    }

    let mut methods: Vec<MethodSpec> = Vec::new();
    // Active methods
    #[derive(Clone)]
    struct ActiveSpec {
        ident: syn::Ident,
        wants_ctx: bool,
        ret_case: RetCase,
        interval_ms: Option<u64>,
        times: Option<u64>,
        immediate: bool,
    }
    #[derive(Default)]
    struct ActiveArgs {
        interval_ms: Option<u64>,
        times: Option<u64>,
        immediate: bool,
    }
    impl Parse for ActiveArgs {
        fn parse(input: ParseStream) -> syn::Result<Self> {
            let mut me = ActiveArgs::default();
            while !input.is_empty() {
                let key: Ident = input.parse()?;
                // optional =value
                if input.peek(Token![=]) {
                    let _eq: Token![=] = input.parse()?;
                    if key == "interval_ms" {
                        let lit: syn::LitInt = input.parse()?;
                        me.interval_ms = Some(lit.base10_parse::<u64>()?);
                    } else if key == "times" {
                        let lit: syn::LitInt = input.parse()?;
                        me.times = Some(lit.base10_parse::<u64>()?);
                    } else if key == "immediate" {
                        let lit: syn::LitBool = input.parse()?;
                        me.immediate = lit.value;
                    } else if key == "once" {
                        let lit: syn::LitBool = input.parse()?;
                        me.times = Some(1);
                        me.immediate = lit.value || me.immediate;
                    } else {
                        return Err(syn::Error::new(key.span(), "unknown key in #[active(...)]"));
                    }
                } else {
                    // bare flags
                    if key == "immediate" {
                        me.immediate = true;
                    } else if key == "once" {
                        me.times = Some(1);
                        me.immediate = true;
                    } else {
                        return Err(syn::Error::new(key.span(), "unknown key in #[active(...)]"));
                    }
                }
                if input.peek(Token![,]) {
                    let _c: Token![,] = input.parse()?;
                }
            }
            Ok(me)
        }
    }
    fn parse_active_args(a: &Attribute) -> Option<ActiveArgs> {
        let last = a
            .path()
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        if last.as_str() == "active" {
            a.parse_args::<ActiveArgs>().ok()
        } else {
            None
        }
    }
    let mut actives: Vec<ActiveSpec> = Vec::new();
    let mut compile_errors: Vec<proc_macro2::TokenStream> = Vec::new();
    for it in &item.items {
        if let syn::ImplItem::Fn(m) = it {
            // 处理 #[handle]
            let mut attr = HandleAttr::default();
            let mut has_handle_attr = false;
            let mut handle_attr_count = 0usize;
            for a in &m.attrs {
                let last = a
                    .path()
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                if last.as_str() == "handle" {
                    has_handle_attr = true;
                    handle_attr_count += 1;
                    let instances = parse_handle_attr(a);
                    attr.instance_list = instances;
                }
            }
            if handle_attr_count > 1 {
                compile_errors.push(quote! { compile_error!("a method can only have one #[handle(...)] attribute"); });
            }
            if has_handle_attr {
                // 新规则：签名必须严格为 (&ComponentContext, &T)
                let mut params: Vec<Type> = Vec::new();
                for arg in &m.sig.inputs {
                    if let syn::FnArg::Typed(pat_ty) = arg {
                        if is_ctx_type(&pat_ty.ty) {
                            // ctx 优先识别
                            params.push(syn::parse_quote!(__CTX_PLACEHOLDER__));
                        } else if let Some(t) = parse_msg_arg_ref(&pat_ty.ty) {
                            params.push(Type::Path(match t { Type::Path(tp) => tp, _ => unreachable!() }));
                        } else {
                            compile_errors.push(quote! { compile_error!("#[handle] method only allows (&ComponentContext, &T) parameters"); });
                        }
                    }
                }
                // 过滤掉 receiver self，本身不计入；期望 params 精确包含 [ctx, msg]
                if params.len() != 2 {
                    compile_errors.push(quote! { compile_error!("#[handle] method must take exactly two parameters: (&ComponentContext, &T)"); });
                    continue;
                } else {
                    // 检查顺序
                    match (&params[0], &params[1]) {
                        (Type::Path(tp0), Type::Path(tp1)) => {
                            let is_ctx0 = tp0.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default() == "__CTX_PLACEHOLDER__";
                            if is_ctx0 {
                                let wants_ctx = true;
                                let msg_ty = Type::Path(tp1.clone());
                                // 生成过滤表达式：仅支持按实例字符串过滤；缺省为任意来源
                                methods.push(MethodSpec {
                                    ident: m.sig.ident.clone(),
                                    msg_ty,
                                    wants_ctx,
                                    instance_tokens: attr.instance_list.iter().map(|lit| quote! { mmg_microbus::bus::ComponentId(#lit.to_string()) }).collect(),
                                    ret_case: analyze_return(&m.sig),
                                });
                            } else {
                                // 如果第一个不是 ctx，则不接受
                                compile_errors.push(quote! { compile_error!("#[handle] parameters must be in order (&ComponentContext, &T)"); });
                                continue;
                            }
                        }
                        _ => {
                            compile_errors.push(quote! { compile_error!("#[handle] method must take (&ComponentContext, &T)"); });
                            continue;
                        }
                    }
                }
            } // no else: 未标注 #[handle] 的方法视为普通方法

            // 处理 #[active]
            let mut is_active = false;
            let mut args_parsed: Option<ActiveArgs> = None;
            for a in &m.attrs {
                if let Some(parsed) = parse_active_args(a) {
                    is_active = true;
                    args_parsed = Some(parsed);
                }
            }
            if !is_active {
                continue;
            }
            // forbid &mut self to avoid concurrent mutable borrow in select loop
            if let Some(rcv) = m.sig.receiver() {
                if rcv.mutability.is_some() {
                    let err = syn::Error::new_spanned(
                        &m.sig,
                        "#[active] method cannot take &mut self; use interior mutability if needed",
                    );
                    return err.to_compile_error().into();
                }
            }
            let mut wants_ctx = false;
            let mut extra_ref_params: Vec<Type> = Vec::new();
            // no extra annotations needed
            for arg in &m.sig.inputs {
                match arg {
                    syn::FnArg::Receiver(_) => {}
                    syn::FnArg::Typed(p) => {
                        if is_ctx_type(&p.ty) {
                            wants_ctx = true;
                        } else if let Some(t) = parse_msg_arg_ref(&p.ty) {
                            extra_ref_params.push(t);
                        }
                    }
                }
            }
            let args_parsed = args_parsed.unwrap_or_default();
            if !extra_ref_params.is_empty() {
                // 主动函数禁止除 ctx 以外的参数
                let err = syn::Error::new_spanned(
                    &m.sig,
                    "#[active] method can only take &ComponentContext as parameter; other &T parameters are not allowed",
                );
                return err.to_compile_error().into();
            }
            actives.push(ActiveSpec {
                ident: m.sig.ident.clone(),
                wants_ctx,
                ret_case: analyze_return(&m.sig),
                interval_ms: args_parsed.interval_ms,
                times: args_parsed.times,
                immediate: args_parsed.immediate,
            });
        }
    }

    // 生成 run()：为每个 handle 创建订阅，并在 select 循环中分发
    // 收集 #[init] 与 #[stop]
    // #[init]: 仅允许 self + 可选一个 &CfgType；不允许 ctx 或多个配置
    let mut inits: Vec<(syn::Ident, Option<Type>, RetCase)> = Vec::new();
    // #[stop] 仅允许“仅 self”签名（&self 或 &mut self）；不允许 ctx 或任意额外参数
    let mut stops: Vec<(syn::Ident, RetCase)> = Vec::new();
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
                if last.as_str() == "init" {
                    has_init = true;
                }
                if last.as_str() == "stop" {
                    has_stop = true;
                }
            }
            if has_init || has_stop {
                // 收集 #[init] 的参数（支持 ctx 与若干 &CfgType）
                if has_init {
                    let mut cfg_ty: Option<Type> = None;
                    let mut errors = Vec::new();
                    for arg in &m.sig.inputs {
                        if let syn::FnArg::Typed(pat_ty) = arg {
                            if is_ctx_type(&pat_ty.ty) {
                                errors.push(syn::Error::new_spanned(&m.sig, "#[init] method cannot take &ComponentContext; only self and optionally one &CfgType are allowed").to_compile_error());
                                continue;
                            }
                            if let Some(t) = parse_msg_arg_ref(&pat_ty.ty) {
                                if cfg_ty.is_none() {
                                    cfg_ty = Some(t);
                                } else {
                                    errors.push(syn::Error::new_spanned(&m.sig, "#[init] method can take at most one &CfgType parameter").to_compile_error());
                                }
                            } else {
                                errors.push(syn::Error::new_spanned(&m.sig, "#[init] extra parameters are not allowed; expected at most one &CfgType").to_compile_error());
                            }
                        }
                    }
                    if !errors.is_empty() {
                        compile_errors.extend(errors);
                    }
                    let rc = analyze_return(&m.sig);
                    inits.push((m.sig.ident.clone(), cfg_ty.clone(), rc));
                }
                // 校验 #[stop]：只允许接收器（&self 或 &mut self），不允许其他参数
                if has_stop {
                    let mut extraneous = Vec::new();
                    for arg in &m.sig.inputs {
                        if let syn::FnArg::Typed(p) = arg {
                            extraneous.push(p.ty.clone());
                        }
                    }
                    if !extraneous.is_empty() {
                        let err = syn::Error::new_spanned(
                            &m.sig,
                            "#[stop] method must take only self (no context or config parameters)",
                        );
                        compile_errors.push(err.to_compile_error());
                    } else {
                        let rc = analyze_return(&m.sig);
                        stops.push((m.sig.ident.clone(), rc));
                    }
                }
            }
        }
    }

    // 生成 init/stop 调用代码
    let mut init_calls = Vec::new();
    for (ident, cfg_opt, rc) in inits.iter() {
        let call_expr = if let Some(cty) = cfg_opt {
            let var = format_ident!("__icfg_{}", ident);
            let call_core = quote! { this.#ident(&*#var) };
            let ret = match rc {
                RetCase::Unit => quote! { let _ = #call_core.await; },
                RetCase::ResultUnit => {
                    quote! { if let Err(e) = #call_core.await { tracing::warn!(error=?e, "init returned error"); } }
                }
                RetCase::Some => quote! { { let __v = #call_core.await; ctx.publish(__v).await; } },
                RetCase::ResultSome => {
                    quote! { match #call_core.await { Ok(v) => ctx.publish(v).await, Err(e) => tracing::warn!(error=?e, "init returned error") } }
                }
            };
            quote! {{
                let #var = ctx.config::<#cty>();
                if let Some(#var) = #var { #ret } else { tracing::error!("missing config for init"); }
            }}
        } else {
            let call_core = quote! { this.#ident() };
            let ret = match rc {
                RetCase::Unit => quote! { let _ = #call_core.await; },
                RetCase::ResultUnit => {
                    quote! { if let Err(e) = #call_core.await { tracing::warn!(error=?e, "init returned error"); } }
                }
                RetCase::Some => quote! { { let __v = #call_core.await; ctx.publish(__v).await; } },
                RetCase::ResultSome => {
                    quote! { match #call_core.await { Ok(v) => ctx.publish(v).await, Err(e) => tracing::warn!(error=?e, "init returned error") } }
                }
            };
            quote! {{ #ret }}
        };
        init_calls.push(call_expr);
    }
    let mut stop_calls = Vec::new();
    for (ident, rc) in stops.iter() {
        let call_core = quote! { this.#ident() };
        let call_expr = match rc {
            RetCase::Unit => quote! { let _ = #call_core.await; },
            RetCase::ResultUnit => {
                quote! { if let Err(e) = #call_core.await { tracing::warn!(error=?e, "stop returned error"); } }
            }
            RetCase::Some => quote! { { let __v = #call_core.await; ctx.publish(__v).await; } },
            RetCase::ResultSome => {
                quote! { match #call_core.await { Ok(v) => ctx.publish(v).await, Err(e) => tracing::warn!(error=?e, "stop returned error") } }
            }
        };
        stop_calls.push(quote! {{ #call_expr }});
    }
    let mut sub_decls = Vec::new();
    let mut select_arms = Vec::new();
    // prepare active tickers and state
    let mut active_decls = Vec::new();
    let mut active_arms = Vec::new();
    let mut active_pre_immediate = Vec::new();
    for (idx, ms) in methods.iter().enumerate() {
        let ty = &ms.msg_ty;
        let method_ident = &ms.ident;
        if ms.instance_tokens.is_empty() {
            // 订阅任意来源（类型级）
            let sub_var = format_ident!("__sub_any_{}", idx);
            sub_decls.push(quote! {
                let mut #sub_var = mmg_microbus::component::__subscribe_any_auto::<#ty>(&ctx).await;
            });
            // handle 仅支持可选 ctx 和单一消息 &T
            let call_core = if ms.wants_ctx {
                quote! { this.#method_ident(&ctx, &*env) }
            } else {
                quote! { this.#method_ident(&*env) }
            };
        // 根据返回类型自动处理发布：T / Result<T> 自动 publish；() / Result<()> 仅记录错误
            let call_expr = match &ms.ret_case {
                RetCase::Unit => quote! { let _ = #call_core.await; },
                RetCase::ResultUnit => {
                    quote! { if let Err(e) = #call_core.await { tracing::warn!(error=?e, "handle returned error"); } }
                }
                RetCase::Some => quote! { { let __v = #call_core.await; ctx.publish(__v).await; } },
                RetCase::ResultSome => {
                    quote! { match #call_core.await { Ok(v) => ctx.publish(v).await, Err(e) => tracing::warn!(error=?e, "handle returned error") } }
                }
            };
            select_arms.push(quote! {
                msg = #sub_var.recv() => {
                    match msg {
                        Some(env) => {
                            { #call_expr }
                        }
                        None => { break; }
                    }
                }
            });
        } else {
            for (jdx, inst_tok) in ms.instance_tokens.iter().enumerate() {
                let sub_var = format_ident!("__sub_{}_{}", idx, jdx);
                sub_decls.push(quote! {
                    let mut #sub_var = mmg_microbus::component::__subscribe_exact_auto::<#ty>(&ctx, #inst_tok).await;
                });
                let call_core = if ms.wants_ctx {
                    quote! { this.#method_ident(&ctx, &*env) }
                } else {
                    quote! { this.#method_ident(&*env) }
                };
                let call_expr = match &ms.ret_case {
                    RetCase::Unit => quote! { let _ = #call_core.await; },
                    RetCase::ResultUnit => {
                        quote! { if let Err(e) = #call_core.await { tracing::warn!(error=?e, "handle returned error"); } }
                    }
                    RetCase::Some => quote! { { let __v = #call_core.await; ctx.publish(__v).await; } },
                    RetCase::ResultSome => {
                        quote! { match #call_core.await { Ok(v) => ctx.publish(v).await, Err(e) => tracing::warn!(error=?e, "handle returned error") } }
                    }
                };
                select_arms.push(quote! {
                    msg = #sub_var.recv() => {
                        match msg {
                            Some(env) => {
                                { #call_expr }
                            }
                            None => { break; }
                        }
                    }
                });
            }
        }
    }

    // Generate active loops
    for (idx, a) in actives.iter().enumerate() {
        let method_ident = &a.ident;
        // call core
        let call_core = if a.wants_ctx {
            quote! { this.#method_ident(&ctx) }
        } else {
            quote! { this.#method_ident() }
        };
        let call_expr = match &a.ret_case {
            RetCase::Unit => quote! { let _ = #call_core.await; },
            RetCase::ResultUnit => {
                quote! { if let Err(e) = #call_core.await { tracing::warn!(error=?e, "active returned error"); } }
            }
            RetCase::Some => quote! { { let __v = #call_core.await; ctx.publish(__v).await; } },
            RetCase::ResultSome => {
                quote! { match #call_core.await { Ok(v) => ctx.publish(v).await, Err(e) => tracing::warn!(error=?e, "active returned error") } }
            }
        };
        // declarations: counters and done flags
        let cnt = format_ident!("__active_cnt_{}", idx);
        let done = format_ident!("__active_done_{}", idx);
        active_decls.push(quote! { let mut #cnt: u64 = 0; let mut #done: bool = false; });
        // immediate exec (no `continue` here; guard on config presence)
        if a.immediate || a.times == Some(1) {
            let times_limit = if let Some(n) = a.times {
                quote! { if #cnt >= #n { #done = true; } }
            } else {
                quote! {}
            };
            active_pre_immediate.push(
                quote! { if !#done { { #call_expr } #cnt = #cnt.saturating_add(1); #times_limit } },
            );
        }
        // ticker if interval specified
        if let Some(ms) = a.interval_ms {
            if ms > 0 {
                let tk = format_ident!("__active_ticker_{}", idx);
                active_decls.push(
                    quote! { let mut #tk = ctx.ticker(std::time::Duration::from_millis(#ms)); },
                );
                let times_limit = if let Some(n) = a.times {
                    quote! { if #cnt >= #n { #done = true; } }
                } else {
                    quote! {}
                };
                active_arms.push(quote! {
                    __tick = #tk.tick(), if !#done => {
                        match __tick {
                            None => { break; }
                            Some(()) => {
                                { #call_expr }
                                #cnt = #cnt.saturating_add(1);
                                #times_limit
                            }
                        }
                    }
                });
            }
        } // end if ms
    }

    let gen_run = quote! {
        #[allow(unreachable_code)]
        #[async_trait::async_trait]
    impl mmg_microbus::component::Component for #self_ty {
            fn id(&self) -> &mmg_microbus::bus::ComponentId { &self.id }
            async fn run(self: Box<Self>, mut ctx: mmg_microbus::component::ComponentContext) -> anyhow::Result<()> {
                let mut this = *self;
        // 初始化阶段：调用 #[init] 标注的方法
        #( #init_calls )*
        // 为每个处理方法建立强类型订阅
                #(#sub_decls)*
        // Active counters/tickers
        #(#active_decls)*
        // Immediate active invocations
        #(#active_pre_immediate)*
                // 组件类型 KindId
                let __kind_id = mmg_microbus::bus::KindId::of::<#self_ty>();
                // 主循环：各类型订阅自动随停（无显式 shutdown 分支）
        loop {
                    tokio::select! {
                        #(#select_arms)*
            #(#active_arms)*
            _ = ctx.graceful_sleep(std::time::Duration::from_secs(3600)) => { break; }
                    }
                }
        // 停止阶段：调用 #[stop] 标注的方法
        #( #stop_calls )*
                Ok(())
            }
        }
    };

    let expanded = quote! {
        #item
        #gen_run
        #(#compile_errors)*
    };
    expanded.into()
}

// 已移除 #[handles]：旧的 impl 标注入口不再存在，请使用 #[component] 标注 struct 与 impl。

/// Mark a method as proactive/active loop. Example:
///   #[active(interval_ms=1000)] async fn tick(&self, &Context, &Cfg) -> anyhow::Result<()>
#[proc_macro_attribute]
pub fn active(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}
