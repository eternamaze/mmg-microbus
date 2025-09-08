use crate::bus::KindId;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub queue_capacity: usize,
    pub components: Vec<ComponentConfig>,
}

pub const APP_DEFAULT_QUEUE: usize = 1024;

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            queue_capacity: APP_DEFAULT_QUEUE,
            components: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ComponentConfig {
    pub id: String,
    pub kind: KindId,
}

// 配置结构体仅负责运行参数；业务配置以类型注入方式在 handle 签名中获取。
