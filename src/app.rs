use anyhow::Result;
#[cfg(feature = "bus-metrics")]
use std::sync::Arc;
use tokio::task::JoinHandle;

#[cfg(feature = "bus-metrics")]
use crate::bus::BusMetrics;
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
    // per-component typed config channels (Any)
    comp_cfg_txs: Vec<(
        String,
        tokio::sync::watch::Sender<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
    )>,
    // initial typed config blob (project-wide aggregate struct)
    init_cfg_any: Option<std::sync::Arc<dyn std::any::Any + Send + Sync>>,
}

impl App {
    pub fn new(cfg: AppConfig) -> Self {
        #[cfg(feature = "bus-metrics")]
        let bus = {
            let metrics = if cfg.bus_metrics {
                Some(Arc::new(BusMetrics::new()))
            } else {
                None
            };
            Bus::new(cfg.queue_capacity, metrics)
        };
        #[cfg(not(feature = "bus-metrics"))]
        let bus = {
            if cfg.bus_metrics {
                // 编译时未启用 bus-metrics 特性，但配置中请求了 metrics：给出明确提示
                tracing::warn!("bus-metrics feature disabled at compile time; AppConfig.bus_metrics=true will be ignored");
            }
            Bus::new(cfg.queue_capacity)
        };
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let linger = std::time::Duration::from_millis(cfg.shutdown_linger_ms);
        Self {
            cfg,
            bus,
            tasks: Vec::new(),
            started: false,
            shutdown_tx: tx,
            shutdown_linger: linger,
            comp_cfg_txs: Vec::new(),
            init_cfg_any: None,
        }
    }
    pub fn new_default() -> Self {
        Self::new(AppConfig::default())
    }
    /// 强类型配置入口：传入聚合配置结构体（项目自定义类型）。
    /// - 启动前调用：作为初始配置，在 start() 时注入到每个组件。
    /// - 运行期调用：暂停总线，广播到所有组件的配置通道（Arc<dyn Any>），作为热更新注入。
    pub async fn config<T: 'static + Send + Sync>(&mut self, cfg: T) -> Result<&mut Self> {
        let v: std::sync::Arc<dyn std::any::Any + Send + Sync> = std::sync::Arc::new(cfg);
        if !self.started {
            self.init_cfg_any = Some(v);
            return Ok(self);
        }
        // runtime: pause -> broadcast -> tiny barrier -> resume
        let h = self.bus.handle();
        h.pause();
        for (_id, tx) in &self.comp_cfg_txs {
            let _ = tx.send(v.clone());
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        h.resume();
        Ok(self)
    }
    pub async fn start(&mut self) -> Result<()> {
        if self.started {
            return Ok(());
        }
        // 基于注解自动发现，每个组件默认一个实例
        if self.cfg.components.is_empty() {
            for e in inventory::iter::<crate::registry::FactoryEntry> {
                let f = (e.0)();
                let name = f.type_name();
                let id = format!("{}-1", name);
                self.cfg.components.push(ComponentConfig {
                    id,
                    kind: name.to_string(),
                });
            }
        }

        let handle = self.bus.handle();
        for cc in self.cfg.components.iter() {
            // Find factory from inventory by type name
            let mut factory_opt = None;
            for e in inventory::iter::<crate::registry::FactoryEntry> {
                let f = (e.0)();
                if f.type_name() == cc.kind {
                    factory_opt = Some(f);
                    break;
                }
            }
            let factory = match factory_opt {
                Some(f) => f,
                None => {
                    return Err(anyhow::anyhow!("unknown component kind: {}", cc.kind));
                }
            };
            let id = cc.id.clone();
            let bus_handle = handle.clone();
            // ComponentConfig no longer carries JSON params; typed config flows via App::config(T)
            let kind_id = factory.kind_id();
            let rx = self.shutdown_tx.subscribe();
            // create per-component typed config watch
            // 初始化为全局 init_cfg_any（若提供）；否则使用空占位 Arc<()> 表示“无配置”。
            let init_any = self
                .init_cfg_any
                .clone()
                .unwrap_or_else(|| std::sync::Arc::new(()));
            let (cfg_tx, cfg_rx) = tokio::sync::watch::channel(init_any);
            self.comp_cfg_txs.push((id.clone(), cfg_tx));
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
                            cfg_rx,
                        );
                        // 配置处理：启动前调用一次（若注册了 #[configure(T)] ）
                        let cfg_ctx = crate::component::ConfigContext::new(
                            crate::bus::ComponentId(id.clone()),
                            kind_id,
                        );
                        for ce in inventory::iter::<crate::config_registry::DesiredCfgEntry> {
                            if (ce.0.component_kind)() == kind_id {
                                let v = ctx.current_config_any();
                                if let Err(e) = (ce.0.invoke)(&mut *comp, cfg_ctx.clone(), v).await
                                {
                                    tracing::warn!(component = %id, error = ?e, "config handler failed at startup");
                                }
                            }
                        }
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

    /// Zero-config bootstrap: discover factories and start with default instances.
    pub async fn bootstrap(&mut self) -> Result<()> {
        self.start().await
    }

    /// 单一启动入口：启动直至 Ctrl-C（SIGINT）。
    pub async fn run_until_ctrl_c(&mut self) -> Result<()> {
        self.bootstrap().await?;
        // Startup summary
        let mut kinds: Vec<&'static str> = Vec::new();
        for e in inventory::iter::<crate::registry::FactoryEntry> {
            kinds.push(((e.0)()).type_name());
        }
        tracing::info!(kinds=?kinds, instances=self.cfg.components.len(), queue=self.cfg.queue_capacity, "mmg-microbus started");
        // Wait for interrupt
        tokio::signal::ctrl_c().await.ok();
        self.stop().await;
        Ok(())
    }
}
