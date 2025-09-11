pub mod app;
pub mod bus;
pub mod component;
pub mod config;
pub mod error;

// 允许在本 crate 内通过 `mmg_microbus::...` 自引用（供 proc-macro 展开使用）
extern crate self as mmg_microbus;

pub mod prelude {
    pub use crate::app::App;
    // 参数注入：仅通过函数参数访问上下文、消息与配置
    pub use crate::component::ComponentContext;
    pub use crate::error::{MicrobusError, Result};
}

pub use microbus_macros::*;
// 框架提供标准启停 API（无入口宏）
