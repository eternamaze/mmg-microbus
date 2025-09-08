use mmg_microbus::prelude::*;

#[derive(Clone, Debug)]
struct MyCfg {
    val: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CfgEcho(i32);

#[mmg_microbus::component]
struct Echoer {
    id: mmg_microbus::bus::ComponentId,
}

#[mmg_microbus::component]
impl Echoer {
    #[mmg_microbus::init]
    async fn init(&mut self, cfg: &MyCfg) -> CfgEcho {
        CfgEcho(cfg.val)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn config_last_write_wins_with_warning() {
    let mut app = App::new(Default::default());
    app.add_component::<Echoer>("e1");

    // 同类型多次设置，最后一次应生效
    let _ = app.config(MyCfg { val: 1 }).await.unwrap();
    let _ = app.config(MyCfg { val: 2 }).await.unwrap();

    // 订阅 Echoer 在 #[init] 阶段发布的 CfgEcho
    let h = app.bus_handle();
    let mut sub = h
        .subscribe::<CfgEcho>(&mmg_microbus::bus::Address { service: None, instance: Some(mmg_microbus::bus::ComponentId("e1".to_string())) })
        .await;

    app.start().await.unwrap();

    let got = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
        .await
        .ok()
        .flatten()
        .expect("no CfgEcho received");
    assert_eq!(&*got, &CfgEcho(2));

    app.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn config_many_is_thin_wrapper_over_config() {
    let mut app = App::new(Default::default());
    app.add_component::<Echoer>("e1");

    // 先通过 config_many(闭包) 设置，再用单项 config 覆盖
    let _ = app
        .config_many(|a| {
            Box::pin(async move {
                let _ = a.config(MyCfg { val: 10 }).await?;
                Ok(())
            })
        })
        .await
        .unwrap();
    let _ = app.config(MyCfg { val: 99 }).await.unwrap();

    let h = app.bus_handle();
    let mut sub = h
        .subscribe::<CfgEcho>(&mmg_microbus::bus::Address { service: None, instance: Some(mmg_microbus::bus::ComponentId("e1".to_string())) })
        .await;

    app.start().await.unwrap();
    let got = tokio::time::timeout(std::time::Duration::from_secs(1), sub.recv())
        .await
        .ok()
        .flatten()
        .expect("no CfgEcho received");
    assert_eq!(&*got, &CfgEcho(99));

    app.stop().await;
}
