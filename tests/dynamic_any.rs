use mmg_microbus::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug)]
struct A(pub u64);
#[derive(Clone, Debug)]
struct B(pub u64);
#[derive(Clone, Debug)]
struct C(pub u64);
#[derive(Clone, Debug)]
struct D(pub u64);

static SEQ: AtomicUsize = AtomicUsize::new(0);
static SEEN_A: AtomicUsize = AtomicUsize::new(0);
static SEEN_B: AtomicUsize = AtomicUsize::new(0);
static SEEN_C: AtomicUsize = AtomicUsize::new(0);
static SEEN_D: AtomicUsize = AtomicUsize::new(0);

#[mmg_microbus::component]
#[derive(Default)]
struct DynProducer;

#[mmg_microbus::component]
impl DynProducer {
    // 动态 Any 路径：交替返回 A / B
    #[mmg_microbus::active]
    async fn any_stream(&self) -> Box<dyn std::any::Any + Send + Sync> {
        let n = SEQ.fetch_add(1, Ordering::Relaxed) as u64;
        if n.is_multiple_of(2) {
            Box::new(A(n))
        } else {
            Box::new(B(n))
        }
    }

    // ErasedEvent 路径：每次发布 C 与 D（Vec<ErasedEvent>）
    #[mmg_microbus::active]
    async fn erased_stream(&self) -> Vec<mmg_microbus::bus::ErasedEvent> {
        let n = SEQ.fetch_add(1, Ordering::Relaxed) as u64;
        vec![
            mmg_microbus::bus::ErasedEvent::new(C(n)),
            mmg_microbus::bus::ErasedEvent::new(D(n)),
        ]
    }
}

#[mmg_microbus::component]
#[derive(Default)]
struct DynCollector;

#[mmg_microbus::component]
impl DynCollector {
    #[mmg_microbus::handle]
    async fn on_a(&self, _ctx: &mmg_microbus::component::ComponentContext, _x: &A) {
        let _ = _x.0;
        SEEN_A.fetch_add(1, Ordering::Relaxed);
    }
    #[mmg_microbus::handle]
    async fn on_b(&self, _ctx: &mmg_microbus::component::ComponentContext, _x: &B) {
        let _ = _x.0;
        SEEN_B.fetch_add(1, Ordering::Relaxed);
    }
    #[mmg_microbus::handle]
    async fn on_c(&self, _ctx: &mmg_microbus::component::ComponentContext, _x: &C) {
        let _ = _x.0;
        SEEN_C.fetch_add(1, Ordering::Relaxed);
    }
    #[mmg_microbus::handle]
    async fn on_d(&self, _ctx: &mmg_microbus::component::ComponentContext, _x: &D) {
        let _ = _x.0;
        SEEN_D.fetch_add(1, Ordering::Relaxed);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dynamic_any_and_erased_publish() {
    let mut app = App::new(mmg_microbus::config::AppConfig::default());
    app.start().await.expect("start");
    // 允许运行一段时间以积累事件
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    app.stop();
    assert!(SEEN_A.load(Ordering::Relaxed) > 0, "A not received");
    assert!(SEEN_B.load(Ordering::Relaxed) > 0, "B not received");
    assert!(SEEN_C.load(Ordering::Relaxed) > 0, "C not received");
    assert!(SEEN_D.load(Ordering::Relaxed) > 0, "D not received");
}
