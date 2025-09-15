use proc_macro::TokenStream;
use quote::{format_ident, quote};
// removed unused Parse traits after simplifying active parsing
use syn::Ident;
use syn::{parse_macro_input, Attribute, Item, ItemImpl, ItemStruct, Type};

// 仅保留强类型“方法即订阅”路径。

// 组件工厂通过 #[component] 结构体注解自动生成（单一构造路径）

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
    let factory_ident = format_ident!("__{}Factory", struct_ident);
    let build_body = quote! { Ok(Box::new(<#struct_ident as Default>::default())) };
    let expanded = quote! {
        #item
        #[doc(hidden)]
        #[derive(Default)]
        struct #factory_ident;
        #[async_trait::async_trait]
        impl mmg_microbus::component::ComponentFactory for #factory_ident {
            fn type_name(&self) -> &'static str { std::any::type_name::<#struct_ident>() }
            async fn build(&self, _bus: mmg_microbus::bus::BusHandle) -> mmg_microbus::error::Result<Box<dyn mmg_microbus::component::Component>> { #build_body }
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

// 配置注入：在 #[init] 方法签名中以 &CfgType 参数体现

// --- 方法即订阅（强类型，&T-only） + 生命周期钩子 ---
// 用法（新）：
//   #[mmg_microbus::component]
//   impl MyComp {
//       #[mmg_microbus::handle]
//       async fn on_tick(&mut self, ctx: &mmg_microbus::component::ComponentContext, tick: &Tick) -> mmg_microbus::error::Result<()> { Ok(()) }
//   }
// 说明：仅支持签名 (&ComponentContext, &T)，必须显式标注 #[handle]

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
    // 解析 #[handle]：不允许任何参数
    #[derive(Default, Clone)]
    struct HandleAttr {
        has_args: bool,
    }
    fn parse_handle_attr(a: &Attribute) -> HandleAttr {
        // 若存在任何 token，标记为非法
        let has = a.meta.require_path_only().is_err();
        HandleAttr { has_args: has }
    }
    fn get_param_ident(p: &syn::Pat) -> Option<Ident> {
        if let syn::Pat::Ident(pi) = p {
            Some(pi.ident.clone())
        } else {
            None
        }
    }

    // 收集处理方法规范
    struct MethodSpec {
        ident: syn::Ident,
        msg_ty: Type,
        wants_ctx: bool,
        ret_case: RetCase,
    }
    #[derive(Clone)]
    enum RetCase {
        Unit,
        Some,
        OptionSome,
        ResultUnit,
        ResultSome,
        ResultOption,
    }
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
                                    // Result<Option<T>, E>
                                    if let syn::Type::Path(ok_tp) = ok_ty {
                                        let ok_last = ok_tp
                                            .path
                                            .segments
                                            .last()
                                            .map(|s| s.ident.to_string())
                                            .unwrap_or_default();
                                        if ok_last == "Option" {
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

    let mut methods: Vec<MethodSpec> = Vec::new();
    // Active methods
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ActiveKind {
        Loop,
        Once,
    }
    #[derive(Clone)]
    struct ActiveSpec {
        ident: syn::Ident,
        wants_ctx: bool,
        ret_case: RetCase,
        kind: ActiveKind,
    }
    // 解析 #[active]：允许空（Loop）或 (once)
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
                // #[active(...)] 仅允许 once
                if list.tokens.is_empty() {
                    return Some(Ok(ActiveKind::Loop));
                }
                let content = list.tokens.to_string();
                let trimmed = content.trim();
                if trimmed == "once" {
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
    let mut actives: Vec<ActiveSpec> = Vec::new();
    let mut compile_errors: Vec<proc_macro2::TokenStream> = Vec::new();
    for it in &item.items {
        if let syn::ImplItem::Fn(m) = it {
            // 处理 #[handle]
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
                    let parsed = parse_handle_attr(a);
                    if parsed.has_args {
                        compile_errors.push(quote!{ compile_error!("#[handle] does not accept any arguments in this model"); });
                    }
                }
            }
            if handle_attr_count > 1 {
                compile_errors.push(quote! { compile_error!("a method can only have one #[handle(...)] attribute"); });
            }
            if has_handle_attr {
                // 放宽规则：允许可选 ctx；默认要求且仅允许一个业务 &T 参数。
                let mut wants_ctx = false;
                let mut candidates: Vec<(Option<Ident>, Type)> = Vec::new();
                for arg in &m.sig.inputs {
                    if let syn::FnArg::Typed(pat_ty) = arg {
                        if is_ctx_type(&pat_ty.ty) {
                            wants_ctx = true;
                            continue;
                        }
                        if let Some(t) = parse_msg_arg_ref(&pat_ty.ty) {
                            let name = get_param_ident(&pat_ty.pat);
                            candidates.push((name, t));
                        } else {
                            // 不支持其他参数（避免侵入）。
                        }
                    }
                }
                let chosen_msg: Option<Type> = {
                    if candidates.len() == 1 {
                        Some(candidates[0].1.clone())
                    } else if candidates.is_empty() {
                        compile_errors.push(quote! { compile_error!("#[handle] requires exactly one &T parameter (message payload)") });
                        None
                    } else {
                        compile_errors.push(quote! { compile_error!("#[handle] allows only one &T parameter; remove extras") });
                        None
                    }
                };
                if let Some(msg_ty) = chosen_msg {
                    methods.push(MethodSpec {
                        ident: m.sig.ident.clone(),
                        msg_ty,
                        wants_ctx,
                        ret_case: analyze_return(&m.sig),
                    });
                }
            } // no else: 未标注 #[handle] 的方法视为普通方法

            // 处理 #[active]
            let mut is_active = false;
            let mut active_kind: Option<ActiveKind> = None;
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
            let kind = active_kind.unwrap_or(ActiveKind::Loop);
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
                kind,
            });
        }
    }

    // 生成 run()：为每个 handle 创建订阅，并在 select 循环中分发
    // 收集 #[init] 与 #[stop]
    // #[init]: 允许 self + 可选 &ComponentContext + 恰好一个 &CfgType
    struct InitSpec {
        ident: syn::Ident,
        wants_ctx: bool,
        cfg_ty: Option<Type>,
        ret_case: RetCase,
    }
    let mut inits: Vec<InitSpec> = Vec::new();
    // #[stop]: 允许 self + 可选 &ComponentContext
    struct StopSpec {
        ident: syn::Ident,
        wants_ctx: bool,
        ret_case: RetCase,
    }
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
                if last.as_str() == "init" {
                    has_init = true;
                }
                if last.as_str() == "stop" {
                    has_stop = true;
                }
            }
            if has_init || has_stop {
                // 收集 #[init] 的参数（严格：恰好一个 &CfgType；允许可选 ctx）
                if has_init {
                    let mut cfg_ty: Option<Type> = None;
                    let mut param_count = 0usize;
                    let mut wants_ctx = false;
                    let mut errors = Vec::new();
                    for arg in &m.sig.inputs {
                        if let syn::FnArg::Typed(pat_ty) = arg {
                            if is_ctx_type(&pat_ty.ty) {
                                wants_ctx = true;
                                continue;
                            }
                            if let Some(t) = parse_msg_arg_ref(&pat_ty.ty) {
                                param_count += 1;
                                if cfg_ty.is_none() {
                                    cfg_ty = Some(t);
                                } else { /* duplicated; counted */
                                }
                            } else {
                                errors.push(syn::Error::new_spanned(&m.sig, "#[init] parameter must be a single by-reference config type: &Cfg").to_compile_error());
                            }
                        }
                    }
                    if param_count != 1 {
                        errors.push(
                            syn::Error::new_spanned(
                                &m.sig,
                                "#[init] requires exactly one &Cfg parameter",
                            )
                            .to_compile_error(),
                        );
                    }
                    if !errors.is_empty() {
                        compile_errors.extend(errors);
                    }
                    let rc = analyze_return(&m.sig);
                    inits.push(InitSpec {
                        ident: m.sig.ident.clone(),
                        wants_ctx,
                        cfg_ty: cfg_ty.clone(),
                        ret_case: rc,
                    });
                }
                // 校验 #[stop]：只允许接收器（&self 或 &mut self），不允许其他参数
                if has_stop {
                    let mut extraneous = Vec::new();
                    let mut wants_ctx = false;
                    for arg in &m.sig.inputs {
                        if let syn::FnArg::Typed(p) = arg {
                            if is_ctx_type(&p.ty) {
                                wants_ctx = true;
                            } else {
                                extraneous.push(p.ty.clone());
                            }
                        }
                    }
                    if !extraneous.is_empty() {
                        let err = syn::Error::new_spanned(
                            &m.sig,
                            "#[stop] method must take only self or optionally &self plus &ComponentContext",
                        );
                        compile_errors.push(err.to_compile_error());
                    } else {
                        let rc = analyze_return(&m.sig);
                        stops.push(StopSpec {
                            ident: m.sig.ident.clone(),
                            wants_ctx,
                            ret_case: rc,
                        });
                    }
                }
            }
        }
    }

    // 生成 init/stop 调用代码
    let mut init_calls = Vec::new();
    for i in inits.iter() {
        let ident = &i.ident;
        let call_expr = if let Some(cty) = &i.cfg_ty {
            let var = format_ident!("__icfg_{}", ident);
            // 严格：恰好一个 &Cfg 参数，且可选 ctx
            let call_core = if i.wants_ctx {
                quote! { this.#ident(&ctx, &*#var) }
            } else {
                quote! { this.#ident(&*#var) }
            };
            let rc = &i.ret_case;
            let ret = match rc {
                RetCase::Unit => quote! { let _ = #call_core.await; },
                RetCase::ResultUnit => {
                    quote! { if let Err(e) = #call_core.await { tracing::warn!(error=?e, "init returned error"); } }
                }
                RetCase::Some => {
                    quote! { { let __v = #call_core.await; mmg_microbus::component::__publish_auto(&ctx, __v).await; } }
                }
                RetCase::OptionSome => {
                    quote! { { if let Some(__v) = #call_core.await { mmg_microbus::component::__publish_auto(&ctx, __v).await; } } }
                }
                RetCase::ResultSome => {
                    quote! { match #call_core.await { Ok(v) => mmg_microbus::component::__publish_auto(&ctx, v).await, Err(e) => tracing::warn!(error=?e, "init returned error") } }
                }
                RetCase::ResultOption => {
                    quote! { match #call_core.await { Ok(opt) => if let Some(v) = opt { mmg_microbus::component::__publish_auto(&ctx, v).await }, Err(e) => tracing::warn!(error=?e, "init returned error") } }
                }
            };
            quote! {{
                let #var = mmg_microbus::component::__get_config::<#cty>(&ctx);
                if let Some(#var) = #var { #ret } else { return Err(mmg_microbus::error::MicrobusError::MissingConfig(stringify!(#cty))); }
            }}
        } else {
            // 不应出现：#[init] 必须提供一个 &CfgType；若宏到此仍无类型，静默不生成调用。
            quote! {}
        };
        init_calls.push(call_expr);
    }
    let mut stop_calls = Vec::new();
    for s in stops.iter() {
        let ident = &s.ident;
        let call_core = if s.wants_ctx {
            quote! { this.#ident(&ctx) }
        } else {
            quote! { this.#ident() }
        };
        let rc = &s.ret_case;
        let call_expr = match rc {
            RetCase::Unit => quote! { let _ = #call_core.await; },
            RetCase::ResultUnit => {
                quote! { if let Err(e) = #call_core.await { tracing::warn!(error=?e, "stop returned error"); } }
            }
            RetCase::Some => {
                quote! { { let __v = #call_core.await; mmg_microbus::component::__publish_auto(&ctx, __v).await; } }
            }
            RetCase::OptionSome => {
                quote! { { if let Some(__v) = #call_core.await { mmg_microbus::component::__publish_auto(&ctx, __v).await; } } }
            }
            RetCase::ResultSome => {
                quote! { match #call_core.await { Ok(v) => mmg_microbus::component::__publish_auto(&ctx, v).await, Err(e) => tracing::warn!(error=?e, "stop returned error") } }
            }
            RetCase::ResultOption => {
                quote! { match #call_core.await { Ok(opt) => if let Some(v) = opt { mmg_microbus::component::__publish_auto(&ctx, v).await }, Err(e) => tracing::warn!(error=?e, "stop returned error") } }
            }
        };
        stop_calls.push(quote! {{ #call_expr }});
    }
    let mut sub_decls = Vec::new();
    let mut select_arms = Vec::new();
    // prepare active tickers and state
    let mut active_arms = Vec::new();
    for (idx, ms) in methods.iter().enumerate() {
        let ty = &ms.msg_ty;
        let method_ident = &ms.ident;
        {
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
                RetCase::Some => {
                    quote! { { let __v = #call_core.await; mmg_microbus::component::__publish_auto(&ctx, __v).await; } }
                }
                RetCase::OptionSome => {
                    quote! { { if let Some(__v) = #call_core.await { mmg_microbus::component::__publish_auto(&ctx, __v).await; } } }
                }
                RetCase::ResultSome => {
                    quote! { match #call_core.await { Ok(v) => mmg_microbus::component::__publish_auto(&ctx, v).await, Err(e) => tracing::warn!(error=?e, "handle returned error") } }
                }
                RetCase::ResultOption => {
                    quote! { match #call_core.await { Ok(opt) => if let Some(v) = opt { mmg_microbus::component::__publish_auto(&ctx, v).await }, Err(e) => tracing::warn!(error=?e, "handle returned error") } }
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

    // Active 调度： once 类直接在循环前调用； loop 类合并为单一 select 分支（yield 驱动公平）
    let mut once_calls = Vec::new();
    let mut loop_call_bodies = Vec::new();
    for a in actives.iter() {
        let method_ident = &a.ident;
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
            RetCase::Some => {
                quote! { { let __v = #call_core.await; mmg_microbus::component::__publish_auto(&ctx, __v).await; } }
            }
            RetCase::OptionSome => {
                quote! { { if let Some(__v) = #call_core.await { mmg_microbus::component::__publish_auto(&ctx, __v).await; } } }
            }
            RetCase::ResultSome => {
                quote! { match #call_core.await { Ok(v) => mmg_microbus::component::__publish_auto(&ctx, v).await, Err(e) => tracing::warn!(error=?e, "active returned error") } }
            }
            RetCase::ResultOption => {
                quote! { match #call_core.await { Ok(opt) => if let Some(v) = opt { mmg_microbus::component::__publish_auto(&ctx, v).await }, Err(e) => tracing::warn!(error=?e, "active returned error") } }
            }
        };
        if a.kind == ActiveKind::Once {
            once_calls.push(call_expr);
        } else {
            loop_call_bodies.push(call_expr);
        }
    }
    if !loop_call_bodies.is_empty() {
        // 永远就绪的分支：若前面消息分支均未就绪，则立即执行全部 loop-active；实现“尽可能快”语义
        active_arms.push(quote! { _ = async {} => { #( #loop_call_bodies )* } });
    }

    let gen_run = quote! {
        #[allow(unreachable_code)]
        #[async_trait::async_trait]
    impl mmg_microbus::component::Component for #self_ty {
            async fn run(self: Box<Self>, mut ctx: mmg_microbus::component::ComponentContext) -> mmg_microbus::error::Result<()> {
                let mut this = *self;
        // 初始化阶段：调用 #[init] 标注的方法
        #( #init_calls )*
    // 为每个处理方法建立强类型订阅
                #(#sub_decls)*
    // once-active 延迟到主循环首轮内执行（避免在其他组件尚未订阅前发布）
    let mut __once_done = false;
    // Active loop 分支：无调度间隔；无消息就绪时立即执行（最小延迟）
                // 主循环：选择消息、主动任务与框架停机信号；收到停机信号即退出。
                // 单次让出，保证其它组件已进入 run 并完成自身订阅
                tokio::task::yield_now().await;
                loop {
                    if !__once_done { { #( #once_calls )* } __once_done = true; }
                    tokio::select! {
                        #(#select_arms)*
                        #(#active_arms)*
                        _ = mmg_microbus::component::__recv_stop(&ctx) => { break; }
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

// 统一入口：使用 #[component] 标注 struct 与 impl

/// Mark a method as proactive/active loop. Example:
///   #[active] async fn tick(&self) -> mmg_microbus::error::Result<()> // loop
///   #[active(once)] async fn init_once(&self) -> Option<Message>      // one-shot
#[proc_macro_attribute]
pub fn active(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}
