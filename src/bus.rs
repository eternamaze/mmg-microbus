use smallvec::SmallVec;

use parking_lot::RwLock;
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
    sync::Arc,
};
use tokio::sync::mpsc;

// Small helper alias used across functions
type SenderVec<T> = SmallVec<[mpsc::Sender<Arc<T>>; 8]>;

// 类型级 fanout 路由（按消息类型广播，不做拓扑/主题分层）

pub struct Subscription<T> {
    rx: mpsc::Receiver<Arc<T>>,
}
impl<T> Subscription<T> {
    pub async fn recv(&mut self) -> Option<Arc<T>>
    where
        T: Send + Sync + 'static,
    {
        self.rx.recv().await
    }
}

// 订阅索引：仅类型级；启动完成后不再增删（冻结）
struct TypeIndex<T: Send + Sync + 'static> {
    any: SmallVec<[mpsc::Sender<Arc<T>>; 4]>,
}
impl<T: Send + Sync + 'static> Default for TypeIndex<T> {
    fn default() -> Self {
        Self {
            any: SmallVec::new(),
        }
    }
}

#[derive(Clone)]
pub struct BusHandle {
    inner: Arc<BusInner>,
}

struct BusInner {
    subs: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    default_capacity: usize,
    sealed: AtomicBool, // 一旦置 true，订阅结构视为只读
}

impl fmt::Debug for BusHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BusHandle").finish()
    }
}

// 路由索引：类型级（any-of-type）

pub struct Bus {
    handle: BusHandle,
}
impl Bus {
    #[must_use]
    pub fn new(default_capacity: usize) -> Self {
        let inner = BusInner {
            subs: RwLock::new(HashMap::new()),
            default_capacity,
            sealed: AtomicBool::new(false),
        };
        Self {
            handle: BusHandle {
                inner: Arc::new(inner),
            },
        }
    }
    #[must_use]
    pub fn handle(&self) -> BusHandle {
        self.handle.clone()
    }
}

impl BusHandle {
    pub(crate) fn subscribe_type<T: Send + Sync + 'static>(&self) -> Subscription<T> {
        if self.inner.sealed.load(Ordering::Acquire) {
            tracing::warn!("subscribe called after bus sealed (late task start); accepting for compatibility with startup race");
        }
        let cap = self.inner.default_capacity;
        let type_id = TypeId::of::<T>();
        let (tx_local, rx) = mpsc::channel::<Arc<T>>(cap);
        {
            let mut map = self.inner.subs.write();
            let entry = map.entry(type_id).or_insert_with(|| {
                Box::new(TypeIndex::<T>::default()) as Box<dyn Any + Send + Sync>
            });
            if let Some(idx) = (**entry).downcast_mut::<TypeIndex<T>>() {
                idx.any.push(tx_local);
            } else {
                tracing::error!("type index downcast failed; subscription ignored");
            }
            // drop lock early
            drop(map);
        }
        Subscription { rx }
    }
    // 内部发送实现（统一入口）
    pub(crate) async fn publish_type<T: Send + Sync + 'static>(&self, msg: T) {
        // 顺序语义：同一类型的消息进入每个订阅者通道的顺序=各 publish 调用实际完成入队的顺序；无全局跨组件开播时间排序保证。
        let type_id = TypeId::of::<T>();
        let arc = Arc::new(msg);
        // 读取订阅快照（只读锁期间不执行 await）
        let (open_count, idx_any): (usize, Option<SenderVec<T>>) = {
            let subs = self.inner.subs.read();
            subs.get(&type_id).map_or_else(
                || (0, None),
                |entry| {
                    (**entry).downcast_ref::<TypeIndex<T>>().map_or_else(
                        || {
                            tracing::error!("type mismatch in type index for this type");
                            (0, None)
                        },
                        |idx| {
                            // 单次遍历统计并复制打开的发送端；通常订阅者很少，SmallVec 足够
                            let mut opened: SenderVec<T> = SmallVec::new();
                            for tx in &idx.any {
                                if !tx.is_closed() {
                                    opened.push(tx.clone());
                                }
                            }
                            let c = opened.len();
                            (c, Some(opened))
                        },
                    )
                },
            )
        };
        if open_count == 0 {
            return;
        }
        // 单订阅者快路径
        let Some(senders) = idx_any else {
            return;
        };
        if open_count == 1 {
            let tx = &senders[0];
            match tx.try_send(arc.clone()) {
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    let _ = tx.send(arc).await;
                    return;
                }
                Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
            }
        }
        // 多订阅者：先 try_send，满的收集到 pending 再顺序 await（最后一次复用 arc）
        let mut pending: SmallVec<[mpsc::Sender<Arc<T>>; 8]> = SmallVec::new();
        for tx in &senders {
            match tx.try_send(arc.clone()) {
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => pending.push(tx.clone()),
                Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
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
    // 发布接口：仅供宏生成代码内部使用

    #[cfg(test)]
    #[must_use]
    pub fn debug_count_subscribers<T: Send + Sync + 'static>(&self) -> usize {
        let type_id = TypeId::of::<T>();
        let subs = self.inner.subs.read();
        subs.get(&type_id)
            .and_then(|entry| (**entry).downcast_ref::<TypeIndex<T>>())
            .map_or(0, |idx| idx.any.iter().filter(|tx| !tx.is_closed()).count())
    }
}

impl BusHandle {
    pub(crate) fn seal(&self) {
        self.inner.sealed.store(true, Ordering::Release);
    }
}

// 路由模型：类型级 fanout

// 背压策略：有界 mpsc + try_send 优先，必要时 await；单订阅者快路径；SmallVec 降低分配成本。

// 实现保持最小化（无内部指标采集）

// 内部单元测试省略：由集成测试覆盖
