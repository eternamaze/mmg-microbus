use mmg_microbus::prelude::*;
use std::sync::Arc;

#[derive(Clone, Debug)]
struct Tick(pub u64);
#[derive(Clone, Debug)]
struct Price(pub f64);
#[derive(Clone, Debug)]
struct Ack(pub &'static str);

#[mmg_microbus::component]
struct Trader {
    id: mmg_microbus::bus::ComponentId,
}

#[mmg_microbus::handles]
impl Trader {
    // Envelope 形态，注入上下文
    #[mmg_microbus::handle(Tick)]
    async fn on_tick(
        &mut self,
        _ctx: &mmg_microbus::component::ComponentContext,
        _env: Arc<mmg_microbus::bus::Envelope<Tick>>,
    ) -> anyhow::Result<()> {
        // 读取字段以避免 dead_code 的“字段未读”告警
        let _n = _env.msg.0;
        Ok(())
    }

    // 负载形态，注入 ScopedBus，发布 Ack
    #[mmg_microbus::handle(Price)]
    async fn on_price(
        &mut self,
        bus: &mmg_microbus::bus::ScopedBus,
        _price: Arc<Price>,
    ) -> anyhow::Result<()> {
        // 读取字段以避免 dead_code 的“字段未读”告警
        let _p = _price.0;
        bus.publish(Ack("ok")).await;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn method_based_subscription_works() {
    let mut app = App::new_default();
    app.start().await.unwrap();
    // 等待组件完成订阅建立，避免发布过早导致丢失
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // 订阅 Trader 发出的 Ack
    let h = app.bus_handle();
    let mut sub = h
        .subscribe_pattern::<Ack>(ServicePattern::for_kind::<Trader>())
        .await;

    // 从外部来源发布消息
    struct External;
    struct Ext1;
    impl mmg_microbus::bus::InstanceMarker for Ext1 {
        fn id() -> &'static str {
            "ext-1"
        }
    }
    let ext = mmg_microbus::bus::ServiceAddr::of_instance::<External, Ext1>();
    h.publish(&ext, Price(1.0)).await;
    h.publish_enveloped(&ext, Tick(1), None).await;

    // 应能收到 Ack
    let ack = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
        .await
        .ok()
        .flatten()
        .expect("no ack received");
    assert_eq!(ack.0, "ok");

    app.stop().await;
}
