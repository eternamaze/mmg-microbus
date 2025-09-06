pub mod app;
pub mod bus;
pub mod component;
pub mod config;
pub mod registry;

pub mod prelude {
    pub use crate::app::App;
    // 参数注入核心：仅通过函数参数访问上下文、消息与配置
    pub use crate::component::ComponentContext;
    pub type Result<T = ()> = anyhow::Result<T>;
}

pub use microbus_macros::*;
// 供宏展开使用，避免下游使用者显式依赖 inventory
pub use inventory;
// 不再提供具体化主函数宏；框架仅提供标准启停 API
