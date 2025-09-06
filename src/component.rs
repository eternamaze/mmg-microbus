use crate::bus::{Address, BusHandle, ComponentId, KindId, ServiceAddr};
use async_trait::async_trait;
use std::{any::{Any, TypeId}, fmt, sync::Arc, collections::HashMap};
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

// 只读配置存储：在 App::start 时冻结，运行期只读访问
#[derive(Clone)]
pub struct ConfigStore {
    inner: Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}
impl ConfigStore {
    pub fn empty() -> Self { Self { inner: Arc::new(HashMap::new()) } }
    pub fn from_frozen_map(map: HashMap<TypeId, Arc<dyn Any + Send + Sync>>) -> Self {
        Self { inner: Arc::new(map) }
    }
    pub fn get<T: 'static + Send + Sync>(&self) -> Option<Arc<T>> {
        let tid = TypeId::of::<T>();
        self.inner.get(&tid).and_then(|v| v.clone().downcast::<T>().ok())
    }
}

pub struct ComponentContext {
    pub id: crate::bus::ComponentId,
    pub self_addr: ServiceAddr,
    pub bus: BusHandle,
    pub shutdown: watch::Receiver<bool>,
    pub cfg: ConfigStore,
}

impl ComponentContext {
    pub fn new_with_service(
        id: ComponentId,
        service: KindId,
        bus: BusHandle,
        shutdown: watch::Receiver<bool>,
        cfg: ConfigStore,
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
            cfg,
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
    /// Subscribe and get an auto-shutdown subscription: recv will end when App stops.
    pub async fn subscribe_from_auto<T: Send + Sync + 'static>(
        &self,
        from: &Address,
    ) -> AutoSubscription<T> {
        let sub = self.bus.subscribe::<T>(from).await;
        AutoSubscription { inner: sub, shutdown: self.shutdown.clone() }
    }
    /// Pattern subscribe with auto-shutdown behavior.
    pub async fn subscribe_pattern_auto<T: Send + Sync + 'static>(
        &self,
        pattern: Address,
    ) -> AutoSubscription<T> {
        let sub = self.bus.subscribe_pattern::<T>(pattern).await;
        AutoSubscription { inner: sub, shutdown: self.shutdown.clone() }
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

    // 不再提供配置热更新：配置仅在启动时一次性注入
}

// ---- 配置注入上下文与契约 ----
// 取消旧的配置回调契约：改为在 handler 签名中通过 &ConfigType 参数进行注入

/// A subscription wrapper that automatically treats App shutdown as stream end.
pub struct AutoSubscription<T> {
    inner: crate::bus::Subscription<T>,
    shutdown: watch::Receiver<bool>,
}
impl<T> AutoSubscription<T> {
    pub async fn recv(&mut self) -> Option<std::sync::Arc<T>> {
        self.inner.recv_or_shutdown(&self.shutdown).await
    }
}

impl ComponentContext {
    /// Sleep with graceful shutdown: returns early when shutdown is signaled.
    pub async fn graceful_sleep(&self, dur: std::time::Duration) {
        let mut sd = self.shutdown.clone();
        tokio::select! {
            _ = sd.changed() => {}
            _ = tokio::time::sleep(dur) => {}
        }
    }
}
