use crate::bus::{Address, BusHandle, ComponentId, KindId, ServiceAddr};
use async_trait::async_trait;
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
    /// Basic builder without config.
    async fn build(&self, id: ComponentId, bus: BusHandle) -> anyhow::Result<Box<dyn Component>>;
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
    pub bus: BusHandle,
    pub shutdown: watch::Receiver<bool>,
}

impl ComponentContext {
    pub fn new_with_service(
        id: ComponentId,
        service: KindId,
        bus: BusHandle,
        shutdown: watch::Receiver<bool>,
    ) -> Self {
        let self_addr = ServiceAddr {
            service,
            instance: id.clone(),
        };
        Self {
            id,
            self_addr,
            bus,
            shutdown,
        }
    }

    // Keep a single constructor to avoid confusion; components are always typed by kind.

    pub async fn subscribe_from<T: Send + Sync + 'static>(
        &self,
        from: &Address,
    ) -> crate::bus::Subscription<T> {
        self.bus.subscribe::<T>(from).await
    }
    pub async fn subscribe_pattern<T: Send + Sync + 'static>(
        &self,
        pattern: Address,
    ) -> crate::bus::Subscription<T> {
        self.bus.subscribe_pattern::<T>(pattern).await
    }
    pub async fn publish<T: Send + Sync + 'static>(&self, msg: T) {
        let me = Address {
            service: Some(self.self_addr.service),
            instance: Some(self.self_addr.instance.clone()),
        };
        self.bus.publish(&me, msg).await;
    }
    pub async fn publish_from<T: Send + Sync + 'static>(&self, from: &Address, msg: T) {
        self.bus.publish(from, msg).await;
    }
    // 仅提供强类型通道（&T），不提供 Any 自动装配通道。

    // 不再提供配置热更新辅助：配置仅在启动时一次性注入
}

// ---- 配置注入上下文与契约 ----
#[derive(Clone)]
pub struct ConfigContext {
    pub id: ComponentId,
    pub self_addr: ServiceAddr,
}
impl ConfigContext {
    pub fn new(id: ComponentId, service: KindId) -> Self {
        let self_addr = ServiceAddr {
            service,
            instance: id.clone(),
        };
        Self { id, self_addr }
    }
    pub fn from_component_ctx(c: &ComponentContext) -> Self {
        Self {
            id: c.id.clone(),
            self_addr: c.self_addr.clone(),
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
        v: Arc<dyn Any + Send + Sync>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>>;
}
