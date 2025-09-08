use mmg_microbus::prelude::*;

#[derive(Clone, Debug)]
struct Tick(pub u64);
#[derive(Clone, Debug)]
struct Quote(pub f64);
#[derive(Clone, Debug)]
struct Ack(pub &'static str);

#[mmg_microbus::component]
struct Trader {
    id: mmg_microbus::bus::ComponentId,
}

#[mmg_microbus::component]
impl Trader {
    // 只读 &T 形态
    #[mmg_microbus::handle]
    async fn on_tick_ro(
        &mut self,
        _ctx: &mmg_microbus::component::ComponentContext,
        _tick: &Tick,
    ) -> anyhow::Result<()> {
        // 实际读取字段，避免 dead_code 告警
        let _n = _tick.0;
        Ok(())
    }

    // 只读负载形态（&T），带过滤：仅按实例字符串过滤
    #[mmg_microbus::handle(instance="ext-accept")]
    async fn on_quote_filtered(
        &mut self,
        ctx: &mmg_microbus::component::ComponentContext,
        _q: &Quote,
    ) -> anyhow::Result<()> {
        // 实际读取字段，避免 dead_code 告警
        let _p = _q.0;
        ctx.publish(Ack("ok")).await;
        Ok(())
    }
}

struct External;
struct ExtAccept;
impl mmg_microbus::bus::InstanceMarker for ExtAccept {
    fn id() -> &'static str {
        "ext-accept"
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn readonly_and_filters_work() {
    let mut app = App::new(Default::default());
    app.add_component::<Trader>("trader-1");
    app.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let h = app.bus_handle();
    let mut sub_ack = h
        .subscribe::<Ack>(&mmg_microbus::bus::Address { service: None, instance: Some(mmg_microbus::bus::ComponentId("trader-1".to_string())) })
        .await;

    // 不匹配的 instance：不应收到 Ack
    struct ExtReject;
    impl mmg_microbus::bus::InstanceMarker for ExtReject {
        fn id() -> &'static str {
            "ext-reject"
        }
    }
    let ext_reject = mmg_microbus::bus::Address::of_instance::<External, ExtReject>();
    h.publish(&ext_reject, Quote(1.0)).await;
    let no_ack = tokio::time::timeout(std::time::Duration::from_millis(50), sub_ack.recv())
        .await
        .ok()
        .flatten();
    assert!(no_ack.is_none(), "unexpected ack for mismatched instance");

    // 匹配的 instance：应收到 Ack
    let ext_accept = mmg_microbus::bus::Address::of_instance::<External, ExtAccept>();
    h.publish(&ext_accept, Quote(2.0)).await;
    let ack = tokio::time::timeout(std::time::Duration::from_secs(1), sub_ack.recv())
        .await
        .ok()
        .flatten()
        .expect("no ack for matched filter");
    assert_eq!(ack.0, "ok");

    // 发布只读 Tick 匹配事件
    h.publish(&ext_accept, Tick(1)).await;

    app.stop().await;
}
