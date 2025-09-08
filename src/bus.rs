#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ComponentId(pub String);
use smallvec::SmallVec;

use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    hash::Hash,
    sync::Arc,
};
use tokio::sync::mpsc;
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

// 地址模型已移除：仅按消息类型进行路由

pub struct Subscription<T> {
    rx: mpsc::Receiver<Arc<T>>,
}
impl<T> Subscription<T> {
    pub async fn recv(&mut self) -> Option<Arc<T>> {
        self.rx.recv().await
    }
}

// 订阅索引：仅类型级（any-of-type）
struct TypeIndex<T: Send + Sync + 'static> {
    any: SmallVec<[mpsc::Sender<Arc<T>>; 4]>,
}
impl<T: Send + Sync + 'static> Default for TypeIndex<T> {
    fn default() -> Self { Self { any: SmallVec::new() } }
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

// 路由索引：仅类型级（any-of-type），不支持实例级寻址

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
    pub(crate) async fn subscribe_type<T: Send + Sync + 'static>(&self) -> Subscription<T> {
        let mut subs = self.inner.subs.write();
        let cap = self.inner.default_capacity;
        let type_id = TypeId::of::<T>();
        let entry = subs
            .entry(type_id)
            .or_insert_with(|| Box::new(TypeIndex::<T>::default()) as Box<dyn Any + Send + Sync>);
        let (tx, rx) = mpsc::channel::<Arc<T>>(cap);
        let idx = entry.downcast_mut::<TypeIndex<T>>().expect("type index");
        idx.any.push(tx);
        Subscription { rx }
    }
    // 内部发送实现（统一入口）
    pub(crate) async fn publish_type<T: Send + Sync + 'static>(&self, msg: T) {
        let type_id = TypeId::of::<T>();
        let arc = Arc::new(msg);
        let senders: Vec<mpsc::Sender<Arc<T>>> = {
            let subs = self.inner.subs.read();
            if let Some(entry) = subs.get(&type_id) {
                if let Some(idx) = entry.downcast_ref::<TypeIndex<T>>() {
                    let mut v: Vec<mpsc::Sender<Arc<T>>> = Vec::new();
                    v.extend(idx.any.iter().cloned());
                    v
                } else {
                    tracing::error!("type mismatch in type index for this type");
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
    // 单订阅者快路径：避免不必要的分配
        if total == 1 {
            // 找到唯一仍打开的接收者
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
    // 多订阅者：收集仍然开启的接收者（转移所有权以便 try_send）
    let mut recipients: SmallVec<[mpsc::Sender<Arc<T>>; 8]> = SmallVec::new();
    // 依据 total 预留容量，减少重分配
    recipients.reserve(total);
        for tx in &senders {
            if !tx.is_closed() {
                recipients.push(tx.clone());
            }
        }
        if recipients.is_empty() { return; }
    // try_send 快路径：优先非阻塞发送，剩余的再 await
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
    // 发布接口不对外暴露；仅供宏生成代码内部使用

    #[cfg(test)]
    pub async fn debug_count_subscribers<T: Send + Sync + 'static>(&self) -> usize {
        let type_id = TypeId::of::<T>();
        let subs = self.inner.subs.read();
        if let Some(entry) = subs.get(&type_id) {
            if let Some(idx) = entry.downcast_ref::<TypeIndex<T>>() {
                return idx.any.iter().filter(|tx| !tx.is_closed()).count();
            }
        }
        0
    }
}

// 地址模型：已移除；仅类型级路由

// 背压策略：有界 mpsc + try_send 优先，必要时 await；单订阅者快路径；SmallVec 降低分配成本。

// 指标采集已移除：保持总线实现最小化

// 内部单元测试省略：由集成测试覆盖
