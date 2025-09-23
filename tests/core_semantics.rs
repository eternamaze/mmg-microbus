use mmg_microbus::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tick(pub u64);
#[derive(Clone, Debug, PartialEq, Eq)]
struct Price(pub u64);
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stopped(&'static str);

#[mmg_microbus::component]
#[derive(Default)]
struct Producer;
#[mmg_microbus::component]
impl Producer {
    #[mmg_microbus::active]
    async fn tick(&self, _ctx: &mmg_microbus::component::ComponentContext) -> Tick {
        tokio::task::yield_now().await;
        Tick(PRODUCER_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
    }
}

#[mmg_microbus::component]
#[derive(Default)]
struct Trader {
    bias: u64,
}
#[mmg_microbus::component]
impl Trader {
    #[mmg_microbus::init]
    async fn init(&mut self) {
        tokio::task::yield_now().await;
        // 自行初始化需要的状态（示例：固定偏移量）
        self.bias = 1;
    }
    #[mmg_microbus::handle]
    async fn on_tick(
        &self,
        _ctx: &mmg_microbus::component::ComponentContext,
        t: &Tick,
    ) -> Option<Price> {
        tokio::task::yield_now().await;
        Some(Price(t.0 + self.bias))
    }
    #[mmg_microbus::stop]
    async fn on_stop(&self) -> Stopped {
        tokio::task::yield_now().await;
        STOP_CALLED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Stopped("bye")
    }
}

#[mmg_microbus::component]
#[derive(Default)]
struct Collector;
static SEEN_PRICE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static STOP_CALLED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PRODUCER_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[mmg_microbus::component]
impl Collector {
    #[mmg_microbus::handle]
    async fn on_price(&self, _ctx: &mmg_microbus::component::ComponentContext, p: &Price) {
        tokio::task::yield_now().await;
        SEEN_PRICE.store(p.0, std::sync::atomic::Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn end_to_end_flow_and_stop() {
    let mut app = App::new(mmg_microbus::config::AppConfig::default());
    app.start().await.expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    assert!(SEEN_PRICE.load(std::sync::atomic::Ordering::SeqCst) > 0);
    app.stop().await;
    assert!(STOP_CALLED.load(std::sync::atomic::Ordering::SeqCst) >= 1);
}
