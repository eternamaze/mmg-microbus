use anyhow::{Context, Result};
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
    // per-component config channels for hot update
    comp_cfg_txs: Vec<(String, tokio::sync::watch::Sender<serde_json::Value>)>,
    // global, typed-config serialized as JSON; applied at start or via config() at runtime
    init_cfg_json: Option<serde_json::Value>,
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
            init_cfg_json: None,
        }
    }
    pub fn new_default() -> Self {
        Self::new(AppConfig::default())
    }
    /// 统一配置入口：传入任意可序列化的配置类型，框架只负责分发，不参与构造。
    /// - 启动前调用：作为初始配置，在 start() 时注入到每个组件。
    /// - 运行期调用：暂停总线，广播到所有组件的配置通道，作为热更新注入。
    pub async fn config<T: serde::Serialize>(&mut self, cfg: T) -> Result<&mut Self> {
        let v = serde_json::to_value(cfg).context("serialize config to json")?;
        if !self.started {
            self.init_cfg_json = Some(v);
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
                    params: serde_json::Value::Null,
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
            let factory =
                factory_opt.with_context(|| format!("unknown component kind: {}", cc.kind))?;
            let id = cc.id.clone();
            let bus_handle = handle.clone();
            let params_owned = cc.params.clone();
            let kind_id = factory.kind_id();
            let rx = self.shutdown_tx.subscribe();
            // create per-component config watch
            // 初始化每个组件的配置流：优先使用全局 init config，否则沿用默认（通常为 Null）
            let init_v = self
                .init_cfg_json
                .clone()
                .unwrap_or_else(|| params_owned.clone());
            let (cfg_tx, cfg_rx) = tokio::sync::watch::channel(init_v);
            self.comp_cfg_txs.push((id.clone(), cfg_tx));
            let fut = async move {
                let parsed = factory
                    .parse_config((!params_owned.is_null()).then_some(params_owned))
                    .map_err(|e| {
                        tracing::warn!(component = %id, kind = %factory.type_name(), error = ?e, "parse_config failed; falling back to build()");
                        e
                    })
                    .ok();
                let built = if let Some(cfg) = parsed {
                    factory
                        .build_with_config(
                            crate::bus::ComponentId(id.clone()),
                            bus_handle.clone(),
                            cfg,
                        )
                        .await
                } else {
                    factory
                        .build(crate::bus::ComponentId(id.clone()), bus_handle.clone())
                        .await
                };
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
                            ctx.scoped_bus.clone(),
                        );
                        for ce in inventory::iter::<crate::config_registry::DesiredCfgEntry> {
                            if (ce.0.component_kind)() == kind_id {
                                let v = ctx.current_config_json();
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

    /// Run until Ctrl-C (SIGINT). Suitable for most dev runs: one line to start and block.
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
    /// 适合新手的一键运行：默认配置启动，直到 Ctrl+C 退出
    pub async fn run_default() -> Result<()> {
        let mut app = Self::new_default();
        app.start().await?;
        #[cfg(feature = "bus-metrics")]
        {
            // 给运行一小段时间以建立订阅
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        tokio::signal::ctrl_c().await.context("wait ctrl-c")?;
        app.stop().await;
        Ok(())
    }
    /// 适合新手的一键运行：注入一次性初始配置，直到 Ctrl+C 退出
    pub async fn run_with_config<T: serde::Serialize>(cfg: T) -> Result<()> {
        let mut app = Self::new_default();
        app.config(cfg).await?;
        app.start().await?;
        tokio::signal::ctrl_c().await.context("wait ctrl-c")?;
        app.stop().await;
        Ok(())
    }
}
