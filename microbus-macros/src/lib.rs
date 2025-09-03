use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{parse_macro_input, Attribute, ItemImpl, ItemStruct, LitStr, Type};
use syn::{Ident, Token};

// 仅保留强类型“方法即订阅”路径。

#[proc_macro_attribute]
pub fn component_factory(_args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemImpl);
    let self_ty = &item.self_ty;
    let expanded = quote! {
        #item
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        const _: () = {
            mmg_microbus::inventory::submit! {
                mmg_microbus::registry::FactoryEntry(|| {
                    let f: std::sync::Arc<dyn mmg_microbus::component::ComponentFactory> =
                        std::sync::Arc::new(<#self_ty as Default>::default());
                    f
                })
            }
        };
    };
    expanded.into()
}

#[proc_macro_attribute]
pub fn component(_args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemStruct);
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
        #[allow(non_upper_case_globals)]
        const _: () = {
            #[async_trait::async_trait]
            impl mmg_microbus::component::ComponentFactory for #factory_ident {
                fn kind_id(&self) -> mmg_microbus::bus::KindId { mmg_microbus::bus::KindId::of::<#struct_ident>() }
                fn type_name(&self) -> &'static str { std::any::type_name::<#struct_ident>() }
                async fn build(&self, id: mmg_microbus::bus::ComponentId, _bus: mmg_microbus::bus::BusHandle) -> anyhow::Result<Box<dyn mmg_microbus::component::Component>> { #build_body }
            }
            mmg_microbus::inventory::submit! {
                mmg_microbus::registry::FactoryEntry(|| {
                    let f: std::sync::Arc<dyn mmg_microbus::component::ComponentFactory> = std::sync::Arc::new(#factory_ident::default());
                    f
                })
            }
        };
    };
    expanded.into()
}

// 标注一个实现块为“配置处理函数”，类型参数是配置结构体 T。
// 要求实现 `Configure<T>` trait（在运行库中），宏只做注册工作，使框架可自动发现并在启动/热更时调用。
#[proc_macro_attribute]
pub fn configure(args: TokenStream, input: TokenStream) -> TokenStream {
    let ty: Type = parse_macro_input!(args as Type);
    let item = parse_macro_input!(input as ItemImpl);
    let self_ty = &item.self_ty;
    let expanded = quote! {
        #item
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        const _: () = {
            fn __kind() -> mmg_microbus::bus::KindId { mmg_microbus::bus::KindId::of::<#self_ty>() }
            fn __cfg() -> std::any::TypeId { std::any::TypeId::of::<#ty>() }
            impl mmg_microbus::component::ConfigApplyDyn for #self_ty {
                fn apply<'a>(
                    &'a mut self,
                    ctx: mmg_microbus::component::ConfigContext,
                    v: serde_json::Value,
                ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
                    Box::pin(async move {
                        let cfg: #ty = serde_json::from_value(v)?;
                        <Self as mmg_microbus::component::Configure<#ty>>::on_config(self, &ctx, cfg).await
                    })
                }
            }
            fn __invoke<'a>(
                comp: &'a mut dyn mmg_microbus::component::Component,
                ctx: mmg_microbus::component::ConfigContext,
                v: serde_json::Value,
            ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
                if let Some(c) = comp.as_any_mut().downcast_mut::<#self_ty>() {
                    mmg_microbus::component::ConfigApplyDyn::apply(c, ctx, v)
                } else {
                    Box::pin(async { Ok(()) })
                }
            }
            mmg_microbus::inventory::submit! { mmg_microbus::config_registry::DesiredCfgEntry(mmg_microbus::config_registry::DesiredCfgSpec { component_kind: __kind, cfg_type: __cfg, invoke: __invoke }) }
        };
    };
    expanded.into()
}

// --- 方法即订阅（强类型） ---
// 用法：
//   #[mmg_microbus::handles]
//   impl MyComp {
//       #[mmg_microbus::handle(Tick)]
//       async fn on_tick(&mut self, ctx: &mmg_microbus::component::ComponentContext, env: std::sync::Arc<mmg_microbus::bus::Envelope<Tick>>) -> anyhow::Result<()> {
//           // ... 业务逻辑 ...
//           Ok(())
//       }
//   }
// 要求：结构体已使用 #[component] 注册工厂；无需手写 Component 实现，宏会自动生成 run()，为每个 #[handle(T)] 建立强类型订阅并调度到对应方法。

#[proc_macro_attribute]
pub fn handle(_args: TokenStream, input: TokenStream) -> TokenStream {
    // 标记型属性：保持方法体不变，由 #[handles] 在同一 impl 中解析。
    input
}

#[proc_macro_attribute]
pub fn handles(_args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemImpl);
    let self_ty = &item.self_ty;

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
        // 订阅类型：true 表示 Envelope<T>，false 表示 T
        sub_is_envelope: bool,
        param_is_ref: bool,
        msg_ty: Type,
        wants_ctx: bool,
        wants_scoped_bus: bool,
        pattern_tokens: proc_macro2::TokenStream,
    }

    fn is_ctx_type(ty: &syn::Type) -> (bool, bool) {
        if let syn::Type::Reference(r) = ty {
            if let syn::Type::Path(tp) = &*r.elem {
                let last = tp
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                if last == "ComponentContext" {
                    return (true, false);
                }
                if last == "ScopedBus" {
                    return (false, true);
                }
            }
        }
        (false, false)
    }

    fn parse_msg_arg(ty: &syn::Type) -> Option<(bool /*is_env*/, bool /*by_ref*/, Type)> {
        // 支持 &Envelope<T> / &T / Arc<Envelope<T>> / Arc<T>
        if let syn::Type::Reference(r) = ty {
            if let syn::Type::Path(tp) = &*r.elem {
                if let Some(last) = tp.path.segments.last() {
                    if last.ident == "Envelope" {
                        if let syn::PathArguments::AngleBracketed(env_ab) = &last.arguments {
                            if let Some(syn::GenericArgument::Type(t)) = env_ab.args.first() {
                                return Some((true, true, t.clone()));
                            }
                        }
                    } else {
                        return Some((false, true, Type::Path(tp.clone())));
                    }
                }
            }
        }
        // Arc<...>
        if let syn::Type::Path(tp) = ty {
            for seg in &tp.path.segments {
                if seg.ident == "Arc" {
                    if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
                        if let Some(syn::GenericArgument::Type(syn::Type::Path(inner))) = ab.args.first() {
                                // Arc<...>
                                if let Some(last) = inner.path.segments.last() {
                                    if last.ident == "Envelope" {
                                        if let syn::PathArguments::AngleBracketed(env_ab) =
                                            &last.arguments
                                        {
                                            if let Some(syn::GenericArgument::Type(t)) =
                                                env_ab.args.first()
                                            {
                                                return Some((true, false, t.clone()));
                                            }
                                        }
                                    } else {
                                        // Arc<T>
                                        return Some((false, false, Type::Path(inner.clone())));
                                    }
                                }
                        }
                    }
                }
            }
        }
        None
    }

    let mut methods: Vec<MethodSpec> = Vec::new();
    let mut compile_errors: Vec<proc_macro2::TokenStream> = Vec::new();
    for it in &item.items {
        if let syn::ImplItem::Fn(m) = it {
            // 查找 #[handle(T)] / #[handle(T, from=..., instance=...)]
            let mut attr = HandleAttr::default();
            for a in &m.attrs {
                let last = a
                    .path()
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                if last.as_str() == "handle" {
                    let (m, f, i_str, i_ty) = parse_handle_attr(a);
                    attr.msg_ty = m;
                    attr.from_service = f;
                    attr.instance_str = i_str;
                    attr.instance_ty = i_ty;
                }
            }

            // 解析参数：可选注入 &ComponentContext 或 &ScopedBus；消息参数必须是 Arc<Envelope<T>> 或 Arc<T>
            let mut wants_ctx = false;
            let mut wants_sb = false;
            let mut msg: Option<(bool, bool, Type)> = None;
            for arg in &m.sig.inputs {
                if let syn::FnArg::Typed(pat_ty) = arg {
                    let (is_ctx, is_sb) = is_ctx_type(&pat_ty.ty);
                    if is_ctx {
                        wants_ctx = true;
                        continue;
                    }
                    if is_sb {
                        wants_sb = true;
                        continue;
                    }
                    if msg.is_none() {
                        msg = parse_msg_arg(&pat_ty.ty);
                    }
                }
            }
            // 先用方法签名确定“形态”（Envelope/T），类型可被 #[handle(T)] 覆盖
            let (sub_is_env, param_is_ref, mut msg_ty) =
                if let Some((is_env, by_ref, t)) = msg.clone() {
                    (is_env, by_ref, t)
                } else if let Some(t) = attr.msg_ty.clone() {
                    // 未从签名解析出参数时，默认 Envelope 形态
                    (true, false, t)
                } else {
                    continue; // 无法推断
                };
            if let Some(over) = attr.msg_ty {
                msg_ty = over;
            }

            // 生成过滤表达式
            let pattern_tokens = if let Some(from_ty) = attr.from_service.clone() {
                if let Some(inst_str) = attr.instance_str.clone() {
                    // 明确禁止字符串实例：用带 span 的编译期错误，指向该字符串字面量
                    compile_errors.push(
                        syn::Error::new(
                            inst_str.span(),
                            "#[handle]: `instance` expects a marker type (impl InstanceMarker)",
                        )
                        .to_compile_error(),
                    );
                    quote! { mmg_microbus::bus::ServicePattern::for_kind::<#from_ty>() }
                } else if let Some(inst_ty) = attr.instance_ty.clone() {
                    quote! { mmg_microbus::bus::ServicePattern::for_instance_marker::<#from_ty, #inst_ty>() }
                } else {
                    quote! { mmg_microbus::bus::ServicePattern::for_kind::<#from_ty>() }
                }
            } else if attr.instance_str.is_some() || attr.instance_ty.is_some() {
                // 仅 instance 而缺少 from 类型：给出明确的编译期错误
                compile_errors.push(quote! { compile_error!("#[handle]: `instance = ...` requires also specifying `from = ServiceType`"); });
                quote! { mmg_microbus::bus::ServicePattern::any() }
            } else {
                quote! { mmg_microbus::bus::ServicePattern::any() }
            };

            methods.push(MethodSpec {
                ident: m.sig.ident.clone(),
                sub_is_envelope: sub_is_env,
                param_is_ref,
                msg_ty,
                wants_ctx,
                wants_scoped_bus: wants_sb,
                pattern_tokens,
            });
        }
    }

    // 生成 run()：为每个 handler 创建订阅，并在 select 循环中分发；同时监听配置热更新
    let mut sub_decls = Vec::new();
    let mut select_arms = Vec::new();
    for (idx, ms) in methods.iter().enumerate() {
        let sub_var = format_ident!("__sub_{}", idx);
        let ty = &ms.msg_ty;
        let method_ident = &ms.ident;
        let pattern = &ms.pattern_tokens;
        if ms.sub_is_envelope {
            sub_decls.push(quote! {
                let mut #sub_var = ctx
                    .subscribe_pattern::<mmg_microbus::bus::Envelope<#ty>>(#pattern)
                    .await;
            });
            let arg_env = if ms.param_is_ref {
                quote! { &*env }
            } else {
                quote! { env }
            };
            let call = if ms.wants_ctx && ms.wants_scoped_bus {
                quote! { this.#method_ident(&ctx, &ctx.scoped_bus, #arg_env).await }
            } else if ms.wants_ctx {
                quote! { this.#method_ident(&ctx, #arg_env).await }
            } else if ms.wants_scoped_bus {
                quote! { this.#method_ident(&ctx.scoped_bus, #arg_env).await }
            } else {
                quote! { this.#method_ident(#arg_env).await }
            };
            select_arms.push(quote! {
                Some(env) = #sub_var.recv() => {
                    if let Err(e) = #call { tracing::warn!(method = stringify!(#method_ident), error = ?e, "handler returned error"); }
                }
            });
        } else {
            sub_decls.push(quote! {
                let mut #sub_var = ctx
                    .subscribe_pattern::<#ty>(#pattern)
                    .await;
            });
            let arg_env = if ms.param_is_ref {
                quote! { &*env }
            } else {
                quote! { env }
            };
            let call = if ms.wants_ctx && ms.wants_scoped_bus {
                quote! { this.#method_ident(&ctx, &ctx.scoped_bus, #arg_env).await }
            } else if ms.wants_ctx {
                quote! { this.#method_ident(&ctx, #arg_env).await }
            } else if ms.wants_scoped_bus {
                quote! { this.#method_ident(&ctx.scoped_bus, #arg_env).await }
            } else {
                quote! { this.#method_ident(#arg_env).await }
            };
            select_arms.push(quote! {
                Some(env) = #sub_var.recv() => {
                    if let Err(e) = #call { tracing::warn!(method = stringify!(#method_ident), error = ?e, "handler returned error"); }
                }
            });
        }
    }

    let gen_run = quote! {
        #[async_trait::async_trait]
        impl mmg_microbus::component::Component for #self_ty {
            fn id(&self) -> &mmg_microbus::bus::ComponentId { &self.id }
            async fn run(self: Box<Self>, mut ctx: mmg_microbus::component::ComponentContext) -> anyhow::Result<()> {
                let mut this = *self;
                // 为每个处理方法建立强类型订阅
                #(#sub_decls)*
                // 组件类型 KindId
                let __kind_id = mmg_microbus::bus::KindId::of::<#self_ty>();
                // 主循环：监听关停、配置热更与各类型订阅
                loop {
                    tokio::select! {
                        changed = ctx.shutdown.changed() => {
                            if changed.is_ok() { break; } else { break; }
                        }
                        // 配置热更：将 JSON 广播派发给该组件类型已注册的 #[configure] 处理器
            cfg_changed = ctx.config_rx.changed() => {
                            if cfg_changed.is_ok() {
                                let v = ctx.current_config_json();
                                let cfg_ctx = mmg_microbus::component::ConfigContext::from_component_ctx(&ctx);
                for ce in mmg_microbus::inventory::iter::<mmg_microbus::config_registry::DesiredCfgEntry> {
                                    if (ce.0.component_kind)() == __kind_id {
                                        let _ = (ce.0.invoke)(&mut this, cfg_ctx.clone(), v.clone()).await;
                                    }
                                }
                            }
                        }
                        #(#select_arms)*
                    }
                }
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
