use mmg_microbus::prelude::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Boot(pub usize);

static ACTIVE_CALLS: AtomicUsize = AtomicUsize::new(0);
static RECEIVED: AtomicUsize = AtomicUsize::new(0);

#[mmg_microbus::component]
#[derive(Default)]
struct Booter;

#[mmg_microbus::component]
impl Booter {
    #[mmg_microbus::active(once)]
    async fn boot(&self) -> Boot {
        let n = ACTIVE_CALLS.fetch_add(1, Ordering::SeqCst);
        Boot(n)
    }
}

#[mmg_microbus::component]
#[derive(Default)]
struct Collector;

#[mmg_microbus::component]
impl Collector {
    #[mmg_microbus::handle]
    async fn on_boot(&self, _ctx: &mmg_microbus::component::ComponentContext, b: &Boot) {
        // 只应收到一次，b.0 应为 0
        let _ = b.0;
        RECEIVED.fetch_add(1, Ordering::SeqCst);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn active_once_executes_exactly_once() {
    let mut app = App::new(Default::default());
    app.add_component::<Booter>("booter");
    app.add_component::<Collector>("collector");
    app.start().await.expect("start");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    app.stop().await;
    assert_eq!(
        ACTIVE_CALLS.load(Ordering::SeqCst),
        1,
        "active(once) should run exactly once"
    );
    assert_eq!(
        RECEIVED.load(Ordering::SeqCst),
        1,
        "message from active(once) should be published exactly once"
    );
}
