#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ComponentId(pub String);
use smallvec::SmallVec;

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};
use tokio::sync::{mpsc, watch};
use parking_lot::RwLock;


#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct KindId(TypeId);
impl KindId {
    pub fn of<T: 'static>() -> Self {
        KindId(TypeId::of::<T>())
    }
}
impl fmt::Debug for KindId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "KindId(..)")
    }
}

#[derive(Clone, Eq)]
pub struct ServiceAddr {
    pub service: KindId,
    pub instance: ComponentId,
}
impl PartialEq for ServiceAddr {
    fn eq(&self, other: &Self) -> bool {
        self.service == other.service && self.instance == other.instance
    }
}
impl Hash for ServiceAddr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.service.hash(state);
        self.instance.hash(state);
    }
}
impl fmt::Debug for ServiceAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceAddr")
            .field("service", &self.service)
            .field("instance", &self.instance)
            .finish()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Address {
    pub service: Option<KindId>,
    pub instance: Option<ComponentId>,
}
impl Address {
    pub fn of_instance<C: 'static, I: InstanceMarker>() -> Self {
        Self {
            service: Some(KindId::of::<C>()),
            instance: Some(ComponentId(I::id().to_string())),
        }
    }
    fn require_exact(&self) -> Option<ServiceAddr> {
        match (&self.service, &self.instance) {
            (Some(s), Some(i)) => Some(ServiceAddr {
                service: *s,
                instance: i.clone(),
            }),
            _ => None,
        }
    }
    fn require_instance(&self) -> Option<ComponentId> {
        self.instance.clone()
    }
}

// 类型安全的实例标记：实现该 trait 的零尺寸类型可作为“实例 ID”，避免字符串契约与拼写错误
pub trait InstanceMarker {
    fn id() -> &'static str;
}

impl ServiceAddr {
    pub fn of_instance<C: 'static, I: InstanceMarker>() -> Self {
        ServiceAddr {
            service: KindId::of::<C>(),
            instance: ComponentId(I::id().to_string()),
        }
    }
}

pub struct Subscription<T> {
    rx: mpsc::Receiver<Arc<T>>,
}
impl<T> Subscription<T> {
    pub async fn recv(&mut self) -> Option<Arc<T>> {
        self.rx.recv().await
    }
    /// Receive next message or end on shutdown. When the shutdown receiver signals, returns None.
    pub async fn recv_or_shutdown(&mut self, shutdown: &watch::Receiver<bool>) -> Option<Arc<T>> {
        let mut sd = shutdown.clone();
        tokio::select! {
            _ = sd.changed() => {
                None
            }
            msg = self.rx.recv() => {
                msg
            }
        }
    }
}

// 订阅索引：支持类型级（any）与按实例聚合
struct InstanceIndex<T: Send + Sync + 'static> {
    any: SmallVec<[mpsc::Sender<Arc<T>>; 4]>,
    by_instance: HashMap<ComponentId, SmallVec<[mpsc::Sender<Arc<T>>; 4]>>,
}
impl<T: Send + Sync + 'static> Default for InstanceIndex<T> {
    fn default() -> Self {
        Self { any: SmallVec::new(), by_instance: HashMap::new() }
    }
}

#[derive(Clone)]
pub struct BusHandle {
    inner: Arc<BusInner>,
}

struct BusInner {
    subs: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    default_capacity: usize,
}

impl fmt::Debug for BusHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BusHandle").finish()
    }
}
impl BusHandle {}

// 路由索引：类型级(any-of-type) 与 精确实例 两级

pub struct Bus {
    handle: BusHandle,
}
impl Bus {
    pub fn new(default_capacity: usize) -> Self {
        let inner = BusInner {
            subs: RwLock::new(HashMap::new()),
            default_capacity,
        };
        Self {
            handle: BusHandle {
                inner: Arc::new(inner),
            },
        }
    }
    pub fn handle(&self) -> BusHandle {
        self.handle.clone()
    }
}

impl BusHandle {
    pub async fn subscribe<T: Send + Sync + 'static>(&self, from: &Address) -> Subscription<T> {
        let mut subs = self.inner.subs.write();
        let cap = self.inner.default_capacity;
        let type_id = TypeId::of::<T>();
        let entry = subs
            .entry(type_id)
            .or_insert_with(|| Box::new(InstanceIndex::<T>::default()) as Box<dyn Any + Send + Sync>);
        let (tx, rx) = mpsc::channel::<Arc<T>>(cap);
        let idx = entry.downcast_mut::<InstanceIndex<T>>().expect("instance index type");
        if let Some(inst) = from.require_instance() {
            idx.by_instance
                .entry(inst)
                .or_insert_with(SmallVec::new)
                .push(tx);
        } else {
            idx.any.push(tx);
        }
        Subscription { rx }
    }
    // 内部发送实现（统一入口）
    async fn publish_inner<T: Send + Sync + 'static>(&self, from: &ServiceAddr, msg: T) {
        let type_id = TypeId::of::<T>();
        let arc = Arc::new(msg);
        let senders: Vec<mpsc::Sender<Arc<T>>> = {
            let subs = self.inner.subs.read();
            if let Some(entry) = subs.get(&type_id) {
                if let Some(idx) = entry.downcast_ref::<InstanceIndex<T>>() {
                    let mut v: Vec<mpsc::Sender<Arc<T>>> = Vec::new();
                    // 类型级订阅者
                    v.extend(idx.any.iter().cloned());
                    // 指定实例订阅者
                    if let Some(inst_vec) = idx.by_instance.get(&from.instance) {
                        v.extend(inst_vec.iter().cloned());
                    }
                    v
                } else {
                    tracing::error!("type mismatch in instance index for this type");
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        };
    let total = senders.iter().filter(|tx| !tx.is_closed()).count();
        if total == 0 {
            return;
        }
        // total==1 快路径：避免额外分配
        if total == 1 {
            // 找到唯一一个仍打开的接收者
            for tx in &senders {
                if !tx.is_closed() {
                    match tx.try_send(arc.clone()) {
                        Ok(()) => return,
                        Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                            let _ = tx.send(arc).await;
                            return;
                        }
                        Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
                    }
                }
            }
            return;
        }
        // 多订阅者：收集仍然开启的接收者（拥有所有权，便于 try_send）
    let mut recipients: SmallVec<[mpsc::Sender<Arc<T>>; 8]> = SmallVec::new();
    // 依据 total 预留，减少重分配
    recipients.reserve(total);
        for tx in &senders {
            if !tx.is_closed() {
                recipients.push(tx.clone());
            }
        }
        if recipients.is_empty() { return; }
        // try_send 快路径：先尽量非阻塞发送，剩余的再 await
        let mut pending: SmallVec<[mpsc::Sender<Arc<T>>; 8]> = SmallVec::new();
        for tx in recipients.into_iter() {
            match tx.try_send(arc.clone()) {
                Ok(()) => {}
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    pending.push(tx);
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
            }
        }
        if !pending.is_empty() {
            let last = pending.len() - 1;
            for i in 0..last {
                let _ = pending[i].send(arc.clone()).await;
            }
            let _ = pending[last].send(arc).await;
        }
    }
    pub async fn publish<T: Send + Sync + 'static>(&self, from: &Address, msg: T) {
        if let Some(ex) = from.require_exact() {
            self.publish_inner::<T>(&ex, msg).await;
        } else {
            tracing::warn!("publish<T> ignored: Address must be exact (service+instance)");
        }
    }

    #[cfg(test)]
    pub async fn debug_count_subscribers<T: Send + Sync + 'static>(&self) -> usize {
        let type_id = TypeId::of::<T>();
        let subs = self.inner.subs.read();
        let mut n = 0usize;
        if let Some(entry) = subs.get(&type_id) {
            if let Some(idx) = entry.downcast_ref::<InstanceIndex<T>>() {
                for vec in idx.by_instance.values() {
                    n += vec.iter().filter(|tx| !tx.is_closed()).count();
                }
            }
        }
        n
    }
}

// 地址模型：通过 Option 表示“类型级（任意来源）”与“精确实例”两种路由目标

// 背压：基于有界 mpsc 队列 + try_send 优先，必要时 await；单订阅者快路径，小向量减少分配。

// 已移除总线指标功能：简化核心总线实现。

// (no internal tests here; covered by integration tests)
