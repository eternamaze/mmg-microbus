use crate::bus::BusHandle;
use crate::error::Result;
use async_trait::async_trait;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::{any::Any, fmt, sync::Arc};
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
    bus: BusHandle,
    stop: Arc<StopFlag>,
    startup: Arc<StartupBarrier>,
}

impl ComponentContext {
    pub const fn new_with_service(
        bus: BusHandle,
        stop: Arc<StopFlag>,
        startup: Arc<StartupBarrier>,
    ) -> Self {
        Self { bus, stop, startup }
    }

    // 仅保留单一构造路径，避免歧义；组件以 kind 进行类型化

    // 发布采用“返回值即发布”模型（由宏注入的内部助手完成）
    // 仅支持强类型通道（&T），不提供 Any 装配；配置不支持热更新

    #[doc(hidden)]
    #[must_use]
    pub fn __fork(&self) -> Self {
        Self {
            bus: self.bus.clone(),
            stop: self.stop.clone(),
            startup: self.startup.clone(),
        }
    }
}

// 外部配置注入模型已移除：组件自管内部初始化，不支持 #[init](&Cfg)

/// 订阅封装（不含协作停机）
pub struct AutoSubscription<T> {
    inner: crate::bus::Subscription<T>,
}
impl<T: Send + Sync + 'static> AutoSubscription<T> {
    pub async fn recv(&mut self) -> Option<std::sync::Arc<T>> {
        self.inner.recv().await
    }
}

// 设计约束：Context 为只读，不提供副作用或协作停机 API（详见文档）

// 内部宏辅助 API（不对业务暴露）
// 订阅：仅类型级（任意来源）

#[must_use]
pub fn __subscribe_any_auto<T: Send + Sync + 'static>(
    ctx: &ComponentContext,
) -> AutoSubscription<T> {
    let sub = ctx.bus.subscribe_type::<T>();
    AutoSubscription { inner: sub }
}

// 发布：仅由宏在返回值场景调用；不对业务暴露
pub async fn __publish_auto<T: Send + Sync + 'static>(ctx: &ComponentContext, msg: T) {
    ctx.bus.publish_type(msg).await;
}

// 配置相关能力已移除：init 仅由组件自身内部逻辑决定，其它注入路径删除。

/// 内部停止信号（仅供宏生成的 `run()` 使用）
pub async fn __recv_stop(ctx: &ComponentContext) {
    if ctx.stop.is_set() {
        return;
    }
    ctx.stop.notify.notified().await;
}

pub(crate) fn __new_stop_flag() -> Arc<StopFlag> {
    Arc::new(StopFlag::new())
}
pub(crate) fn __trigger_stop_flag(flag: &Arc<StopFlag>) {
    flag.trigger();
}

// 启动屏障：确保 active(once) 发布在所有组件完成订阅后才发生，避免竞态丢失一次性消息
pub struct StartupBarrier {
    total: usize,
    arrived: AtomicUsize,
    notify: Notify,
    failed: AtomicBool,
}
impl StartupBarrier {
    #[must_use]
    pub fn new(total: usize) -> Self {
        Self {
            total,
            arrived: AtomicUsize::new(0),
            notify: Notify::new(),
            failed: AtomicBool::new(false),
        }
    }
    #[inline]
    fn is_ready(&self) -> bool {
        self.arrived.load(Ordering::Acquire) >= self.total || self.failed.load(Ordering::Acquire)
    }

    async fn wait_ready(&self) {
        while !self.is_ready() {
            self.notify.notified().await;
        }
    }

    async fn arrive_and_wait(&self) {
        let n = self.arrived.fetch_add(1, Ordering::AcqRel) + 1;
        if n == self.total {
            self.notify.notify_waiters();
            return;
        }
        self.wait_ready().await;
    }
    pub fn mark_failed(&self) {
        if !self.failed.swap(true, Ordering::AcqRel) {
            self.notify.notify_waiters();
        }
    }
    pub fn is_failed(&self) -> bool {
        self.failed.load(Ordering::Acquire)
    }
    pub async fn wait_all(&self) {
        self.wait_ready().await;
    }
}

pub(crate) fn __new_startup_barrier(total: usize) -> Arc<StartupBarrier> {
    Arc::new(StartupBarrier::new(total))
}
pub async fn __startup_arrive_and_wait(ctx: &ComponentContext) {
    ctx.startup.arrive_and_wait().await;
}

pub fn __startup_mark_failed(ctx: &ComponentContext) {
    ctx.startup.mark_failed();
}
pub fn __startup_mark_failed_barrier(b: &Arc<StartupBarrier>) {
    b.mark_failed();
}
pub async fn __startup_wait_all(b: &Arc<StartupBarrier>) {
    b.wait_all().await;
}
pub fn __startup_failed(b: &Arc<StartupBarrier>) -> bool {
    b.is_failed()
}
