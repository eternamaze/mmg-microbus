//! microbus-macros 宏入口（接口层）。
//!
//! 仅声明属性宏并把展开逻辑转发到 `gen.rs`；本文件不包含任何实现，以符合“接口与实现分离”约束。
//!
//! 属性简述：
//! - #[component] : struct => 工厂注册；impl => 生成 Component::run
//! - #[handle]    : (&ComponentContext? , &T) -> 六类返回之一，自动发布
//! - #[active]    : 主动逻辑；可 #[active(once)] 一次执行
//! - #[init]      : 主循环前一次调用（无外部配置注入）
//! - #[stop]      : 退出前一次调用

use proc_macro::TokenStream;

mod gen; // 私有实现模块

#[proc_macro_attribute]
pub fn component(args: TokenStream, input: TokenStream) -> TokenStream {
    gen::component_entry(args, input)
}

#[proc_macro_attribute]
pub fn handle(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn init(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn stop(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}

#[proc_macro_attribute]
pub fn active(_args: TokenStream, input: TokenStream) -> TokenStream {
    input
}
