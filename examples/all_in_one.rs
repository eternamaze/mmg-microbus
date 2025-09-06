//! 单文件全功能示例：
//! - 唯一解耦路径：函数参数注入（上下文、消息、配置）
//! - 组件/handler 宏（component/handles/handle）
//! - 主动消息源组件（自定义 run）与被动订阅组件
//! - 过滤（from=ServiceType, instance=MarkerType）
//! - 强类型配置（app.provide_config，handler 以 &Cfg 注入）

use mmg_microbus::prelude::*;

// ---- 消息类型 ----
#[derive(Clone, Debug)]
struct Tick(pub u64);
#[derive(Clone, Debug)]
struct Price(pub f64);

// ---- 强类型配置 ----
#[derive(Clone)]
struct SymbolCfg { symbol: String }
#[derive(Clone)]
struct TickCfg { min_tick: u64 }

// ---- 主动消息源组件：定时发布 Tick ----
#[mmg_microbus::component]
struct Feeder { id: mmg_microbus::bus::ComponentId }

#[async_trait::async_trait]
impl mmg_microbus::component::Component for Feeder {
    fn id(&self) -> &mmg_microbus::bus::ComponentId { &self.id }
    async fn run(self: Box<Self>, mut ctx: mmg_microbus::component::ComponentContext) -> anyhow::Result<()> {
        let mut n = 0u64;
        let mut intv = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            tokio::select! {
                _ = intv.tick() => { n += 1; ctx.publish(Tick(n)).await; }
                _ = ctx.shutdown.changed() => { break; }
            }
        }
        Ok(())
    }
}

// ---- 被动订阅组件：演示上下文、过滤与多配置注入 ----
#[mmg_microbus::component]
struct Trader { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::handles]
impl Trader {
    // 订阅来自 Feeder 的 Tick；注入 &ComponentContext、&Tick、&TickCfg
    #[mmg_microbus::handle(Tick, from=Feeder)]
    async fn on_tick(
        &mut self,
        ctx: &mmg_microbus::component::ComponentContext,
        tick: &Tick,
        tcfg: &TickCfg,
    ) -> anyhow::Result<()> {
        if tcfg.min_tick == 0 || tick.0 % tcfg.min_tick == 0 {
            // 将 Tick 转换成 Price，并以 Exchange::Binance 身份发布
            let from = mmg_microbus::bus::Address::of_instance::<Exchange, Binance>();
            ctx.publish_from(&from, Price(tick.0 as f64)).await;
        }
        Ok(())
    }

    // 只接收来自 Exchange::Binance 的价格；注入 &Price 与 &SymbolCfg
    #[mmg_microbus::handle(Price, from=Exchange, instance=Binance)]
    async fn on_price_binance(
        &mut self,
        price: &Price,
        scfg: &SymbolCfg,
    ) -> anyhow::Result<()> {
    tracing::info!(target = "example.all", symbol = %scfg.symbol, price = price.0);
        Ok(())
    }

    // 展示可选的 Ack 发布（匹配 Trader 类型的监听者）
    #[mmg_microbus::handle(Price)]
    async fn on_any_price(&mut self, _p: &Price) -> anyhow::Result<()> {
        // 这里不做过滤，任意来源价格都会触发
        Ok(())
    }
}

// ---- 服务与实例标记（用于过滤）----
struct Exchange;
struct Binance;
impl mmg_microbus::bus::InstanceMarker for Binance { fn id() -> &'static str { "binance" } }

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // App 是唯一控制入口
    let mut app = App::new(Default::default());
    // 注册组件实例（类型安全）
    app.add_component::<Feeder>("feeder-1");
    app.add_component::<Trader>("trader-1");
    // 注入强类型配置（可多项）
    app.provide_config(SymbolCfg { symbol: "BTCUSDT".into() }).await?;
    app.provide_config(TickCfg { min_tick: 2 }).await?;

    app.start().await?;

    // 从外部也可发布消息（使用 BusHandle）
    let bus = app.bus_handle();
    let ext = mmg_microbus::bus::Address::of_instance::<Exchange, Binance>();
    bus.publish(&ext, Price(100.0)).await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    app.stop().await;
    Ok(())
}
