use crate::bus::{BusHandle, ComponentId, KindId, ScopedBus, ServiceAddr, ServicePattern};
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use std::{any::Any, fmt, sync::Arc};
use tokio::sync::watch;

// 已由 bus.rs 定义强类型 ComponentId

#[async_trait]
pub trait Component: Send + Sync + 'static + Any {
    fn id(&self) -> &ComponentId;
    async fn run(self: Box<Self>, ctx: ComponentContext) -> anyhow::Result<()>;
}

impl dyn Component {
    pub fn as_any(&self) -> &dyn Any {
        self
    }
    pub fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Factory for components so they can be registered and constructed.
#[async_trait]
pub trait ComponentFactory: Send + Sync {
    fn kind_id(&self) -> KindId;
    fn type_name(&self) -> &'static str;
    /// Optional: provide default config for zero-config instantiation (JSON value for serialization compatibility).
    fn default_config(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    /// Optional: parse raw JSON config into a type-erased boxed value understood by this factory.
    fn parse_config(
        &self,
        v: Option<serde_json::Value>,
    ) -> anyhow::Result<Box<dyn Any + Send + Sync>> {
        Ok(Box::new(v.unwrap_or(serde_json::Value::Null)))
    }
    /// Basic builder without config.
    async fn build(&self, id: ComponentId, bus: BusHandle) -> anyhow::Result<Box<dyn Component>>;
    /// Optional: build with parsed type-erased config; default calls build() and ignores config.
    async fn build_with_config(
        &self,
        id: ComponentId,
        bus: BusHandle,
        _cfg: Box<dyn Any + Send + Sync>,
    ) -> anyhow::Result<Box<dyn Component>> {
        self.build(id, bus).await
    }
}

impl fmt::Debug for dyn ComponentFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ComponentFactory(..)")
    }
}

pub type DynFactory = Arc<dyn ComponentFactory>;

pub struct ComponentContext {
    pub id: crate::bus::ComponentId,
    pub self_addr: ServiceAddr,
    pub scoped_bus: ScopedBus,
    pub shutdown: watch::Receiver<bool>,
    pub config_rx: watch::Receiver<serde_json::Value>,
}

impl ComponentContext {
    pub fn new_with_service(
        id: ComponentId,
        service: KindId,
        bus: BusHandle,
        shutdown: watch::Receiver<bool>,
        config_rx: watch::Receiver<serde_json::Value>,
    ) -> Self {
        let self_addr = ServiceAddr {
            service,
            instance: id.clone(),
        };
        let scoped_bus = ScopedBus::new(bus, self_addr.clone());
        Self {
            id,
            self_addr,
            scoped_bus,
            shutdown,
            config_rx,
        }
    }

    // Keep a single constructor to avoid confusion; components are always typed by kind.

    pub async fn subscribe_from<T: Send + Sync + 'static>(
        &self,
        from: &ServiceAddr,
    ) -> crate::bus::Subscription<T> {
        self.scoped_bus.subscribe_from::<T>(from).await
    }
    pub async fn subscribe_pattern<T: Send + Sync + 'static>(
        &self,
        pattern: ServicePattern,
    ) -> crate::bus::Subscription<T> {
        self.scoped_bus.subscribe_pattern::<T>(pattern).await
    }
    pub async fn publish<T: Send + Sync + 'static>(&self, msg: T) {
        self.scoped_bus.publish(msg).await;
    }
    pub async fn publish_enveloped<T: Send + Sync + 'static>(&self, msg: T) {
        self.scoped_bus.publish_enveloped(msg).await;
    }
    // 仅提供强类型通道（Envelope<T>/T），不提供 AnyEnvelope 自动装配通道。

    // -------- Config helpers --------
    pub fn current_config_json(&self) -> serde_json::Value {
        self.config_rx.borrow().clone()
    }
    pub fn current_config_as<T: DeserializeOwned>(&self) -> anyhow::Result<T> {
        let v = self.current_config_json();
        Ok(serde_json::from_value::<T>(v)?)
    }
    pub async fn wait_config_change(&mut self) -> Option<serde_json::Value> {
        if self.config_rx.changed().await.is_ok() {
            Some(self.config_rx.borrow().clone())
        } else {
            None
        }
    }
}

// ---- 配置注入上下文与契约 ----
#[derive(Clone)]
pub struct ConfigContext {
    pub id: ComponentId,
    pub self_addr: ServiceAddr,
    pub scoped_bus: ScopedBus,
}
impl ConfigContext {
    pub fn new(id: ComponentId, service: KindId, scoped_bus: ScopedBus) -> Self {
        let self_addr = ServiceAddr {
            service,
            instance: id.clone(),
        };
        Self {
            id,
            self_addr,
            scoped_bus,
        }
    }
    pub fn from_component_ctx(c: &ComponentContext) -> Self {
        Self {
            id: c.id.clone(),
            self_addr: c.self_addr.clone(),
            scoped_bus: c.scoped_bus.clone(),
        }
    }
}

#[async_trait]
pub trait Configure<C>: Send + Sync {
    async fn on_config(&mut self, ctx: &ConfigContext, cfg: C) -> anyhow::Result<()>;
}

pub trait ConfigApplyDyn {
    fn apply<'a>(
        &'a mut self,
        ctx: ConfigContext,
        v: serde_json::Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>>;
}
