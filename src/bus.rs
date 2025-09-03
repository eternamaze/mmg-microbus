#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ComponentId(pub String);
use smallvec::SmallVec;
#[cfg(feature = "bus-metrics")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
#[cfg(feature = "bus-metrics")]
use std::time::Instant;
use std::time::SystemTime;
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    fmt,
    hash::{Hash, Hasher},
    sync::Arc,
};
use tokio::sync::{mpsc, RwLock};
#[cfg(feature = "bus-metrics")]
use tokio::time::Duration;
use uuid::Uuid;

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
pub struct ServicePattern {
    pub service: Option<KindId>,
    pub instance: Option<ComponentId>,
}
impl ServicePattern {
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
    pub fn matches(&self, addr: &ServiceAddr) -> bool {
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
impl ServicePattern {
    pub fn for_instance_marker<C: 'static, I: InstanceMarker>() -> Self {
        Self {
            service: Some(KindId::of::<C>()),
            instance: Some(ComponentId(I::id().to_string())),
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
    #[cfg(feature = "bus-metrics")]
    metrics: Option<Arc<BusMetrics>>,
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
    pattern: ServicePattern,
    txs: SmallVec<[mpsc::Sender<Arc<T>>; 4]>,
}

pub struct Bus {
    handle: BusHandle,
}
impl Bus {
    #[cfg(feature = "bus-metrics")]
    pub fn new(default_capacity: usize, metrics: Option<Arc<BusMetrics>>) -> Self {
        let inner = BusInner {
            topics: RwLock::new(HashMap::new()),
            patterns: RwLock::new(HashMap::new()),
            default_capacity,
            metrics,
            paused: std::sync::atomic::AtomicBool::new(false),
            resume_notify: tokio::sync::Notify::new(),
        };
        Self {
            handle: BusHandle {
                inner: Arc::new(inner),
            },
        }
    }
    #[cfg(not(feature = "bus-metrics"))]
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
    pub async fn subscribe<T: Send + Sync + 'static>(&self, from: &ServiceAddr) -> Subscription<T> {
        let mut topics = self.inner.topics.write().await;
        let cap = self.inner.default_capacity;
        let type_id = TypeId::of::<T>();
        let typed_map = topics.entry(from.clone()).or_insert_with(HashMap::new);
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
        pattern: ServicePattern,
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
    pub async fn publish<T: Send + Sync + 'static>(&self, from: &ServiceAddr, msg: T) {
        while self.inner.paused.load(Ordering::SeqCst) {
            #[cfg(feature = "bus-metrics")]
            {
                let t0 = Instant::now();
                self.inner.resume_notify.notified().await;
                if let Some(m) = &self.inner.metrics {
                    m.record_pause(t0.elapsed());
                }
            }
            #[cfg(not(feature = "bus-metrics"))]
            {
                self.inner.resume_notify.notified().await;
            }
        }
        let type_id = TypeId::of::<T>();
        let arc = Arc::new(msg);
        #[cfg(feature = "bus-metrics")]
        if let Some(m) = &self.inner.metrics {
            m.published.fetch_add(1, Ordering::Relaxed);
        }
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
        #[cfg(feature = "bus-metrics")]
        if let Some(m) = &self.inner.metrics {
            m.record_fanout(total);
            m.inc_inflight();
        }
        // 单一路径：阻塞发送（不丢包）。保持最小必要队列，抵抗短暂抖动。
        #[cfg(feature = "bus-metrics")]
        let start = self.inner.metrics.as_ref().map(|_| Instant::now());
        #[cfg(feature = "bus-metrics")]
        let mut delivered = 0;
        // total==1 快路径：不分配 recipients 容器
        if total == 1 {
            if let Some(s) = &senders_exact {
                if let Some(tx) = s.iter().find(|tx| !tx.is_closed()) {
                    if tx.send(arc).await.is_ok() {
                        #[cfg(feature = "bus-metrics")]
                        {
                            delivered += 1;
                        }
                    }
                    #[cfg(feature = "bus-metrics")]
                    if let Some(m) = &self.inner.metrics {
                        m.delivered.fetch_add(delivered, Ordering::Relaxed);
                        if let Some(s) = start {
                            m.record_latency(s.elapsed());
                        }
                        m.dec_inflight();
                    }
                    return;
                }
            }
            // exact 没找到就一定在 patterns 中
            if let Some(tx) = senders_patterns.iter().find(|tx| !tx.is_closed()) {
                if tx.send(arc).await.is_ok() {
                    #[cfg(feature = "bus-metrics")]
                    {
                        delivered += 1;
                    }
                }
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
                if recipients[i].send(arc.clone()).await.is_ok() {
                    #[cfg(feature = "bus-metrics")]
                    {
                        delivered += 1;
                    }
                }
            }
            if recipients[total - 1].send(arc).await.is_ok() {
                #[cfg(feature = "bus-metrics")]
                {
                    delivered += 1;
                }
            }
        }
        #[cfg(feature = "bus-metrics")]
        if let Some(m) = &self.inner.metrics {
            m.delivered.fetch_add(delivered, Ordering::Relaxed);
            if let Some(s) = start {
                m.record_latency(s.elapsed());
            }
            m.dec_inflight();
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
            #[cfg(feature = "bus-metrics")]
            if let Some(m) = &self.inner.metrics {
                m.pruned.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
    pub async fn publish_enveloped<T: Send + Sync + 'static>(
        &self,
        from: &ServiceAddr,
        payload: T,
        trace_id: Option<Uuid>,
    ) {
        while self.inner.paused.load(Ordering::SeqCst) {
            self.inner.resume_notify.notified().await;
        }
        let env = Envelope {
            origin: from.clone(),
            time: SystemTime::now(),
            trace_id,
            msg: payload,
        };
        self.publish::<Envelope<T>>(from, env).await;
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

#[derive(Clone)]
pub struct ScopedBus {
    handle: BusHandle,
    pub self_addr: ServiceAddr,
}
impl ScopedBus {
    pub fn new(handle: BusHandle, self_addr: ServiceAddr) -> Self {
        Self { handle, self_addr }
    }
    pub async fn publish<T: Send + Sync + 'static>(&self, msg: T) {
        self.handle.publish(&self.self_addr, msg).await;
    }
    pub async fn publish_enveloped<T: Send + Sync + 'static>(&self, msg: T) {
        self.handle
            .publish_enveloped(&self.self_addr, msg, None)
            .await;
    }
    pub async fn subscribe_from<T: Send + Sync + 'static>(
        &self,
        from: &ServiceAddr,
    ) -> Subscription<T> {
        self.handle.subscribe::<T>(from).await
    }
    pub async fn subscribe_self<T: Send + Sync + 'static>(&self) -> Subscription<T> {
        self.handle.subscribe::<T>(&self.self_addr).await
    }
    pub async fn subscribe_pattern<T: Send + Sync + 'static>(
        &self,
        pattern: ServicePattern,
    ) -> Subscription<T> {
        self.handle.subscribe_pattern::<T>(pattern).await
    }

    /// 从标记类型来源发布（类型安全实例 ID）
    pub async fn publish_from_marker<S: 'static, I: InstanceMarker, T: Send + Sync + 'static>(
        &self,
        msg: T,
    ) {
        let from = ServiceAddr::of_instance::<S, I>();
        self.handle.publish(&from, msg).await;
    }
    pub async fn publish_from_marker_enveloped<
        S: 'static,
        I: InstanceMarker,
        T: Send + Sync + 'static,
    >(
        &self,
        msg: T,
    ) {
        let from = ServiceAddr::of_instance::<S, I>();
        self.handle.publish_enveloped(&from, msg, None).await;
    }
}

#[derive(Clone, Debug)]
pub struct Envelope<T: Send + Sync + 'static> {
    pub origin: ServiceAddr,
    pub time: SystemTime,
    pub trace_id: Option<Uuid>,
    pub msg: T,
}

// Backpressure 已移除：总线采用阻塞发送实现，确保不丢包（在队列容量范围内）。

#[cfg(feature = "bus-metrics")]
pub struct BusMetrics {
    published: AtomicU64,
    delivered: AtomicU64,
    pruned: AtomicU64,
    hist_ms: [AtomicU64; 12],
    inflight: AtomicU64,
    max_inflight: AtomicU64,
    pause_waits: AtomicU64,
    pause_hist_ms: [AtomicU64; 8],
    fanout_hist: [AtomicU64; 8],
}
#[cfg(feature = "bus-metrics")]
impl Default for BusMetrics {
    fn default() -> Self {
        fn a() -> AtomicU64 {
            AtomicU64::new(0)
        }
        Self {
            published: a(),
            delivered: a(),
            pruned: a(),
            hist_ms: [a(), a(), a(), a(), a(), a(), a(), a(), a(), a(), a(), a()],
            inflight: a(),
            max_inflight: a(),
            pause_waits: a(),
            pause_hist_ms: [a(), a(), a(), a(), a(), a(), a(), a()],
            fanout_hist: [a(), a(), a(), a(), a(), a(), a(), a()],
        }
    }
}
#[cfg(feature = "bus-metrics")]
impl BusMetrics {
    pub fn new() -> Self {
        Self::default()
    }
    fn bucket_idx_ms(ms: u128) -> usize {
        match ms {
            0 => 0,
            1 => 1,
            2 => 2,
            3..=5 => 3,
            6..=10 => 4,
            11..=20 => 5,
            21..=50 => 6,
            51..=100 => 7,
            101..=200 => 8,
            201..=500 => 9,
            501..=1000 => 10,
            _ => 11,
        }
    }
    pub fn record_latency(&self, dur: Duration) {
        let ms = dur.as_millis();
        let idx = Self::bucket_idx_ms(ms);
        self.hist_ms[idx].fetch_add(1, Ordering::Relaxed);
    }
    fn bucket_idx_pause(ms: u128) -> usize {
        match ms {
            0 => 0,
            1..=2 => 1,
            3..=5 => 2,
            6..=10 => 3,
            11..=20 => 4,
            21..=50 => 5,
            51..=100 => 6,
            _ => 7,
        }
    }
    pub fn record_pause(&self, dur: Duration) {
        let ms = dur.as_millis();
        let idx = Self::bucket_idx_pause(ms);
        self.pause_waits.fetch_add(1, Ordering::Relaxed);
        self.pause_hist_ms[idx].fetch_add(1, Ordering::Relaxed);
    }
    fn bucket_idx_fanout(n: usize) -> usize {
        match n {
            0 => 0,
            1 => 1,
            2 => 2,
            3..=5 => 3,
            6..=10 => 4,
            11..=20 => 5,
            21..=50 => 6,
            _ => 7,
        }
    }
    pub fn record_fanout(&self, n: usize) {
        let idx = Self::bucket_idx_fanout(n);
        self.fanout_hist[idx].fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_inflight(&self) {
        let now = self.inflight.fetch_add(1, Ordering::Relaxed) + 1;
        // 简单的最大值更新（无锁）
        let mut cur_max = self.max_inflight.load(Ordering::Relaxed);
        while now > cur_max {
            match self.max_inflight.compare_exchange(
                cur_max,
                now,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(v) => cur_max = v,
            }
        }
    }
    pub fn dec_inflight(&self) {
        self.inflight.fetch_sub(1, Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> BusMetricsSnapshot {
        let mut hist = [0u64; 12];
        for i in 0..12 {
            hist[i] = self.hist_ms[i].load(Ordering::Relaxed);
        }
        let mut pause = [0u64; 8];
        for i in 0..8 {
            pause[i] = self.pause_hist_ms[i].load(Ordering::Relaxed);
        }
        let mut fan = [0u64; 8];
        for i in 0..8 {
            fan[i] = self.fanout_hist[i].load(Ordering::Relaxed);
        }
        BusMetricsSnapshot {
            published: self.published.load(Ordering::Relaxed),
            delivered: self.delivered.load(Ordering::Relaxed),
            pruned: self.pruned.load(Ordering::Relaxed),
            hist_ms: hist,
            inflight: self.inflight.load(Ordering::Relaxed),
            max_inflight: self.max_inflight.load(Ordering::Relaxed),
            pause_waits: self.pause_waits.load(Ordering::Relaxed),
            pause_hist_ms: pause,
            fanout_hist: fan,
        }
    }
}

#[cfg(feature = "bus-metrics")]
#[derive(Clone, Copy, Debug)]
pub struct BusMetricsSnapshot {
    pub published: u64,
    pub delivered: u64,
    pub pruned: u64,
    pub hist_ms: [u64; 12],
    pub inflight: u64,
    pub max_inflight: u64,
    pub pause_waits: u64,
    pub pause_hist_ms: [u64; 8],
    pub fanout_hist: [u64; 8],
}
#[cfg(feature = "bus-metrics")]
impl BusHandle {
    pub fn metrics_snapshot(&self) -> Option<BusMetricsSnapshot> {
        self.inner.metrics.as_ref().map(|m| m.snapshot())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn wildcard_and_envelope_work() {
        #[cfg(feature = "bus-metrics")]
        let bus = Bus::new(8, None);
        #[cfg(not(feature = "bus-metrics"))]
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
        let a = ServiceAddr::of_instance::<S, A>();
        let b = ServiceAddr::of_instance::<S, B>();
        #[derive(Clone, Debug)]
        struct Evt(u32);
        let mut sub = h
            .subscribe_pattern::<Evt>(ServicePattern {
                service: Some(KindId::of::<S>()),
                instance: None,
            })
            .await;
        h.publish(&a, Evt(1)).await;
        h.publish(&b, Evt(2)).await;
        let x = sub.recv().await.unwrap();
        let y = sub.recv().await.unwrap();
        assert!(matches!((x.0, y.0), (1, 2) | (2, 1)));
        let mut sub_env = h.subscribe::<Envelope<Evt>>(&a).await;
        h.publish_enveloped(&a, Evt(9), None).await;
        let env = sub_env.recv().await.unwrap();
        assert_eq!(env.msg.0, 9);
        assert_eq!(env.origin.instance.0, "a".to_string());
    }
}
