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

// 订阅索引：类型级。
// - 启动阶段（未封印）：累积订阅到 `any`。
// - 封印后：惰性构建不可变快照 `frozen_any`，发布阶段直接使用该快照，避免每次发布克隆 sender 与小分配。
struct TypeIndex<T: Send + Sync + 'static> {
    any: SmallVec<[mpsc::Sender<Arc<T>>; 4]>,
    frozen_any: Option<std::sync::Arc<[mpsc::Sender<Arc<T>>]>>,
}
impl<T: Send + Sync + 'static> Default for TypeIndex<T> {
    fn default() -> Self {
        Self {
            any: SmallVec::new(),
            frozen_any: None,
        }
    }
}

// 类型擦除条目：允许在 seal() 时统一冻结，而在泛型路径下仍可做具体类型的 downcast。
trait TypeIndexEntry: Any + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn freeze(&mut self);
}
impl<T: Send + Sync + 'static> TypeIndexEntry for TypeIndex<T> {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn freeze(&mut self) {
        if self.frozen_any.is_none() {
            let small = std::mem::take(&mut self.any);
            let vec = small.into_vec();
            self.frozen_any = Some(Arc::<[mpsc::Sender<Arc<T>>]>::from(vec));
        }
    }
}

#[derive(Clone)]
pub struct BusHandle {
    inner: Arc<BusInner>,
}

struct BusInner {
    subs: RwLock<HashMap<TypeId, Box<dyn TypeIndexEntry>>>,
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
    #[inline]
    fn is_sealed(&self) -> bool {
        self.inner.sealed.load(Ordering::Acquire)
    }
    #[inline]
    async fn send_one<T: Send + Sync + 'static>(&self, tx: &mpsc::Sender<Arc<T>>, arc: Arc<T>) {
        match tx.try_send(arc.clone()) {
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                let _ = tx.send(arc).await;
            }
            Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
        }
    }

    #[inline]
    async fn send_pending_by_index<T: Send + Sync + 'static>(
        senders: &[mpsc::Sender<Arc<T>>],
        pending_idx: &[usize],
        arc: Arc<T>,
    ) {
        if pending_idx.is_empty() {
            return;
        }
        let last = pending_idx.len() - 1;
        for &i in &pending_idx[..last] {
            let _ = senders[i].send(arc.clone()).await;
        }
        let _ = senders[pending_idx[last]].send(arc).await;
    }

    #[inline]
    fn get_frozen_senders<T: Send + Sync + 'static>(
        &self,
        type_id: TypeId,
    ) -> Option<Arc<[mpsc::Sender<Arc<T>>]>> {
        let subs = self.inner.subs.read();
        subs.get(&type_id)
            .and_then(|entry| entry.as_any().downcast_ref::<TypeIndex<T>>())
            .and_then(|idx| idx.frozen_any.clone())
    }

    #[inline]
    fn get_open_senders_unsealed<T: Send + Sync + 'static>(&self, type_id: TypeId) -> SenderVec<T> {
        let mut opened: SenderVec<T> = SmallVec::new();
        if let Some(entry) = self.inner.subs.read().get(&type_id) {
            if let Some(idx) = entry.as_any().downcast_ref::<TypeIndex<T>>() {
                for tx in &idx.any {
                    if !tx.is_closed() {
                        opened.push(tx.clone());
                    }
                }
            } else {
                tracing::error!("type mismatch in type index for this type");
            }
        }
        opened
    }
    pub(crate) fn subscribe_type<T: Send + Sync + 'static>(&self) -> Subscription<T> {
        assert!(
            !self.inner.sealed.load(Ordering::Acquire),
            "subscribe_type called after bus sealed: subscription graph is immutable after startup"
        );
        let cap = self.inner.default_capacity;
        let type_id = TypeId::of::<T>();
        let (tx_local, rx) = mpsc::channel::<Arc<T>>(cap);
        if let Some(entry) = self
            .inner
            .subs
            .write()
            .entry(type_id)
            .or_insert_with(|| Box::<TypeIndex<T>>::default() as Box<dyn TypeIndexEntry>)
            .as_any_mut()
            .downcast_mut::<TypeIndex<T>>()
        {
            entry.any.push(tx_local);
        } else {
            tracing::error!("type index downcast failed; subscription ignored");
        }
        Subscription { rx }
    }
    // 内部发送实现（统一入口）
    pub(crate) async fn publish_type<T: Send + Sync + 'static>(&self, msg: T) {
        // 顺序语义：同一类型的消息进入每个订阅者通道的顺序=各 publish 调用实际完成入队的顺序；无全局跨组件开播时间排序保证。
        let type_id = TypeId::of::<T>();
        let arc = Arc::new(msg);
        if self.is_sealed() {
            self.publish_type_sealed::<T>(type_id, arc).await;
        } else {
            self.publish_type_unsealed::<T>(type_id, arc).await;
        }
    }

    async fn publish_type_sealed<T: Send + Sync + 'static>(&self, type_id: TypeId, arc: Arc<T>) {
        if let Some(frozen) = self.get_frozen_senders::<T>(type_id) {
            self.publish_to_senders(&frozen, arc).await;
        }
    }

    async fn publish_type_unsealed<T: Send + Sync + 'static>(&self, type_id: TypeId, arc: Arc<T>) {
        let senders = self.get_open_senders_unsealed::<T>(type_id);
        self.publish_to_senders(&senders, arc).await;
    }

    #[inline]
    fn try_send_collect_pending<T: Send + Sync + 'static>(
        senders: &[mpsc::Sender<Arc<T>>],
        arc: &Arc<T>,
    ) -> SmallVec<[usize; 8]> {
        let mut pending_idx: SmallVec<[usize; 8]> = SmallVec::new();
        for (i, tx) in senders.iter().enumerate() {
            match tx.try_send(arc.clone()) {
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => pending_idx.push(i),
                Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {}
            }
        }
        pending_idx
    }

    #[inline]
    async fn publish_to_senders<T: Send + Sync + 'static>(
        &self,
        senders: &[mpsc::Sender<Arc<T>>],
        arc: Arc<T>,
    ) {
        match senders.len() {
            0 => {}
            1 => {
                self.send_one(&senders[0], arc).await;
            }
            _ => {
                let pending_idx = Self::try_send_collect_pending(senders, &arc);
                Self::send_pending_by_index::<T>(senders, &pending_idx, arc).await;
            }
        }
    }
    // 发布接口：仅供宏生成代码内部使用

    #[cfg(test)]
    #[must_use]
    pub(crate) fn debug_count_subscribers<T: Send + Sync + 'static>(&self) -> usize {
        let type_id = TypeId::of::<T>();
        let subs = self.inner.subs.read();
        subs.get(&type_id)
            .and_then(|entry| entry.as_any().downcast_ref::<TypeIndex<T>>())
            .map_or(0, |idx| idx.any.iter().filter(|tx| !tx.is_closed()).count())
    }
}

impl BusHandle {
    pub(crate) fn seal(&self) {
        // 在封印前冻结所有已知类型的订阅快照，确保运行期发布路径无需惰性构建。
        let mut subs = self.inner.subs.write();
        for (_, entry) in subs.iter_mut() {
            entry.freeze();
        }
        drop(subs);
        self.inner.sealed.store(true, Ordering::Release);
    }
}

// 路由模型：类型级 fanout

// 背压策略：有界 mpsc + try_send 优先，必要时 await；单订阅者快路径；SmallVec 降低分配成本。

// 实现保持最小化（无内部指标采集）

// 内部单元测试省略：由集成测试覆盖

#[cfg(test)]
mod perf_tests {
    use std::time::Instant;
    use tokio::task::JoinSet;

    #[derive(Debug)]
    struct Msg(u64);

    async fn run_once(n_subs: usize, msgs: u64) -> u128 {
        let bus = crate::bus::Bus::new(4096);
        let handle = bus.handle();

        // 订阅者：每个订阅者消费 msgs 条消息
        let mut join = JoinSet::new();
        for _ in 0..n_subs {
            let mut sub = handle.subscribe_type::<Msg>();
            join.spawn(async move {
                let mut c = 0u64;
                while c < msgs {
                    match sub.recv().await {
                        Some(msg) => {
                            // 读取字段并通过 black_box 防止被优化掉
                            std::hint::black_box(msg.0);
                            c += 1;
                        }
                        None => break,
                    }
                }
                c
            });
        }

        // 测试环境下验证订阅者计数工具，避免 dead_code 且校验预期的订阅规模
        assert_eq!(handle.debug_count_subscribers::<Msg>(), n_subs);

        // 封印后进入发布快路径
        handle.seal();

        let start = Instant::now();
        for i in 0..msgs {
            handle.publish_type(Msg(i)).await;
        }
        // 等全部订阅者完成
        let mut total = 0u64;
        while let Some(res) = join.join_next().await {
            total += res.expect("task join");
        }
        assert_eq!(total, msgs * n_subs as u64);
        start.elapsed().as_micros()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn perf_publish_sealed() {
        // 多组订阅者规模下的粗略性能采样
        let msgs = 20_000u64;
        for &subs in &[1usize, 4, 8] {
            let us = run_once(subs, msgs).await;
            let total_msgs = msgs * subs as u64;
            // 以整数进行每秒吞吐计算，避免浮点精度丢失
            let mps: u128 = (u128::from(total_msgs) * 1_000_000u128) / us;
            eprintln!(
                "sealed publish: subs={subs} msgs={msgs} elapsed={us}us throughput={mps} msg/s"
            );
        }
    }
}
