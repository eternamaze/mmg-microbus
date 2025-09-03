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

#[mmg_microbus::handles]
impl Trader {
    // 只读 Envelope 形态（&Envelope<T>）
    #[mmg_microbus::handle(Tick)]
    async fn on_tick_ro(
        &mut self,
        _ctx: &mmg_microbus::component::ComponentContext,
        _env: &mmg_microbus::bus::Envelope<Tick>,
    ) -> anyhow::Result<()> {
        // 实际读取字段，避免 dead_code 告警
        let _n = _env.msg.0;
        Ok(())
    }

    // 只读负载形态（&T），带过滤：from=External, instance=ExtAccept
    #[mmg_microbus::handle(Quote, from=External, instance=ExtAccept)]
    async fn on_quote_filtered(
        &mut self,
        bus: &mmg_microbus::bus::ScopedBus,
        _q: &Quote,
    ) -> anyhow::Result<()> {
        // 实际读取字段，避免 dead_code 告警
        let _p = _q.0;
        bus.publish(Ack("ok")).await;
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
    let mut app = App::new_default();
    app.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    let h = app.bus_handle();
    let mut sub_ack = h
        .subscribe_pattern::<Ack>(ServicePattern::for_kind::<Trader>())
        .await;

    // 不匹配的 instance：不应收到 Ack
    struct ExtReject;
    impl mmg_microbus::bus::InstanceMarker for ExtReject {
        fn id() -> &'static str {
            "ext-reject"
        }
    }
    let ext_reject = mmg_microbus::bus::ServiceAddr::of_instance::<External, ExtReject>();
    h.publish(&ext_reject, Quote(1.0)).await;
    let no_ack = tokio::time::timeout(std::time::Duration::from_millis(50), sub_ack.recv())
        .await
        .ok()
        .flatten();
    assert!(no_ack.is_none(), "unexpected ack for mismatched instance");

    // 匹配的 instance：应收到 Ack
    let ext_accept = mmg_microbus::bus::ServiceAddr::of_instance::<External, ExtAccept>();
    h.publish(&ext_accept, Quote(2.0)).await;
    let ack = tokio::time::timeout(std::time::Duration::from_secs(1), sub_ack.recv())
        .await
        .ok()
        .flatten()
        .expect("no ack for matched filter");
    assert_eq!(ack.0, "ok");

    // 发布只读 Envelope 匹配事件，确保不会触发 panic/类型错误
    h.publish_enveloped(&ext_accept, Tick(1), None).await;

    app.stop().await;
}
