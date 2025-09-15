use crate::bus::BusHandle;
use crate::error::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, Ordering};
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    sync::Arc,
};
use tokio::sync::Notify;

#[async_trait]
pub trait Component: Send + Sync + 'static + Any {
    async fn run(self: Box<Self>, ctx: ComponentContext) -> Result<()>;
}

impl dyn Component {}

/// 组件工厂：用于注册与构造组件（单例，无需 id/kind 概念）
#[async_trait]
pub trait ComponentFactory: Send + Sync {
    fn type_name(&self) -> &'static str;
    async fn build(&self, bus: BusHandle) -> crate::error::Result<Box<dyn Component>>;
}

impl fmt::Debug for dyn ComponentFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ComponentFactory(..)")
    }
}

pub type DynFactory = Arc<dyn ComponentFactory>;

pub struct __RegisteredFactory {
    pub create: fn() -> Box<dyn ComponentFactory>,
}
inventory::collect!(__RegisteredFactory);

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
}

pub struct StopFlag {
    set: AtomicBool,
    notify: Notify,
}
impl StopFlag {
    pub(crate) fn new() -> Self {
        Self {
            set: AtomicBool::new(false),
            notify: Notify::new(),
        }
    }
    pub(crate) fn trigger(&self) {
        if !self.set.swap(true, Ordering::Release) {
            self.notify.notify_waiters();
        }
    }
    pub(crate) fn is_set(&self) -> bool {
        self.set.load(Ordering::Acquire)
    }
}

pub struct ComponentContext {
    name: String,
    bus: BusHandle,
    stop: Arc<StopFlag>,
    cfg: ConfigStore,
}

impl ComponentContext {
    // 组件标识符仅供运行期内部使用；不对业务暴露寻址能力
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn new_with_service(
        name: String,
        bus: BusHandle,
        stop: Arc<StopFlag>,
        cfg: ConfigStore,
    ) -> Self {
        Self {
            name,
            bus,
            stop,
            cfg,
        }
    }

    // 仅保留单一构造路径，避免歧义；组件以 kind 进行类型化

    // 发布采用“返回值即发布”模型（由宏注入的内部助手完成）
    // 仅支持强类型通道（&T），不提供 Any 装配

    // 配置不支持热更新：仅在启动时注入一次

    #[doc(hidden)]
    pub(crate) fn __fork(&self) -> Self {
        Self {
            name: self.name.clone(),
            bus: self.bus.clone(),
            stop: self.stop.clone(),
            cfg: self.cfg.clone(),
        }
    }
}

// 配置注入：仅在 #[init] 中通过 &CfgType 注入一次；运行期只读，由组件自身持有

/// 订阅封装（不含协作停机）
pub struct AutoSubscription<T> {
    inner: crate::bus::Subscription<T>,
}
impl<T> AutoSubscription<T> {
    pub async fn recv(&mut self) -> Option<std::sync::Arc<T>> {
        self.inner.recv().await
    }
}

// 设计约束：Context 为只读，不提供副作用或协作停机 API（详见文档）

// 内部宏辅助 API（不对业务暴露）
// 订阅：仅类型级（任意来源）

pub async fn __subscribe_any_auto<T: Send + Sync + 'static>(
    ctx: &ComponentContext,
) -> AutoSubscription<T> {
    let sub = ctx.bus.subscribe_type::<T>().await;
    AutoSubscription { inner: sub }
}

// 发布：仅由宏在返回值场景调用；不对业务暴露
pub async fn __publish_auto<T: Send + Sync + 'static>(ctx: &ComponentContext, msg: T) {
    ctx.bus.publish_type(msg).await;
}

/// 内部配置读取：仅供宏使用，防止业务侧滥用
pub fn __get_config<T: 'static + Send + Sync>(ctx: &ComponentContext) -> Option<Arc<T>> {
    let tid = TypeId::of::<T>();
    ctx.cfg
        .inner
        .get(&tid)
        .and_then(|v| v.clone().downcast::<T>().ok())
}

// ConfigStore 附加辅助方法已删除（简化）。

/// 内部停止信号（仅供宏生成的 run() 使用）
pub async fn __recv_stop(ctx: &ComponentContext) {
    if ctx.stop.is_set() {
        return;
    }
    ctx.stop.notify.notified().await;
}

// 框架内部可见：用于 App 停机触发
pub(crate) fn __trigger_stop(ctx: &ComponentContext) {
    ctx.stop.trigger();
}

pub(crate) fn __new_stop_flag() -> Arc<StopFlag> {
    Arc::new(StopFlag::new())
}
pub(crate) fn __trigger_stop_flag(flag: &Arc<StopFlag>) {
    flag.trigger();
}
