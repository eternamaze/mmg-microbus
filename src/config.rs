#[derive(Debug, Clone)]
pub struct AppConfig {
    pub queue_capacity: usize,
}

pub const APP_DEFAULT_QUEUE: usize = 1024;

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            queue_capacity: APP_DEFAULT_QUEUE,
        }
    }
}
// 运行期配置仅保留队列容量；组件采用全局单例自动发现。
