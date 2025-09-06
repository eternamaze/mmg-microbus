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
        msg_ty: Type,
        wants_ctx: bool,
        cfg_param_tys: Vec<Type>,
        pattern_tokens: proc_macro2::TokenStream,
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
                        if msg.is_some() { continue; }
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
                    quote! { mmg_microbus::bus::Address::for_kind::<#from_ty>() }
                } else if let Some(inst_ty) = attr.instance_ty.clone() {
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
            });
        }
    }

    // 生成 run()：为每个 handler 创建订阅，并在 select 循环中分发
    let mut sub_decls = Vec::new();
    let mut select_arms = Vec::new();
    for (idx, ms) in methods.iter().enumerate() {
        let sub_var = format_ident!("__sub_{}", idx);
        let ty = &ms.msg_ty;
        let method_ident = &ms.ident;
        let pattern = &ms.pattern_tokens;
        sub_decls.push(quote! {
            let mut #sub_var = ctx
                .subscribe_pattern::<#ty>(#pattern)
                .await;
        });
        // 生成配置注入绑定与调用表达式
        let mut __cfg_bind = Vec::new();
        let mut __cfg_args = Vec::new();
    for (ci, cty) in ms.cfg_param_tys.iter().enumerate() {
            let var = format_ident!("__cfg_{}_{}", idx, ci);
            __cfg_bind.push(quote! {
        let #var = match ctx.cfg.get::<#cty>() { Some(v) => v, None => {
            let __ty: &str = std::any::type_name::<#cty>();
            tracing::error!("missing config: {}", __ty);
            continue; } };
            });
            __cfg_args.push(quote! { &*#var });
        }
        let call_expr = if ms.wants_ctx {
            quote! { this.#method_ident(&ctx, &*env, #(#__cfg_args),*).await }
        } else {
            quote! { this.#method_ident(&*env, #(#__cfg_args),*).await }
        };
        select_arms.push(quote! {
            Some(env) = #sub_var.recv() => {
                #(#__cfg_bind)*
                if let Err(e) = { #call_expr } { tracing::warn!(method = stringify!(#method_ident), error = ?e, "handler returned error"); }
            }
        });
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
                // 主循环：监听关停与各类型订阅
                loop {
                    tokio::select! {
                        changed = ctx.shutdown.changed() => {
                            if changed.is_ok() { break; } else { break; }
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
