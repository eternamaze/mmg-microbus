pub mod app;
pub mod bus;
pub mod component;
pub mod config;
pub mod config_registry;
pub mod registry;

pub mod prelude {
    pub use crate::app::App;
    // 仅暴露用户常用的高层注入类型（ScopedBus 已移除，统一通过 ComponentContext 使用 BusHandle）
    pub use crate::component::{ConfigContext, Configure};
    pub type Result<T = ()> = anyhow::Result<T>;
}

pub use microbus_macros::*;
// 供宏展开使用，避免下游使用者/trybuild 用例显式依赖 inventory
pub use inventory;
// 不再提供具体化主函数宏；框架仅提供标准启停 API
