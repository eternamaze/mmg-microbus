use crate::bus::{BusHandle, ComponentId, KindId};
use async_trait::async_trait;
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    sync::Arc,
};
use tokio::sync::watch;

// 组件标识由 bus.rs 定义为强类型 ComponentId

#[async_trait]
pub trait Component: Send + Sync + 'static + Any {
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

/// 组件工厂：用于注册与构造组件
#[async_trait]
pub trait ComponentFactory: Send + Sync {
    fn kind_id(&self) -> KindId;
    fn type_name(&self) -> &'static str;
    /// 基础构建器（无业务配置）
    async fn build(&self, id: ComponentId, bus: BusHandle) -> anyhow::Result<Box<dyn Component>>;
}

impl fmt::Debug for dyn ComponentFactory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ComponentFactory(..)")
    }
}

pub type DynFactory = Arc<dyn ComponentFactory>;

/// 标记 trait：由 #[component] 结构体实现，用于暴露工厂（无需全局扫描）
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
    bus: BusHandle,
    // 停止信号：仅框架内部使用，不向业务暴露协作停机接口
    stop: watch::Receiver<bool>,
    cfg: ConfigStore,
}

impl ComponentContext {
    // 组件标识符仅供运行期内部使用；不对业务暴露寻址能力
    pub fn id(&self) -> &crate::bus::ComponentId { &self.id }
    pub fn new_with_service(
        id: ComponentId,
        _service: KindId,
        bus: BusHandle,
        stop: watch::Receiver<bool>,
        cfg: ConfigStore,
    ) -> Self {
        Self {
            id,
            bus,
            stop,
            cfg,
        }
    }

    // 仅保留单一构造路径，避免歧义；组件以 kind 进行类型化

    // 对外发布 API 已移除：业务通过返回值进行发布（由宏注入内部助手完成）
    // 仅支持强类型通道（&T），不提供 Any 装配

    // 配置不支持热更新：仅在启动时注入一次

    #[doc(hidden)]
    pub(crate) fn __fork(&self) -> Self {
        Self {
            id: self.id.clone(),
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
    pub async fn recv(&mut self) -> Option<std::sync::Arc<T>> { self.inner.recv().await }
}

// 设计约束：Context 为只读，不提供副作用或协作停机 API（详见文档）

// 内部宏辅助 API（不对业务暴露）
// 订阅：仅类型级（任意来源）

    #[doc(hidden)]
    pub async fn __subscribe_any_auto<T: Send + Sync + 'static>(ctx: &ComponentContext) -> AutoSubscription<T> {
        let sub = ctx.bus.subscribe_type::<T>().await;
        AutoSubscription { inner: sub }
    }

// 发布：仅由宏在返回值场景调用；不对业务暴露
#[doc(hidden)]
pub async fn __publish_auto<T: Send + Sync + 'static>(ctx: &ComponentContext, msg: T) {
    ctx.bus.publish_type(msg).await;
}

/// 内部配置读取：仅供宏使用，防止业务侧滥用
#[doc(hidden)]
pub fn __get_config<T: 'static + Send + Sync>(ctx: &ComponentContext) -> Option<Arc<T>> {
    ctx.cfg.get::<T>()
}

/// 内部停止信号（仅供宏生成的 run() 使用）
#[doc(hidden)]
pub async fn __recv_stop(ctx: &ComponentContext) {
    let mut rx = ctx.stop.clone();
    let _ = rx.changed().await;
}
