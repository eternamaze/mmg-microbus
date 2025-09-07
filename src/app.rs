use anyhow::Result;
use tokio::task::JoinHandle;

use crate::{
    bus::{Bus, BusHandle},
    component::{ComponentContext, ConfigStore},
    config::{AppConfig, ComponentConfig},
};

pub struct App {
    cfg: AppConfig,
    bus: Bus,
    tasks: Vec<JoinHandle<()>>,
    started: bool,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_linger: std::time::Duration,
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
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let linger = std::time::Duration::from_millis(cfg.shutdown_linger_ms);
        Self {
            cfg,
            bus,
            tasks: Vec::new(),
            started: false,
            shutdown_tx: tx,
            shutdown_linger: linger,
            cfg_map: std::collections::HashMap::new(),
            frozen_cfg: None,
        }
    }

    /// 以类型安全的方式注册组件实例，避免外部硬编码类型名字符串。
    pub fn add_component<T: 'static>(&mut self, id: impl Into<String>) -> &mut Self {
        let kind = crate::bus::KindId::of::<T>();
        self.cfg.components.push(ComponentConfig {
            id: id.into(),
            kind,
        });
        self
    }

    /// 注入一个类型化配置条目；可多次调用以注入多种配置类型。
    /// - 仅在启动前允许；启动时将冻结为只读的 ConfigStore。
    pub async fn provide_config<T: 'static + Send + Sync>(&mut self, cfg: T) -> Result<&mut Self> {
        use std::any::TypeId;
        if self.started {
            return Err(anyhow::anyhow!(
                "App already started; runtime config updates are not supported"
            ));
        }
        let entry = std::sync::Arc::new(cfg) as std::sync::Arc<dyn std::any::Any + Send + Sync>;
        self.cfg_map.insert(TypeId::of::<T>(), entry);
        Ok(self)
    }
    pub async fn start(&mut self) -> Result<()> {
        if self.started {
            return Ok(());
        }
        // 若未显式配置组件，则按注册信息自动实例化：默认单例；若声明了 instances 列表则逐个实例化
        if self.cfg.components.is_empty() {
            for reg in crate::registry::all() {
                let kind = (reg.kind)();
                let instances = (reg.instances)();
                if instances.is_empty() {
                    // default instance id: use type name for readability
                    self.cfg.components.push(crate::config::ComponentConfig {
                        id: (reg.type_name)().to_string(),
                        kind,
                    });
                } else {
                    for &name in instances.iter() {
                        self.cfg.components.push(crate::config::ComponentConfig {
                            id: name.to_string(),
                            kind,
                        });
                    }
                }
            }
        }
        // 冻结配置存储
        let cfg_store = if let Some(f) = self.frozen_cfg.clone() {
            f
        } else {
            let frozen = ConfigStore::from_frozen_map(self.cfg_map.clone());
            self.frozen_cfg = Some(frozen.clone());
            frozen
        };
        // 预构建工厂表：KindId -> factory
        let mut factories: std::collections::HashMap<
            crate::bus::KindId,
            std::sync::Arc<dyn crate::component::ComponentFactory>,
        > = std::collections::HashMap::new();
        for reg in crate::registry::all() {
            let f = (reg.new_factory)();
            factories.entry(f.kind_id()).or_insert(f);
        }

        // 校验路由约束：凡 handler 声明 from=Kind 且未指明 instance，要求系统中该 kind 只有一个实例
        let mut instance_count: std::collections::HashMap<crate::bus::KindId, usize> =
            std::collections::HashMap::new();
        for c in self.cfg.components.iter() {
            *instance_count.entry(c.kind).or_insert(0) += 1;
        }
        for rc in crate::registry::route_constraints() {
            let n = instance_count.get(&(rc.from_kind)()).cloned().unwrap_or(0);
            if n == 0 {
                return Err(anyhow::anyhow!("route constraint failed: {} expects singleton of kind {:?}, but none configured", (rc.consumer_ty)(), (rc.from_kind)()));
            }
            if n > 1 {
                return Err(anyhow::anyhow!("route constraint failed: {} expects singleton of kind {:?}, but {} instances configured; specify instance in #[handle(.., instance=..)]", (rc.consumer_ty)(), (rc.from_kind)(), n));
            }
        }
        let handle = self.bus.handle();
        for cc in self.cfg.components.iter() {
            // 查表匹配配置的 kind（KindId）
            let factory = match factories.get(&cc.kind) {
                Some(f) => f.clone(),
                None => return Err(anyhow::anyhow!("unknown component kind")),
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
        let linger = self.shutdown_linger;
        if linger > std::time::Duration::from_millis(0) {
            tokio::time::sleep(linger).await;
        }
        for h in self.tasks.drain(..) {
            h.abort();
        }
        self.started = false;
    }
    pub fn bus_handle(&self) -> BusHandle {
        self.bus.handle()
    }
    pub fn is_started(&self) -> bool {
        self.started
    }
    pub fn set_shutdown_linger(&mut self, dur: std::time::Duration) -> &mut Self {
        self.shutdown_linger = dur;
        self
    }
}
