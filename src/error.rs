//! 框架统一错误类型：最小化枚举，避免依赖第三方错误栈，实现简单直接。
use std::{error::Error as StdError, fmt};

#[derive(Debug)]
pub enum MicrobusError {
    NoComponents,                 // 启动时未注册组件
    UnknownComponentKind,         // 注册的 kind 在工厂表中缺失（理论上不应出现）
    MissingConfig(&'static str),  // #[init] 所需配置缺失
    Other(&'static str),          // 简单静态消息
    Dynamic(String),              // 动态字符串（极少使用）
}

impl fmt::Display for MicrobusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MicrobusError::NoComponents => write!(f, "no components configured"),
            MicrobusError::UnknownComponentKind => write!(f, "unknown component kind"),
            MicrobusError::MissingConfig(t) => write!(f, "missing config for init: {t}"),
            MicrobusError::Other(msg) => write!(f, "{msg}"),
            MicrobusError::Dynamic(s) => write!(f, "{s}"),
        }
    }
}
impl StdError for MicrobusError {}

pub type Result<T = ()> = std::result::Result<T, MicrobusError>;

// 已无动态错误构造辅助需求，保留枚举即可（err_dynamic 移除）