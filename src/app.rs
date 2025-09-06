use anyhow::Result;
use tokio::task::JoinHandle;

use crate::{
    bus::{Bus, BusHandle},
    component::ComponentContext,
    config::{AppConfig, ComponentConfig},
};

pub struct App {
    cfg: AppConfig,
    bus: Bus,
    tasks: Vec<JoinHandle<()>>,
    started: bool,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    shutdown_linger: std::time::Duration,
    // initial typed config blob (project-wide aggregate struct)
    init_cfg_any: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
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
            init_cfg_any: None,
        }
    }

    /// 以类型安全的方式注册组件实例，避免外部硬编码类型名字符串。
    pub fn add_component<T: 'static>(&mut self, id: impl Into<String>) -> &mut Self {
        let kind = crate::bus::KindId::of::<T>();
        self.cfg.components.push(ComponentConfig { id: id.into(), kind });
        self
    }

    /// 强类型配置入口（仅启动前一次性注入）。
    /// - 启动前调用：作为初始配置，在 start() 时注入到每个组件。
    /// - 已启动后调用将返回错误（不支持热更新）。
    pub async fn config<T: 'static + Send + Sync>(&mut self, cfg: T) -> Result<&mut Self> {
        let v: std::sync::Arc<dyn std::any::Any + Send + Sync> = std::sync::Arc::new(cfg);
        if self.started {
            return Err(anyhow::anyhow!(
                "App already started; runtime config updates are not supported"
            ));
        }
        self.init_cfg_any = Some(v);
        Ok(self)
    }
    pub async fn start(&mut self) -> Result<()> {
        if self.started {
            return Ok(());
        }
        // 需要显式组件配置；框架不再自动实例化组件
        if self.cfg.components.is_empty() {
            return Err(anyhow::anyhow!(
                "no components configured; please provide AppConfig.components explicitly"
            ));
        }

        // 预构建工厂表：type_name -> factory
        let mut factories: std::collections::HashMap<crate::bus::KindId, std::sync::Arc<dyn crate::component::ComponentFactory>> = std::collections::HashMap::new();
        for e in inventory::iter::<crate::registry::FactoryEntry> {
            let f = (e.0)();
            factories.entry(f.kind_id()).or_insert(f);
        }
        let handle = self.bus.handle();
        // 启动时的初始配置（若无则使用空占位 Arc<()> ）
        let init_any = self
            .init_cfg_any
            .clone()
            .unwrap_or_else(|| std::sync::Arc::new(()));
        for cc in self.cfg.components.iter() {
            // 查表匹配配置的 kind（KindId）
            let factory = match factories.get(&cc.kind) {
                Some(f) => f.clone(),
                None => return Err(anyhow::anyhow!("unknown component kind")),
            };
            let id = cc.id.clone();
            let bus_handle = handle.clone();
            // ComponentConfig no longer carries JSON params; typed config flows via App::config(T)
            let kind_id = factory.kind_id();
            let rx = self.shutdown_tx.subscribe();
            let init_any_i = init_any.clone();
            let fut = async move {
                let built = factory
                    .build(crate::bus::ComponentId(id.clone()), bus_handle.clone())
                    .await;
                match built {
                    Ok(mut comp) => {
                        // Use type-based service kind for routing clarity
                        let ctx = ComponentContext::new_with_service(
                            crate::bus::ComponentId(id.clone()),
                            kind_id,
                            bus_handle.clone(),
                            rx.clone(),
                        );
                        // 启动时进行一次配置应用（若注册了 #[configure(T)]）
                        let cfg_ctx = crate::component::ConfigContext::new(
                            crate::bus::ComponentId(id.clone()),
                            kind_id,
                        );
                        for ce in inventory::iter::<crate::config_registry::DesiredCfgEntry> {
                            if (ce.0.component_kind)() == kind_id {
                                let _ = (ce.0.invoke)(&mut *comp, cfg_ctx.clone(), init_any_i.clone()).await;
                            }
                        }
                        
                        // 运行组件
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
