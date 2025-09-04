//! 最终示例：单一路径（&T-only）、方法即订阅、配置即对象、上下文与过滤

struct Tick(pub u64);
struct Price(pub f64);

#[derive(Clone, serde::Deserialize, serde::Serialize)]
struct MyCfg {
    symbol: String,
    min_tick: u64,
}

#[mmg_microbus::component]
struct Trader {
    id: mmg_microbus::bus::ComponentId,
    cfg: Option<MyCfg>,
}

#[mmg_microbus::handles]
impl Trader {
    // 订阅 Tick（&T 形态），可注入上下文
    #[mmg_microbus::handle(Tick, from=Exchange)]
    async fn on_tick(
        &mut self,
        ctx: &mmg_microbus::component::ComponentContext,
        tick: &Tick,
    ) -> anyhow::Result<()> {
        let min_tick = self.cfg.as_ref().map(|c| c.min_tick).unwrap_or(1);
        if min_tick > 0 && tick.0 % min_tick == 0 {
            // 以 Exchange::Binance 身份发布，从而命中下方的过滤器
            let from = mmg_microbus::bus::Address::of_instance::<Exchange, Binance>();
            ctx.publish_from(&from, Price(tick.0 as f64)).await;
        }
        Ok(())
    }

    // &Price，按实例过滤
    #[mmg_microbus::handle(Price, from=Exchange, instance=Binance)]
    async fn on_price_binance(&mut self, price: &Price) -> anyhow::Result<()> {
        tracing::info!(target = "price.binance", price = price.0);
        Ok(())
    }
}

#[async_trait::async_trait]
#[mmg_microbus::configure(MyCfg)]
impl mmg_microbus::component::Configure<MyCfg> for Trader {
    async fn on_config(
        &mut self,
        ctx: &mmg_microbus::component::ConfigContext,
        cfg: MyCfg,
    ) -> anyhow::Result<()> {
        let _ = ctx; // context 可选使用，这里仅更新内部配置
        self.cfg = Some(cfg);
        Ok(())
    }
}

struct Exchange;
struct Binance;
impl mmg_microbus::bus::InstanceMarker for Binance {
    fn id() -> &'static str {
        "binance"
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // 启动应用并注入一次性配置
    let mut app = mmg_microbus::prelude::App::new_default();
    app.config(MyCfg {
        symbol: "BTCUSDT".into(),
        min_tick: 1,
    })
    .await?;
    app.start().await?;
    // 用 BusHandle 从外部来源发布几条 Tick，形成可见的消息流
    let h = app.bus_handle();
    let ext = mmg_microbus::bus::Address::of_instance::<Exchange, Binance>();
    for i in 1..=5u64 {
        h.publish(&ext, Tick(i)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    app.stop().await;
    Ok(())
}
