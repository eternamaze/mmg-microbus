use mmg_microbus::prelude::*;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tick(pub u64);
#[derive(Clone, Debug, PartialEq, Eq)]
struct Price(pub u64);
#[derive(Clone, Debug, PartialEq, Eq)]
struct Cfg { n: u64 }
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stopped(&'static str);

#[mmg_microbus::component]
#[derive(Default)]
struct Producer;
#[mmg_microbus::component]
impl Producer {
  #[mmg_microbus::active]
  async fn tick(&self, _ctx: &mmg_microbus::component::ComponentContext) -> Tick { static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0); Tick(C.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) }
}

#[mmg_microbus::component]
#[derive(Default)]
struct Trader { cfg: Option<Cfg> }
#[mmg_microbus::component]
impl Trader {
  #[mmg_microbus::init]
  async fn init(&mut self, cfg: &Cfg) { self.cfg = Some(cfg.clone()); }
  #[mmg_microbus::handle]
  async fn on_tick(&mut self, _ctx: &mmg_microbus::component::ComponentContext, t: &Tick) -> Option<Price> { Some(Price(t.0 + self.cfg.as_ref().map(|c| c.n).unwrap_or(0))) }
  #[mmg_microbus::stop]
  async fn on_stop(&self) -> Stopped { STOP_CALLED.fetch_add(1, std::sync::atomic::Ordering::SeqCst); Stopped("bye") }
}

#[mmg_microbus::component]
#[derive(Default)]
struct Collector;
static SEEN_PRICE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static STOP_CALLED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
#[mmg_microbus::component]
impl Collector {
  #[mmg_microbus::handle]
  async fn on_price(&self, _ctx: &mmg_microbus::component::ComponentContext, p: &Price) { SEEN_PRICE.store(p.0, std::sync::atomic::Ordering::SeqCst); }
}

#[tokio::test(flavor = "multi_thread")]
async fn end_to_end_flow_and_stop() {
  let mut app = App::new(Default::default());
  app.add_component::<Producer>("p");
  app.add_component::<Trader>("t");
  app.add_component::<Collector>("c");
  let _ = app.config(Cfg{ n: 1 }).await.expect("config");
  app.start().await.expect("start");
  tokio::time::sleep(std::time::Duration::from_millis(80)).await;
  assert!(SEEN_PRICE.load(std::sync::atomic::Ordering::SeqCst) > 0);
  app.stop().await;
  assert!(STOP_CALLED.load(std::sync::atomic::Ordering::SeqCst) >= 1);
}
