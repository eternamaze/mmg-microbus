use super::msgs::{ERR_ACTIVE_LIST_ONCE_ONLY, ERR_ACTIVE_NO_NV};
use syn::{Attribute, Type};

// 低层解析与判别辅助

#[inline]
pub fn is_ctx_type(ty: &syn::Type) -> bool {
    if let syn::Type::Reference(r) = ty {
        if let syn::Type::Path(tp) = &*r.elem {
            return tp
                .path
                .segments
                .last()
                .is_some_and(|s| s.ident == "ComponentContext");
        }
    }
    false
}

#[inline]
pub fn parse_msg_arg_ref(ty: &syn::Type) -> Option<Type> {
    if let syn::Type::Reference(r) = ty {
        if let syn::Type::Path(tp) = &*r.elem {
            return Some(Type::Path(tp.clone()));
        }
    }
    None
}

// #[handle] 不允许任何参数（保持当前模型简单）
#[inline]
pub fn parse_handle_attr(a: &Attribute) -> bool {
    a.meta.require_path_only().is_err()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ActiveKind {
    Loop,
    Once,
}

pub fn parse_active_kind(a: &Attribute) -> Option<syn::Result<ActiveKind>> {
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
        syn::Meta::List(list_meta) => {
            if list_meta.tokens.is_empty() {
                return Some(Ok(ActiveKind::Loop));
            }
            let content = list_meta.tokens.to_string();
            if content.trim() == "once" {
                Some(Ok(ActiveKind::Once))
            } else {
                Some(Err(syn::Error::new_spanned(
                    &list_meta.tokens,
                    ERR_ACTIVE_LIST_ONCE_ONLY,
                )))
            }
        }
        syn::Meta::NameValue(nv) => Some(Err(syn::Error::new_spanned(nv, ERR_ACTIVE_NO_NV))),
    }
}
