use crate::bus::{Address, BusHandle, ComponentId, KindId, ServiceAddr};
use async_trait::async_trait;
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    sync::Arc,
};
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

/// Marker trait implemented by #[component] structs to expose their factory without global scanning.
pub trait RegisteredComponent {
    fn kind_id() -> KindId
    where
        Self: Sized;
    fn type_name() -> &'static str
    where
        Self: Sized;
    fn factory() -> DynFactory
    where
        Self: Sized;
}

// 只读配置存储：在 App::start 时冻结，运行期只读访问
#[derive(Clone)]
pub struct ConfigStore {
    inner: Arc<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}
impl ConfigStore {
    pub fn empty() -> Self {
        Self {
            inner: Arc::new(HashMap::new()),
        }
    }
    pub fn from_frozen_map(map: HashMap<TypeId, Arc<dyn Any + Send + Sync>>) -> Self {
        Self {
            inner: Arc::new(map),
        }
    }
    pub fn get<T: 'static + Send + Sync>(&self) -> Option<Arc<T>> {
        let tid = TypeId::of::<T>();
        self.inner
            .get(&tid)
            .and_then(|v| v.clone().downcast::<T>().ok())
    }
}

pub struct ComponentContext {
    id: crate::bus::ComponentId,
    self_addr: ServiceAddr,
    bus: BusHandle,
    shutdown: watch::Receiver<bool>,
    cfg: ConfigStore,
}

impl ComponentContext {
    /// Component unique id
    pub fn id(&self) -> &crate::bus::ComponentId {
        &self.id
    }
    /// Component strong-typed service address
    pub fn service_addr(&self) -> &ServiceAddr {
        &self.self_addr
    }
    /// Access a frozen config object by type
    pub fn config<T: 'static + Send + Sync>(&self) -> Option<Arc<T>> {
        self.cfg.get::<T>()
    }
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

    // 删除对外订阅 API：业务方不应直接访问框架订阅。宏使用下面的 crate 可见函数。
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

    #[doc(hidden)]
    pub(crate) fn __fork(&self) -> Self {
        Self {
            id: self.id.clone(),
            self_addr: self.self_addr.clone(),
            bus: self.bus.clone(),
            shutdown: self.shutdown.clone(),
            cfg: self.cfg.clone(),
        }
    }
}

// ---- 配置注入说明 ----
// 配置仅在 #[init] 中通过 &CfgType 注入一次；运行时只读，由组件状态自行持有与使用。

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

    /// Create an auto-shutdown ticker. `tick().await` returns None when the App stops.
    pub fn ticker(&self, dur: std::time::Duration) -> AutoTicker {
        AutoTicker {
            intv: tokio::time::interval(dur),
            shutdown: self.shutdown.clone(),
        }
    }

    /// Spawn a task that will be aborted automatically when App stops.
    /// This avoids wiring shutdown checks inside business logic.
    pub fn spawn_until_shutdown<F>(&self, fut: F) -> tokio::task::JoinHandle<()>
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let mut sd = self.shutdown.clone();
        tokio::spawn(async move {
            // Run future in its own task so we can abort it on shutdown
            let mut handle = tokio::spawn(fut);
            tokio::select! {
                _ = sd.changed() => {
                    handle.abort();
                    let _ = handle.await; // ignore error on abort
                }
                _ = &mut handle => {
                    // inner completed; nothing else to do
                }
            }
        })
    }
}

/// A ticker that stops automatically when App shutdown is signaled.
pub struct AutoTicker {
    intv: tokio::time::Interval,
    shutdown: watch::Receiver<bool>,
}
impl AutoTicker {
    /// Wait for next tick; returns None when App is stopping.
    pub async fn tick(&mut self) -> Option<()> {
        let mut sd = self.shutdown.clone();
        tokio::select! {
            _ = sd.changed() => None,
            _ = self.intv.tick() => Some(()),
        }
    }
}

// (no extra runtime glue needed: macro根据返回类型生成自动发布代码)

// ---- crate 内部宏辅助 API（不暴露给业务）----
#[doc(hidden)]
pub async fn __subscribe_pattern_auto<T: Send + Sync + 'static>(
    ctx: &ComponentContext,
    pattern: Address,
) -> AutoSubscription<T> {
    let sub = ctx.bus.subscribe_pattern::<T>(pattern).await;
    AutoSubscription {
        inner: sub,
        shutdown: ctx.shutdown.clone(),
    }
}
