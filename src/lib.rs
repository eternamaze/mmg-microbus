pub mod app;
pub mod bus;
pub mod component;
pub mod config;
pub mod config_registry;
pub mod registry;

pub mod prelude {
    pub use crate::app::App;
    pub use crate::bus::{BusHandle, Envelope, ScopedBus, ServiceAddr, ServicePattern};
    pub use crate::component::{ConfigApplyDyn, ConfigContext, Configure};
    pub type Result<T = ()> = anyhow::Result<T>;
}

pub use microbus_macros::*;
// 供宏展开使用，避免下游使用者/trybuild 用例显式依赖 inventory
pub use inventory;

// 最简主函数宏：自动生成 tokio 入口并运行 App::run_default()
#[macro_export]
macro_rules! easy_main {
    () => {
        #[::tokio::main(flavor = "multi_thread")]
        async fn main() -> ::anyhow::Result<()> {
            ::mmg_microbus::prelude::App::run_default().await
        }
    };
}

// 携带一次性启动配置的主函数宏
#[macro_export]
macro_rules! easy_main_with_config {
    ($cfg:expr) => {
        #[::tokio::main(flavor = "multi_thread")]
        async fn main() -> ::anyhow::Result<()> {
            ::mmg_microbus::prelude::App::run_with_config($cfg).await
        }
    };
}
