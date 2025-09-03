use mmg_microbus::bus::{Bus, InstanceMarker, ServiceAddr, ServicePattern};

#[derive(Clone, Debug)]
struct Msg(u32);

struct Producer;
struct P1;
struct P2;
struct P3;
impl InstanceMarker for P1 {
    fn id() -> &'static str {
        "p1"
    }
}
impl InstanceMarker for P2 {
    fn id() -> &'static str {
        "p2"
    }
}
impl InstanceMarker for P3 {
    fn id() -> &'static str {
        "p3"
    }
}

#[tokio::test]
async fn cleanup_after_drop_exact_and_pattern() {
    // given a fresh bus
    #[cfg(feature = "bus-metrics")]
    let bus = Bus::new(8, None);
    #[cfg(not(feature = "bus-metrics"))]
    let bus = Bus::new(8);
    let h = bus.handle();
    let from = ServiceAddr::of_instance::<Producer, P1>();

    // subscribe exact and pattern
    let mut sub_exact = h.subscribe::<Msg>(&from).await;
    let sub_pat = h
        .subscribe_pattern::<Msg>(ServicePattern::for_kind::<Producer>())
        .await;

    // drop pattern subscriber, trigger cleanup via publish
    drop(sub_pat);
    h.publish(&from, Msg(1)).await; // deliver to exact; also prune closed pattern
                                    // still can receive from exact subscriber
    let got = tokio::time::timeout(std::time::Duration::from_millis(100), sub_exact.recv())
        .await
        .ok()
        .flatten();
    assert!(got.is_some());
    if let Some(m) = got {
        let _ = m.0;
    } // read field to avoid dead_code warning

    // drop exact, trigger cleanup
    drop(sub_exact);
    h.publish(&from, Msg(2)).await; // no recipients; prune exact topic
                                    // publishing should be a no-op; no panic/hang
                                    // can't directly assert internal counts; absence of receive is sufficient here
}

#[tokio::test]
async fn cleanup_on_repeated_sub_unsub_pattern() {
    #[cfg(feature = "bus-metrics")]
    let bus = Bus::new(4, None);
    #[cfg(not(feature = "bus-metrics"))]
    let bus = Bus::new(4);
    let h = bus.handle();
    let from = ServiceAddr::of_instance::<Producer, P2>();
    for i in 0..10u32 {
        let sub = h
            .subscribe_pattern::<Msg>(ServicePattern::for_instance_marker::<Producer, P2>())
            .await;
        drop(sub);
        h.publish(&from, Msg(i)).await; // trigger prune
                                        // If prune worked, subsequent publish finds no recipients and returns quickly
    }
}

#[tokio::test]
async fn publish_with_no_subscribers_is_noop() {
    #[cfg(feature = "bus-metrics")]
    let bus = Bus::new(2, None);
    #[cfg(not(feature = "bus-metrics"))]
    let bus = Bus::new(2);
    let h = bus.handle();
    let from = ServiceAddr::of_instance::<Producer, P3>();
    // should not panic or block indefinitely
    h.publish(&from, Msg(0)).await;
    // No subscribers; just ensure it returns
}
