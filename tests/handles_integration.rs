use mmg_microbus::prelude::*;

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

#[mmg_microbus::component]
impl Trader {
    // &T 形态，注入上下文
    #[mmg_microbus::handle(instance="ext-1")]
    async fn on_tick(
        &mut self,
        _ctx: &mmg_microbus::component::ComponentContext,
        _tick: &Tick,
    ) -> anyhow::Result<()> {
        let _n = _tick.0;
        Ok(())
    }

    // 负载形态，通过上下文发布 Ack
    #[mmg_microbus::handle(instance="ext-1")]
    async fn on_price(
        &mut self,
        ctx: &mmg_microbus::component::ComponentContext,
        price: &Price,
    ) -> anyhow::Result<()> {
        let _p = price.0;
        ctx.publish(Ack("ok")).await;
        Ok(())
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn method_based_subscription_works() {
    let mut app = App::new(Default::default());
    app.add_component::<Trader>("trader-1");
    app.start().await.unwrap();
    // 等待组件完成订阅建立，避免发布过早导致丢失
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    // 订阅 Trader 发出的 Ack
    let h = app.bus_handle();
    // 现行语义：订阅需指明实例；这里订阅 trader-1 实例发出的 Ack
    let mut sub = h
        .subscribe::<Ack>(&mmg_microbus::bus::Address { service: None, instance: Some(mmg_microbus::bus::ComponentId("trader-1".to_string())) })
        .await;

    // 从外部来源发布消息
    struct External;
    let ext = mmg_microbus::bus::Address::of_instance::<External>("ext-1");
    h.publish(&ext, Price(1.0)).await;
    h.publish(&ext, Tick(1)).await;

    // 应能收到 Ack
    let ack = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
        .await
        .ok()
        .flatten()
        .expect("no ack received");
    assert_eq!(ack.0, "ok");

    app.stop().await;
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Stopped(&'static str);

#[mmg_microbus::component]
struct Stoppable {
    id: mmg_microbus::bus::ComponentId,
}

#[mmg_microbus::component]
impl Stoppable {
    #[mmg_microbus::stop]
    async fn on_stop(&self) -> Stopped {
        Stopped("bye")
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_hook_is_invoked_and_can_publish() {
    let mut app = App::new(Default::default());
    app.add_component::<Stoppable>("s1");

    // 订阅 stop 消息
    let h = app.bus_handle();
    let mut sub = h
        .subscribe::<Stopped>(&mmg_microbus::bus::Address { service: None, instance: Some(mmg_microbus::bus::ComponentId("s1".to_string())) })
        .await;

    app.start().await.unwrap();
    // 停机应触发 #[stop]，发布 Stopped
    app.stop().await;
    let got = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
        .await
        .ok()
        .flatten()
        .expect("no Stopped received");
    assert_eq!(&*got, &Stopped("bye"));
}
