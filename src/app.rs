use crate::error::Result;
use std::any::TypeId;
use tokio::task::JoinHandle;

use crate::{
    bus::{Bus, BusHandle},
    component::{
        ComponentContext, ConfigStore, __RegisteredFactory, __new_startup_barrier, __new_stop_flag,
        __trigger_stop_flag,
    },
    config::AppConfig,
};

pub struct App {
    _cfg: AppConfig,
    bus: Bus,
    tasks: Vec<JoinHandle<()>>,
    started: bool,
    stop_flag: std::sync::Arc<crate::component::StopFlag>,
    // 启动前暂存的配置条目（类型 -> Arc<T>），启动时冻结为只读 ConfigStore
    cfg_map: std::collections::HashMap<
        std::any::TypeId,
        std::sync::Arc<dyn std::any::Any + Send + Sync>,
    >,
    frozen_cfg: Option<ConfigStore>,
}

impl App {
    pub fn new(cfg: AppConfig) -> Self {
        let bus = Bus::new(cfg.queue_capacity);
        let stop_flag = __new_stop_flag();
        Self {
            _cfg: cfg,
            bus,
            tasks: Vec::new(),
            started: false,
            stop_flag,
            cfg_map: std::collections::HashMap::new(),
            frozen_cfg: None,
        }
    }

    /// 配置入口（单项）：
    /// - 传入任意业务配置类型，按类型存入只读配置仓库，供 #[init] 形参按 &T 自动注入。
    /// - 框架配置仅通过 `App::new(AppConfig)` 提供；业务配置通过 `config()` 注入。
    /// - 可多次调用以注入多种类型。
    /// - 仅在启动前允许；启动后不支持动态更新。
    pub async fn config<T: 'static + Send + Sync>(&mut self, cfg: T) -> Result<&mut Self> {
        // 启动后禁止配置：忽略并发出警告
        if self.started {
            tracing::warn!(config_type = %std::any::type_name::<T>(), "config called after start(); ignoring");
            return Ok(self);
        }
        let tid = TypeId::of::<T>();
        if self.cfg_map.contains_key(&tid) {
            tracing::warn!(config_type = %std::any::type_name::<T>(), "config for this type provided multiple times before start; overriding");
        }
        let entry = std::sync::Arc::new(cfg) as std::sync::Arc<dyn std::any::Any + Send + Sync>;
        self.cfg_map.insert(tid, entry);
        Ok(self)
    }
    /// 批量配置入口（闭包）：仅为 `config` 的薄包装器，调用方提供一个异步闭包，内部按序调用 `self.config(...)`。
    /// 示例：
    /// app.config_many(|a| async {
    ///     a.config(CfgA{..}).await?;
    ///     a.config(CfgB{..}).await
    /// }).await?;
    pub async fn config_many<F>(&mut self, f: F) -> Result<&mut Self>
    where
        F: for<'a> FnOnce(
            &'a mut App,
        ) -> core::pin::Pin<
            Box<dyn core::future::Future<Output = Result<()>> + Send + 'a>,
        >,
    {
        if self.started {
            tracing::warn!("config_many called after start(); ignoring all provided configs");
            return Ok(self);
        }
        f(self).await?;
        Ok(self)
    }

    // 框架配置仅能在 new() 时提供；运行期不支持修改。
    pub async fn start(&mut self) -> Result<()> {
        if self.started {
            return Ok(());
        }
        // 冻结配置存储（单例自动发现模式下，不再需要外部 add_component 装配）
        let cfg_store = if let Some(f) = self.frozen_cfg.clone() {
            f
        } else {
            let frozen = ConfigStore::from_frozen_map(self.cfg_map.clone());
            self.frozen_cfg = Some(frozen.clone());
            frozen
        };
        // 自动发现：inventory 收集的所有工厂；按 kind 去重（单例模式）。
        let bus_handle = self.bus.handle();
        let factories: Vec<&__RegisteredFactory> =
            inventory::iter::<__RegisteredFactory>.into_iter().collect();
        let total = factories.len();
        let startup_barrier = __new_startup_barrier(total);
        for reg in factories.into_iter() {
            let factory = (reg.create)();
            let name = factory.type_name().to_string();
            let stop_clone = self.stop_flag.clone();
            let bus_clone = bus_handle.clone();
            let cfg_store_i = cfg_store.clone();
            let barrier_clone = startup_barrier.clone();
            let fut = async move {
                match factory.build(bus_clone.clone()).await {
                    Ok(comp) => {
                        let ctx = ComponentContext::new_with_service(
                            name.clone(),
                            bus_clone.clone(),
                            stop_clone.clone(),
                            cfg_store_i.clone(),
                            barrier_clone.clone(),
                        );
                        if let Err(e) = comp.run(ctx).await {
                            tracing::error!(component = %name, kind = %factory.type_name(), error = %e, "component exited with error");
                        }
                    }
                    Err(e) => {
                        tracing::error!(component = %name, kind = %factory.type_name(), error = %e, "failed to build component");
                    }
                }
            };
            let h = tokio::spawn(fut);
            self.tasks.push(h);
        }
        // 让出多次调度，尽力确保所有组件进入 run() 并完成各自订阅
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
        // 冻结 bus：后续不再期望新增订阅
        self.bus.handle().seal();
        self.started = true;
        Ok(())
    }
    pub async fn stop(&mut self) {
        // 框架主导的单方面停机：
        // 1) 发出停止信号；
        __trigger_stop_flag(&self.stop_flag);
        // 2) 强制结束所有组件任务（无需等待其“自然退出”）。
        let mut rest = Vec::new();
        rest.append(&mut self.tasks);
        for h in rest.into_iter() {
            // 组件 run() 应该在收到停止后尽快返回；这里直接等待一次 join，若 panic/取消也忽略。
            let _ = h.await;
        }
        self.started = false;
    }
    pub fn bus_handle(&self) -> BusHandle {
        self.bus.handle()
    }
    pub fn is_started(&self) -> bool {
        self.started
    }
}

// tests are covered in integration suite; unit tests omitted here
