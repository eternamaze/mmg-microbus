use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_queue")]
    pub queue_capacity: usize,
    #[serde(default)]
    pub components: Vec<ComponentConfig>,
    #[serde(default = "default_shutdown_linger_ms")]
    pub shutdown_linger_ms: u64,
    #[serde(default)]
    pub bus_metrics: bool,
}

pub const APP_DEFAULT_QUEUE: usize = 1024;
fn default_queue() -> usize {
    APP_DEFAULT_QUEUE
}
pub fn default_shutdown_linger_ms() -> u64 {
    2000
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            queue_capacity: APP_DEFAULT_QUEUE,
            components: Vec::new(),
            shutdown_linger_ms: default_shutdown_linger_ms(),
            bus_metrics: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentConfig {
    pub id: String,
    pub kind: String,
}

// 配置结构体仅负责运行参数，不声明订阅或拓扑。
