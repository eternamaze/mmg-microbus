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
    // 新语义：不再支持 from=Kind 单例约束；该测试改为验证可订阅但不失败
    #[mmg_microbus::handle(instance="p1")]
    async fn on_ping(
        &mut self,
        _ctx: &mmg_microbus::component::ComponentContext,
        _p: &Ping,
    ) {
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn route_singleton_constraint_fails_on_multiple_instances() {
    let mut app = App::new(Default::default());
    app.add_component::<ProducerKind>("p1");
    app.add_component::<ProducerKind>("p2");
    app.add_component::<NeedsSingleton>("c1");
    // 新语义：不会失败
    app.start().await.unwrap();
    app.stop().await;
}
