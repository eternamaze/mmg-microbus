use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::Ident;
use syn::{
    parse_macro_input, Attribute, Item, ItemImpl, ItemStruct, LitStr, Token,
    Type,
};

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
    // Check fields; require `id` field; optionally `cfg`
    let mut has_id = false;
    let mut has_cfg = false;
    if let syn::Fields::Named(fields) = &item.fields {
        for f in &fields.named {
            if let Some(id) = &f.ident {
                if id == "id" {
                    has_id = true;
                }
            }
            if let Some(id) = &f.ident {
                if id == "cfg" {
                    has_cfg = true;
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
    let build_body = if has_cfg {
        quote! { Ok(Box::new(#struct_ident { id, cfg: Default::default() })) }
    } else {
        quote! { Ok(Box::new(#struct_ident { id })) }
    };
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

// 移除旧的 #[configure] 宏：配置改为在 handler 签名中以 &CfgType 参数注入

// --- 方法即订阅（强类型，&T-only） ---
// 用法：
//   #[mmg_microbus::handles]
//   impl MyComp {
//       #[mmg_microbus::handle(Tick)]
//       async fn on_tick(&mut self, tick: &Tick) -> anyhow::Result<()> { Ok(()) }
//   }
// 说明：仅支持消息参数为 `&T`，可选注入 `&ComponentContext`。不再支持 Envelope 或 ScopedBus 注入。

#[proc_macro_attribute]
pub fn handle(_args: TokenStream, input: TokenStream) -> TokenStream {
    // 标记型属性：保持方法体不变，由 #[handles] 在同一 impl 中解析。
    input
}

fn generate_run_impl_inner(item: ItemImpl, self_ty: &syn::Type) -> TokenStream {
    // 参数解析器：
    // - #[handle(T)] 或 #[handle(T, from=ServiceType, instance=MarkerType)]
    #[derive(Default, Clone)]
    struct HandleAttr {
        msg_ty: Option<Type>,
        from_service: Option<Type>,
        instance_str: Option<LitStr>,
        instance_ty: Option<Type>,
    }
    struct HandleArgs {
        msg_ty: Option<Type>,
        from_service: Option<Type>,
        instance_str: Option<LitStr>,
        instance_ty: Option<Type>,
    }
    impl Parse for HandleArgs {
        fn parse(input: ParseStream) -> syn::Result<Self> {
            let mut msg_ty: Option<Type> = None;
            let mut from_service: Option<Type> = None;
            let mut instance_str: Option<LitStr> = None;
            let mut instance_ty: Option<Type> = None;

            // 尝试解析首个 Type（如果不是命名键值）
            if !input.is_empty() {
                let look_is_key = input.peek(Ident) && input.peek2(Token![=]);
                if !look_is_key {
                    msg_ty = Some(input.parse::<Type>()?);
                    if input.peek(Token![,]) {
                        let _c: Token![,] = input.parse()?;
                    }
                }
            }
            // 解析其后的逗号分隔命名参数
            while !input.is_empty() {
                if input.peek(Token![,]) {
                    let _c: Token![,] = input.parse()?;
                    if input.is_empty() {
                        break;
                    }
                }
                let key: Ident = input.parse()?;
                let _eq: Token![=] = input.parse()?;
                if key == "from" {
                    from_service = Some(input.parse::<Type>()?);
                } else if key == "instance" {
                    // 允许字符串或类型标记
                    if input.peek(LitStr) {
                        instance_str = Some(input.parse::<LitStr>()?)
                    } else {
                        instance_ty = Some(input.parse::<Type>()?)
                    }
                } else {
                    return Err(syn::Error::new(key.span(), "unknown key in #[handle(..)]"));
                }
                if input.peek(Token![,]) {
                    let _c: Token![,] = input.parse()?;
                }
            }
            Ok(HandleArgs {
                msg_ty,
                from_service,
                instance_str,
                instance_ty,
            })
        }
    }
    fn parse_handle_attr(
        a: &Attribute,
    ) -> (Option<Type>, Option<Type>, Option<LitStr>, Option<Type>) {
        match a.parse_args::<HandleArgs>() {
            Ok(h) => (h.msg_ty, h.from_service, h.instance_str, h.instance_ty),
            Err(_) => (None, None, None, None),
        }
    }

    // 收集处理方法规范
    struct MethodSpec {
        ident: syn::Ident,
        msg_ty: Type,
        wants_ctx: bool,
        cfg_param_tys: Vec<Type>,
        pattern_tokens: proc_macro2::TokenStream,
        ret_case: RetCase,
        from_kind: Option<Type>,
        instance_specified: bool,
    }

    #[derive(Clone)]
    enum RetCase {
        Unit,
        ResultUnit,
        Some,
        ResultSome,
    }

    fn analyze_return(sig: &syn::Signature) -> RetCase {
        match &sig.output {
            syn::ReturnType::Default => RetCase::Unit,
            syn::ReturnType::Type(_, ty) => {
                match &**ty {
                    syn::Type::Tuple(t) if t.elems.is_empty() => RetCase::Unit,
                    syn::Type::Path(tp) => {
                        let last = tp
                            .path
                            .segments
                            .last()
                            .map(|s| s.ident.to_string())
                            .unwrap_or_default();
                        if last == "Result" {
                            // Extract first generic arg as Ok type
                            if let Some(seg) = tp.path.segments.last() {
                                if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
                                    if let Some(syn::GenericArgument::Type(ok_ty)) = ab.args.first()
                                    {
                                        if let syn::Type::Tuple(t) = ok_ty {
                                            if t.elems.is_empty() {
                                                return RetCase::ResultUnit;
                                            }
                                        }
                                        return RetCase::ResultSome;
                                    }
                                }
                            }
                            RetCase::ResultUnit
                        } else {
                            RetCase::Some
                        }
                    }
                    _ => RetCase::Some,
                }
            }
        }
    }

    fn is_ctx_type(ty: &syn::Type) -> bool {
        if let syn::Type::Reference(r) = ty {
            if let syn::Type::Path(tp) = &*r.elem {
                let last = tp
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                if last == "ComponentContext" {
                    return true;
                }
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
    #[derive(Clone)]
    struct ActiveSpec {
        ident: syn::Ident,
        wants_ctx: bool,
        cfg_param_tys: Vec<Type>,
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
            for a in &m.attrs {
                let last = a
                    .path()
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                if last.as_str() == "handle" {
                    has_handle_attr = true;
                    let (m, f, i_str, i_ty) = parse_handle_attr(a);
                    attr.msg_ty = m;
                    attr.from_service = f;
                    attr.instance_str = i_str;
                    attr.instance_ty = i_ty;
                }
            }
            if has_handle_attr {
                // 解析参数：可选注入 &ComponentContext；消息参数必须是 &T；其余 &CfgType 作为配置注入
                let mut wants_ctx = false;
                let mut msg: Option<Type> = None;
                let mut cfg_param_tys: Vec<Type> = Vec::new();
                for arg in &m.sig.inputs {
                    if let syn::FnArg::Typed(pat_ty) = arg {
                        if is_ctx_type(&pat_ty.ty) {
                            wants_ctx = true;
                            continue;
                        }
                        if msg.is_none() {
                            msg = parse_msg_arg_ref(&pat_ty.ty);
                            if msg.is_some() {
                                continue;
                            }
                        }
                        if let Some(t) = parse_msg_arg_ref(&pat_ty.ty) {
                            cfg_param_tys.push(t);
                        }
                    }
                }
                // 确定消息类型：优先从 &T 形参推断，否则从 #[handle(T)] 提供
                let msg_ty = if let Some(t) = msg.clone() {
                    t
                } else if let Some(t) = attr.msg_ty.clone() {
                    t
                } else {
                    continue;
                };

                // 生成过滤表达式
                let mut instance_specified = false;
                let pattern_tokens = if let Some(from_ty) = attr.from_service.clone() {
                    if let Some(_inst_str) = attr.instance_str.clone() {
                        compile_errors.push(quote! { compile_error!("#[handle]: `instance` expects a marker type (impl InstanceMarker)"); });
                        instance_specified = true;
                        quote! { mmg_microbus::bus::Address::any() }
                    } else if let Some(inst_ty) = attr.instance_ty.clone() {
                        instance_specified = true;
                        quote! { mmg_microbus::bus::Address::of_instance::<#from_ty, #inst_ty>() }
                    } else {
                        quote! { mmg_microbus::bus::Address::for_kind::<#from_ty>() }
                    }
                } else if attr.instance_str.is_some() || attr.instance_ty.is_some() {
                    // 仅 instance 而缺少 from 类型：给出明确的编译期错误
                    compile_errors.push(quote! { compile_error!("#[handle]: `instance = ...` requires also specifying `from = ServiceType`"); });
                    quote! { mmg_microbus::bus::Address::any() }
                } else {
                    quote! { mmg_microbus::bus::Address::any() }
                };

                methods.push(MethodSpec {
                    ident: m.sig.ident.clone(),
                    msg_ty,
                    wants_ctx,
                    cfg_param_tys,
                    pattern_tokens,
                    ret_case: analyze_return(&m.sig),
                    from_kind: attr.from_service,
                    instance_specified,
                });
            }

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
            let mut cfg_param_tys: Vec<Type> = Vec::new();
            for arg in &m.sig.inputs {
                match arg {
                    syn::FnArg::Receiver(_) => {}
                    syn::FnArg::Typed(p) => {
                        if is_ctx_type(&p.ty) {
                            wants_ctx = true;
                        } else if let Some(t) = parse_msg_arg_ref(&p.ty) {
                            cfg_param_tys.push(t);
                        }
                    }
                }
            }
            let args_parsed = args_parsed.unwrap_or_default();
            actives.push(ActiveSpec {
                ident: m.sig.ident.clone(),
                wants_ctx,
                cfg_param_tys,
                ret_case: analyze_return(&m.sig),
                interval_ms: args_parsed.interval_ms,
                times: args_parsed.times,
                immediate: args_parsed.immediate,
            });
        }
    }

    // 生成 run()：为每个 handler 创建订阅，并在 select 循环中分发
    let mut sub_decls = Vec::new();
    let mut select_arms = Vec::new();
    // prepare active tickers and state
    let mut active_decls = Vec::new();
    let mut active_arms = Vec::new();
    let mut active_pre_immediate = Vec::new();
    for (idx, ms) in methods.iter().enumerate() {
        let sub_var = format_ident!("__sub_{}", idx);
        let ty = &ms.msg_ty;
        let method_ident = &ms.ident;
        let pattern = &ms.pattern_tokens;
        sub_decls.push(quote! {
            let mut #sub_var = mmg_microbus::component::__subscribe_pattern_auto::<#ty>(&ctx, #pattern).await;
        });
        // 生成配置注入绑定与调用表达式
        let mut __cfg_bind = Vec::new();
        let mut __cfg_args = Vec::new();
        for (ci, cty) in ms.cfg_param_tys.iter().enumerate() {
            let var = format_ident!("__cfg_{}_{}", idx, ci);
            __cfg_bind.push(quote! {
            let #var = match ctx.config::<#cty>() { Some(v) => v, None => {
                    let __ty: &str = std::any::type_name::<#cty>();
                    tracing::error!("missing config: {}", __ty);
                    continue; } };
                    });
            __cfg_args.push(quote! { &*#var });
        }
        let call_core = if ms.wants_ctx {
            quote! { this.#method_ident(&ctx, &*env, #(#__cfg_args),*) }
        } else {
            quote! { this.#method_ident(&*env, #(#__cfg_args),*) }
        };
        // 根据返回类型自动处理发布：T / Result<T> 自动 publish；() / Result<()> 仅记录错误
        let call_expr = match &ms.ret_case {
            RetCase::Unit => quote! { let _ = #call_core.await; },
            RetCase::ResultUnit => {
                quote! { if let Err(e) = #call_core.await { tracing::warn!(error=?e, "handler returned error"); } }
            }
            RetCase::Some => quote! { { let __v = #call_core.await; ctx.publish(__v).await; } },
            RetCase::ResultSome => {
                quote! { match #call_core.await { Ok(v) => ctx.publish(v).await, Err(e) => tracing::warn!(error=?e, "handler returned error") } }
            }
        };
        select_arms.push(quote! {
            msg = #sub_var.recv() => {
                match msg {
                    Some(env) => {
                        #(#__cfg_bind)*
                        { #call_expr }
                    }
                    None => { break; }
                }
            }
        });
    }

    // Generate active handlers
    for (idx, a) in actives.iter().enumerate() {
        let method_ident = &a.ident;
        // cfg binds for active
        let mut __cfg_bind = Vec::new();
        let mut __cfg_args = Vec::new();
        for (ci, cty) in a.cfg_param_tys.iter().enumerate() {
            let var = format_ident!("__cfg_a_{}_{}", idx, ci);
            __cfg_bind.push(quote! {
                let #var = match ctx.config::<#cty>() { Some(v) => v, None => { let __ty: &str = std::any::type_name::<#cty>(); tracing::error!("missing config: {}", __ty); continue; } };
            });
            __cfg_args.push(quote! { &*#var });
        }
        // call core
        let call_core = if a.wants_ctx {
            quote! { this.#method_ident(&ctx, #(#__cfg_args),*) }
        } else {
            quote! { this.#method_ident( #(#__cfg_args),* ) }
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
            if a.cfg_param_tys.is_empty() {
                active_pre_immediate.push(quote! { if !#done { { #call_expr } #cnt = #cnt.saturating_add(1); #times_limit } });
            } else {
                // Build Option binds and guard
                let mut opt_binds = Vec::new();
                let mut is_none_logs = Vec::new();
                let mut unwraps = Vec::new();
                for (ci, cty) in a.cfg_param_tys.iter().enumerate() {
                    let var = format_ident!("__cfg_a_{}_{}", idx, ci);
                    opt_binds.push(quote! { let #var = ctx.config::<#cty>(); });
                    is_none_logs.push(quote! { if #var.is_none() { let __ty: &str = std::any::type_name::<#cty>(); tracing::error!("missing config: {}", __ty); } });
                    unwraps.push(quote! { let #var = #var.unwrap(); });
                }
                let mut all_some = proc_macro2::TokenStream::new();
                for (ci, _cty) in a.cfg_param_tys.iter().enumerate() {
                    let var = format_ident!("__cfg_a_{}_{}", idx, ci);
                    let check = quote! { #var.is_some() };
                    if all_some.is_empty() {
                        all_some = check;
                    } else {
                        all_some = quote! { #all_some && #check };
                    }
                }
                active_pre_immediate.push(quote! {
                    {
                        #( #opt_binds )*
                        if !( #all_some ) {
                            #( #is_none_logs )*
                        } else if !#done {
                            #( #unwraps )*
                            { #call_expr }
                            #cnt = #cnt.saturating_add(1);
                            #times_limit
                        }
                    }
                });
            }
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
                                #(#__cfg_bind)*
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
            _ = ctx.graceful_sleep(std::time::Duration::from_secs(3600)) => { /* idle until shutdown */ }
                    }
                }
                Ok(())
            }
        }
    };

    // Emit route constraints for from=Type without instance
    let mut route_constraints = Vec::new();
    for ms in methods.iter() {
        if let Some(ref fk) = ms.from_kind {
            if !ms.instance_specified {
                route_constraints.push(quote! {
                    mmg_microbus::inventory::submit! { mmg_microbus::registry::RouteConstraint {
                        consumer_ty: || std::any::type_name::<#self_ty>(),
                        consumer_kind: || mmg_microbus::bus::KindId::of::<#self_ty>(),
                        from_kind: || mmg_microbus::bus::KindId::of::<#fk>(),
                    }}
                });
            }
        }
    }
    let expanded = quote! {
        #item
        #gen_run
        #(#route_constraints)*
        #(#compile_errors)*
    };
    expanded.into()
}

#[proc_macro_attribute]
pub fn handles(_args: TokenStream, _input: TokenStream) -> TokenStream {
    syn::Error::new(
        proc_macro2::Span::call_site(),
        "`#[handles]` has been replaced by `#[component]` on impl blocks",
    )
    .to_compile_error()
    .into()
}

/// Mark a method as proactive/active loop. Example:
///   #[active(interval_ms=1000)] async fn tick(&self, &Context, &Cfg) -> anyhow::Result<()>
#[proc_macro_attribute]
pub fn active(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}
