//! 单文件全功能示例：
//! - 唯一解耦路径：函数参数注入（上下文、消息、配置）
//! - 统一注解模型（#[component]/#[handle]/#[active]）
//! - 主动函数（#[active]）与被动订阅（#[handle]）
//! - 过滤：按实例字符串（#[handle(instance="id")]）
//! - 强类型配置（app.config，#[init] 以 &Cfg 注入）

use mmg_microbus::prelude::*;

// ---- 消息类型 ----
#[derive(Clone, Debug)]
struct Tick(pub u64);
#[derive(Clone, Debug)]
struct Price(pub f64);

// ---- 强类型配置 ----
#[derive(Clone)]
struct TraderCfg {
    symbol: String,
    min_tick: u64,
}

// ---- 主动消息源组件：定时发布 Tick ----
#[mmg_microbus::component]
struct Feeder {
    id: mmg_microbus::bus::ComponentId,
}

#[mmg_microbus::component]
impl Feeder {
    #[mmg_microbus::active(interval_ms = 100)]
    async fn tick(&self, ctx: &mmg_microbus::component::ComponentContext) -> anyhow::Result<()> {
        static CNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = CNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        ctx.publish(Tick(n)).await;
        Ok(())
    }
}

// ---- 被动订阅组件：演示上下文、过滤与配置注入 ----
#[mmg_microbus::component]
struct Trader {
    id: mmg_microbus::bus::ComponentId,
    cfg: Option<TraderCfg>,
}

#[mmg_microbus::component]
impl Trader {
    // 初始化阶段读取配置并保存到组件状态
    #[mmg_microbus::init]
    async fn setup(&mut self, cfg: &TraderCfg) -> anyhow::Result<()> {
        self.cfg = Some(cfg.clone());
        Ok(())
    }
    // 订阅 Tick；注入 &ComponentContext 与 &Tick（配置已在 #[init] 保存到状态）
    #[mmg_microbus::handle]
    async fn on_tick(
        &mut self,
        ctx: &mmg_microbus::component::ComponentContext,
        tick: &Tick,
    ) -> anyhow::Result<()> {
        let min_tick = self.cfg.as_ref().map(|c| c.min_tick).unwrap_or(0);
        if min_tick == 0 || tick.0 % min_tick == 0 {
            // 将 Tick 转换成 Price，并以 Exchange::Binance 身份发布
            let from = mmg_microbus::bus::Address::of_instance::<Exchange>("binance");
            ctx.publish_from(&from, Price(tick.0 as f64)).await;
        }
        Ok(())
    }

    // 只接收来自特定实例（binance）的价格；注入 &ComponentContext 与 &Price
    #[mmg_microbus::handle(instance="binance")]
    async fn on_price_binance(
        &mut self,
        _ctx: &mmg_microbus::component::ComponentContext,
        price: &Price,
    ) -> anyhow::Result<()> {
        let symbol = self.cfg.as_ref().map(|c| c.symbol.as_str()).unwrap_or("");
        tracing::info!(target = "example.all", symbol = %symbol, price = price.0);
        Ok(())
    }

    // 展示可选的 Ack 发布（匹配 Trader 类型的监听者）
    #[mmg_microbus::handle]
    async fn on_any_price(
        &mut self,
        _ctx: &mmg_microbus::component::ComponentContext,
        _p: &Price,
    ) -> anyhow::Result<()> {
        // 这里不做过滤，任意来源价格都会触发
        Ok(())
    }
}

// ---- 服务（用于构建 service 类型）；实例使用字符串 ID ----
struct Exchange;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // App 是唯一控制入口
    let mut app = App::new(Default::default());
    // 注册组件实例（类型安全）
    app.add_component::<Feeder>("feeder-1");
    app.add_component::<Trader>("trader-1");
    // 注入强类型配置（可多项）
    app.config(TraderCfg {
        symbol: "BTCUSDT".into(),
        min_tick: 2,
    })
    .await?;

    app.start().await?;

    // 从外部也可发布消息（使用 BusHandle）
    let bus = app.bus_handle();
    let ext = mmg_microbus::bus::Address::of_instance::<Exchange>("binance");
    bus.publish(&ext, Price(100.0)).await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    app.stop().await;
    Ok(())
}
