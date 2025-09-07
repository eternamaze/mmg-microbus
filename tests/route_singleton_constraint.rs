use mmg_microbus::prelude::*;

#[mmg_microbus::component]
struct ProducerKind {
    id: mmg_microbus::bus::ComponentId,
}

#[derive(Clone, Debug)]
struct Ping;

#[mmg_microbus::component]
impl ProducerKind {
    // 空组件，用于制造多实例的 Kind
}

#[mmg_microbus::component]
struct NeedsSingleton {
    id: mmg_microbus::bus::ComponentId,
}

#[mmg_microbus::component]
impl NeedsSingleton {
    // 明确声明 from=ProducerKind，但不指定 instance，触发路由单例约束
    #[mmg_microbus::handle(Ping, from = ProducerKind)]
    async fn on_ping(&mut self, _p: &Ping) {}
}

#[tokio::test(flavor = "multi_thread")]
async fn route_singleton_constraint_fails_on_multiple_instances() {
    let mut app = App::new(Default::default());
    app.add_component::<ProducerKind>("p1");
    app.add_component::<ProducerKind>("p2");
    app.add_component::<NeedsSingleton>("c1");
    let err = app.start().await.err().expect("start should fail");
    let msg = format!("{}", err);
    assert!(msg.contains("expects singleton of kind"));
}
