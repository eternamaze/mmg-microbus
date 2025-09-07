#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ComponentId(pub String);
use smallvec::SmallVec;
use std::sync::atomic::Ordering;
//
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};
use tokio::sync::{mpsc, watch, RwLock};
//

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
    pub fn any() -> Self {
        Self {
            service: None,
            instance: None,
        }
    }
    pub fn for_kind<T: 'static>() -> Self {
        Self {
            service: Some(KindId::of::<T>()),
            instance: None,
        }
    }
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
    fn matches(&self, addr: &ServiceAddr) -> bool {
        (self
            .service
            .as_ref()
            .map(|s| s == &addr.service)
            .unwrap_or(true))
            && (self
                .instance
                .as_ref()
                .map(|i| i == &addr.instance)
                .unwrap_or(true))
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

type TopicMap = HashMap<TypeId, Box<dyn Any + Send + Sync>>;
type ServiceTopics = HashMap<ServiceAddr, TopicMap>;
type PatternHandlers = Vec<Box<dyn Any + Send + Sync>>;

#[derive(Clone)]
pub struct BusHandle {
    inner: Arc<BusInner>,
}

struct BusInner {
    topics: RwLock<ServiceTopics>,
    patterns: RwLock<HashMap<TypeId, PatternHandlers>>,
    default_capacity: usize,
    paused: std::sync::atomic::AtomicBool,
    resume_notify: tokio::sync::Notify,
}

impl fmt::Debug for BusHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BusHandle").finish()
    }
}
impl BusHandle {
    pub fn pause(&self) {
        self.inner.paused.store(true, Ordering::SeqCst);
    }
    pub fn resume(&self) {
        self.inner.paused.store(false, Ordering::SeqCst);
        self.inner.resume_notify.notify_waiters();
    }
}

struct Topic<T: Send + Sync + 'static> {
    txs: SmallVec<[mpsc::Sender<Arc<T>>; 4]>,
}
struct PatternTopic<T: Send + Sync + 'static> {
    pattern: Address,
    txs: SmallVec<[mpsc::Sender<Arc<T>>; 4]>,
}

pub struct Bus {
    handle: BusHandle,
}
impl Bus {
    pub fn new(default_capacity: usize) -> Self {
        let inner = BusInner {
            topics: RwLock::new(HashMap::new()),
            patterns: RwLock::new(HashMap::new()),
            default_capacity,
            paused: std::sync::atomic::AtomicBool::new(false),
            resume_notify: tokio::sync::Notify::new(),
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
        let exact = match from.require_exact() {
            Some(x) => x,
            None => {
                tracing::error!("subscribe<T> requires exact Address (service+instance)");
                let (_tx, rx) = mpsc::channel::<Arc<T>>(self.inner.default_capacity);
                return Subscription { rx };
            }
        };
        let mut topics = self.inner.topics.write().await;
        let cap = self.inner.default_capacity;
        let type_id = TypeId::of::<T>();
        let typed_map = topics.entry(exact.clone()).or_insert_with(HashMap::new);
        let entry = typed_map.entry(type_id).or_insert_with(|| {
            Box::new(Topic::<T> {
                txs: SmallVec::new(),
            }) as Box<dyn Any + Send + Sync>
        });
        let t = match entry.downcast_mut::<Topic<T>>() {
            Some(t) => t,
            None => {
                tracing::error!(
                    "type mismatch in subscribe<T>: service/instance has a different message type"
                );
                // 返回一个空订阅，避免运行期 panic；调用方将 recv() 到 None
                let (_tx, rx) = mpsc::channel::<Arc<T>>(cap);
                return Subscription { rx };
            }
        };
        let (tx, rx) = mpsc::channel::<Arc<T>>(cap);
        t.txs.push(tx);
        Subscription { rx }
    }
    pub async fn subscribe_pattern<T: Send + Sync + 'static>(
        &self,
        pattern: Address,
    ) -> Subscription<T> {
        let mut patterns = self.inner.patterns.write().await;
        let cap = self.inner.default_capacity;
        let type_id = TypeId::of::<T>();
        let list = patterns.entry(type_id).or_insert_with(Vec::new);
        let (tx, rx) = mpsc::channel::<Arc<T>>(cap);
        list.push(Box::new(PatternTopic::<T> {
            pattern,
            txs: SmallVec::from_vec(vec![tx]),
        }) as Box<dyn Any + Send + Sync>);
        Subscription { rx }
    }
    // 内部发送实现（统一入口）
    async fn publish_inner<T: Send + Sync + 'static>(&self, from: &ServiceAddr, msg: T) {
        while self.inner.paused.load(Ordering::SeqCst) {
            self.inner.resume_notify.notified().await;
        }
        let type_id = TypeId::of::<T>();
        let arc = Arc::new(msg);
        let senders_exact: Option<Vec<mpsc::Sender<Arc<T>>>> = {
            let topics = self.inner.topics.read().await;
            if let Some(typed_map) = topics.get(from) {
                if let Some(entry) = typed_map.get(&type_id) {
                    if let Some(t) = entry.downcast_ref::<Topic<T>>() {
                        Some(t.txs.to_vec())
                    } else {
                        tracing::error!("type mismatch in publish<T>: service/instance has a different message type");
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };
        let senders_patterns: Vec<mpsc::Sender<Arc<T>>> = {
            let patterns = self.inner.patterns.read().await;
            if let Some(list) = patterns.get(&type_id) {
                let mut all = Vec::new();
                for entry in list {
                    let pt = entry
                        .downcast_ref::<PatternTopic<T>>()
                        .unwrap_or_else(|| panic!("type mismatch in pattern list for this type"));
                    if pt.pattern.matches(from) {
                        all.extend(pt.txs.iter().cloned());
                    }
                }
                all
            } else {
                Vec::new()
            }
        };
        let mut total = 0usize;
        if let Some(s) = &senders_exact {
            total += s.iter().filter(|tx| !tx.is_closed()).count();
        }
        total += senders_patterns.iter().filter(|tx| !tx.is_closed()).count();
        if total == 0 {
            return;
        }
        // 单一路径：阻塞发送（不丢包）。保持最小必要队列，抵抗短暂抖动。
        // total==1 快路径：不分配 recipients 容器
        if total == 1 {
            if let Some(s) = &senders_exact {
                if let Some(tx) = s.iter().find(|tx| !tx.is_closed()) {
                    let _ = tx.send(arc).await;
                    return;
                }
            }
            // exact 没找到就一定在 patterns 中
            if let Some(tx) = senders_patterns.iter().find(|tx| !tx.is_closed()) {
                let _ = tx.send(arc).await;
            }
        } else {
            // 统一收集所有仍然开启的接收者，最后一个使用 move，其余 clone，避免多余 Arc 克隆
            let mut recipients: SmallVec<[&mpsc::Sender<Arc<T>>; 8]> = SmallVec::new();
            recipients.reserve(total);
            if let Some(s) = &senders_exact {
                for tx in s.iter() {
                    if !tx.is_closed() {
                        recipients.push(tx);
                    }
                }
            }
            for tx in &senders_patterns {
                if !tx.is_closed() {
                    recipients.push(tx);
                }
            }
            debug_assert_eq!(recipients.len(), total);
            // 前 total-1 个 clone 发送，最后一个 move 发送
            for i in 0..(total - 1) {
                let _ = recipients[i].send(arc.clone()).await;
            }
            let _ = recipients[total - 1].send(arc).await;
        }
        // 按需清理：仅当检测到 sender 关闭或数量变化时进入写锁
        let mut need_clean_topics = false;
        let mut need_clean_patterns = false;
        {
            if let Some(s) = &senders_exact {
                if s.iter().any(|tx| tx.is_closed()) {
                    need_clean_topics = true;
                }
            }
            if senders_patterns.iter().any(|tx| tx.is_closed()) {
                need_clean_patterns = true;
            }
        }
        if need_clean_topics {
            let mut topics = self.inner.topics.write().await;
            let mut remove_type = false;
            let mut remove_service = false;
            if let Some(typed_map) = topics.get_mut(from) {
                if let Some(entry) = typed_map.get_mut(&type_id) {
                    if let Some(t) = entry.downcast_mut::<Topic<T>>() {
                        t.txs.retain(|tx| !tx.is_closed());
                        if t.txs.is_empty() {
                            remove_type = true;
                        }
                    }
                }
                if remove_type {
                    typed_map.remove(&type_id);
                }
                if typed_map.is_empty() {
                    remove_service = true;
                }
            }
            if remove_service {
                topics.remove(from);
            }
        }
        if need_clean_patterns {
            let mut patterns = self.inner.patterns.write().await;
            if let Some(list) = patterns.get_mut(&type_id) {
                let mut new_list: Vec<Box<dyn Any + Send + Sync>> = Vec::with_capacity(list.len());
                for mut entry in list.drain(..) {
                    let keep = if let Some(ptm) = entry.downcast_mut::<PatternTopic<T>>() {
                        ptm.txs.retain(|tx| !tx.is_closed());
                        !ptm.txs.is_empty()
                    } else {
                        false
                    };
                    if keep {
                        new_list.push(entry);
                    }
                }
                if new_list.is_empty() {
                    patterns.remove(&type_id);
                } else {
                    *list = new_list;
                }
            }
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
    pub async fn debug_count_subscribers<T: Send + Sync + 'static>(&self) -> (usize, usize) {
        let type_id = TypeId::of::<T>();
        let topics = self.inner.topics.read().await;
        let mut exact = 0usize;
        for (_addr, typed_map) in topics.iter() {
            if let Some(entry) = typed_map.get(&type_id) {
                if let Some(t) = entry.downcast_ref::<Topic<T>>() {
                    exact += t.txs.iter().filter(|tx| !tx.is_closed()).count();
                }
            }
        }
        drop(topics);
        let patterns = self.inner.patterns.read().await;
        let mut pat = 0usize;
        if let Some(list) = patterns.get(&type_id) {
            for entry in list {
                if let Some(pt) = entry.downcast_ref::<PatternTopic<T>>() {
                    pat += pt.txs.iter().filter(|tx| !tx.is_closed()).count();
                }
            }
        }
        (exact, pat)
    }
}

// 统一地址模型：一个类型同时表示精确地址与模式（通过 Option 实现）

// Backpressure 已移除：总线采用阻塞发送实现，确保不丢包（在队列容量范围内）。

// 已移除总线指标功能：简化核心总线实现。

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn wildcard_and_pattern_work() {
        let bus = Bus::new(8);
        let h = bus.handle();
        struct S;
        struct A;
        struct B;
        impl InstanceMarker for A {
            fn id() -> &'static str {
                "a"
            }
        }
        impl InstanceMarker for B {
            fn id() -> &'static str {
                "b"
            }
        }
        let a_exact = ServiceAddr::of_instance::<S, A>();
        let b_exact = ServiceAddr::of_instance::<S, B>();
        #[derive(Clone, Debug)]
        struct Evt(u32);
        let mut sub = h
            .subscribe_pattern::<Evt>(Address {
                service: Some(KindId::of::<S>()),
                instance: None,
            })
            .await;
        h.publish(
            &Address {
                service: Some(KindId::of::<S>()),
                instance: Some(a_exact.instance.clone()),
            },
            Evt(1),
        )
        .await;
        h.publish(
            &Address {
                service: Some(KindId::of::<S>()),
                instance: Some(b_exact.instance.clone()),
            },
            Evt(2),
        )
        .await;
        let x = sub.recv().await.unwrap();
        let y = sub.recv().await.unwrap();
        assert!(matches!((x.0, y.0), (1, 2) | (2, 1)));
        // exact subscribe still works but is considered internal; here we check pattern only.
    }
}
