use anyhow::Result;
use std::any::{Any, TypeId};
use tokio::task::JoinHandle;

use crate::{
    bus::{Bus, BusHandle},
    component::{ComponentContext, ConfigStore, RegisteredComponent},
    config::{AppConfig, ComponentConfig},
};

pub struct App {
    cfg: AppConfig,
    bus: Bus,
    tasks: Vec<JoinHandle<()>>,
    started: bool,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    factories: std::collections::HashMap<
        crate::bus::KindId,
        std::sync::Arc<dyn crate::component::ComponentFactory>,
    >,
    // 启动前暂存的配置条目（类型 -> Arc<T>），启动时冻结为只读 ConfigStore
    cfg_map: std::collections::HashMap<
        std::any::TypeId,
        std::sync::Arc<dyn std::any::Any + Send + Sync>,
    >,
    frozen_cfg: Option<ConfigStore>,
    // 记录框架配置是否已设置过，用于重复设置时发出覆盖警告
    app_cfg_set: bool,
}

impl App {
    pub fn new(cfg: AppConfig) -> Self {
        let bus = Bus::new(cfg.queue_capacity);
        let (tx, _rx) = tokio::sync::watch::channel(false);
        Self {
            cfg,
            bus,
            tasks: Vec::new(),
            started: false,
            shutdown_tx: tx,
            factories: std::collections::HashMap::new(),
            cfg_map: std::collections::HashMap::new(),
            frozen_cfg: None,
            app_cfg_set: false,
        }
    }

    /// 以类型安全的方式注册组件实例并登记其工厂，避免外部硬编码类型名字符串。
    pub fn add_component<T: RegisteredComponent + 'static>(
        &mut self,
        id: impl Into<String>,
    ) -> &mut Self {
        let kind = <T as RegisteredComponent>::kind_id();
        // 注册工厂（若未注册）
        self.factories
            .entry(kind)
            .or_insert_with(|| <T as RegisteredComponent>::factory());
        self.cfg.components.push(ComponentConfig {
            id: id.into(),
            kind,
        });
        self
    }

    /// 配置入口（单项）：
    /// - 传入任意业务配置类型，按类型存入只读配置仓库，供 #[init] 形参按 &T 自动注入。
    /// - 传入框架配置类型（如 AppConfig）会被框架消费并应用到 App 本身，不进入业务配置仓库。
    /// - 可多次调用以注入多种类型。
    /// - 仅在启动前允许；启动后不支持动态更新。
    pub async fn config<T: 'static + Send + Sync>(&mut self, cfg: T) -> Result<&mut Self> {
        // 启动后禁止配置：忽略并发出警告
        if self.started {
            tracing::warn!(config_type = %std::any::type_name::<T>(), "config called after start(); ignoring");
            return Ok(self);
        }
        if TypeId::of::<T>() == TypeId::of::<AppConfig>() {
            let any_box: Box<dyn Any + Send + Sync> = Box::new(cfg);
            match any_box.downcast::<AppConfig>() {
                Ok(b) => {
                    self.set_app_config(*b);
                }
                Err(_) => {
                    debug_assert!(false, "TypeId matched AppConfig but downcast failed");
                    return Ok(self);
                }
            }
        } else {
            let tid = TypeId::of::<T>();
            if self.cfg_map.contains_key(&tid) {
                tracing::warn!(config_type = %std::any::type_name::<T>(), "config for this type provided multiple times before start; overriding");
            }
            let entry = std::sync::Arc::new(cfg) as std::sync::Arc<dyn std::any::Any + Send + Sync>;
            self.cfg_map.insert(tid, entry);
        }
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

    fn apply_app_config(&mut self, cfg: AppConfig) {
        // 合并策略：直接覆盖为最新值（队列容量/停机等待/组件列表）。
        // 组件列表通常通过 add_component 维护；如通过 AppConfig 提供，也予以接纳。
        self.cfg = cfg.clone();
        self.bus = Bus::new(self.cfg.queue_capacity);
    }
    /// 设置框架配置的专用通道，避免和业务配置混用。
    pub fn set_app_config(&mut self, cfg: AppConfig) {
        if self.started {
            tracing::warn!("set_app_config called after start(); ignoring");
            return;
        }
        if self.app_cfg_set {
            tracing::warn!("AppConfig provided multiple times before start; overriding previous values");
        }
        self.app_cfg_set = true;
        self.apply_app_config(cfg);
    }
    pub async fn start(&mut self) -> Result<()> {
        if self.started {
            return Ok(());
        }
        // 统一入口：必须显式装配组件。若未配置，立即报错，避免出现多种装配体验。
        if self.cfg.components.is_empty() {
            return Err(anyhow::anyhow!(
                "no components configured: call App::add_component::<T>(id) before start()"
            ));
        }
        // 冻结配置存储
        let cfg_store = if let Some(f) = self.frozen_cfg.clone() {
            f
        } else {
            let frozen = ConfigStore::from_frozen_map(self.cfg_map.clone());
            self.frozen_cfg = Some(frozen.clone());
            frozen
        };
        // 工厂表来自 add_component 阶段登记的 KindId -> Factory

    // 不再进行路由约束检查：handle 仅按消息类型与可选实例字符串过滤，无需检验组件种类单例性。

        let handle = self.bus.handle();
        for cc in self.cfg.components.iter() {
            // 查表匹配配置的 kind（KindId）
            let factory = match self.factories.get(&cc.kind) {
                Some(f) => f.clone(),
                None => {
                    return Err(anyhow::anyhow!(
                        "unknown component kind: ensure component type with #[component] is linked and added"
                    ));
                }
            };
            let id = cc.id.clone();
            let bus_handle = handle.clone();
            let kind_id = factory.kind_id();
            let rx = self.shutdown_tx.subscribe();
            let cfg_store_i = cfg_store.clone();
            let fut = async move {
                let built = factory
                    .build(crate::bus::ComponentId(id.clone()), bus_handle.clone())
                    .await;
                match built {
                    Ok(comp) => {
                        let ctx = ComponentContext::new_with_service(
                            crate::bus::ComponentId(id.clone()),
                            kind_id,
                            bus_handle.clone(),
                            rx.clone(),
                            cfg_store_i.clone(),
                        );
                        // 运行组件（组件通过参数注入获取上下文、消息与配置）
                        if let Err(e) = comp.run(ctx).await {
                            tracing::error!(component = %id, kind = %factory.type_name(), error = %e, "component exited with error");
                        }
                    }
                    Err(e) => {
                        tracing::error!(component = %id, kind = %factory.type_name(), error = %e, "failed to build component");
                    }
                }
            };
            let h = tokio::spawn(fut);
            self.tasks.push(h);
        }
        self.started = true;
        Ok(())
    }
    pub async fn stop(&mut self) {
        let _ = self.shutdown_tx.send(true);
        // 等待所有组件任务自然退出（组件应响应 shutdown 信号或在 run 中 break）
        let mut rest = Vec::new();
        rest.append(&mut self.tasks);
        for h in rest.into_iter() {
            let _ = h.await; // 忽略错误（如任务自行返回 Err）
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
