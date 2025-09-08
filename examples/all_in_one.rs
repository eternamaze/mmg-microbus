//! 单文件全功能示例：
//! - 唯一解耦路径：函数参数注入（上下文、消息、配置）
//! - 统一注解模型（#[component]/#[handle]/#[active]）
//! - 主动函数（#[active]）与被动订阅（#[handle]）
//! - 路由：仅按消息类型（不再有实例过滤/地址）
//! - 强类型配置（app.config，#[init] 以 &Cfg 注入，允许可选 &Context）

use mmg_microbus::prelude::*;

// ---- 消息类型 ----
#[derive(Clone, Debug)]
struct Tick(pub u64);
#[derive(Clone, Debug)]
struct Price(pub f64);
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stopped(&'static str);

// ---- 强类型配置 ----
#[derive(Clone)]
struct TraderCfg {
    symbol: String,
    min_tick: u64,
}

// ---- 主动消息源组件：定时发布 Tick ----
#[mmg_microbus::component]
#[derive(Default)]
struct Feeder;

#[mmg_microbus::component]
impl Feeder {
    // 展示主动函数：立即触发一次 + 每 100ms 触发一次，最多 5 次
    #[mmg_microbus::active(immediate, interval_ms = 100, times = 5)]
    async fn tick(&self, _ctx: &mmg_microbus::component::ComponentContext) -> Tick {
        static CNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = CNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        Tick(n)
    }
}

// ---- 被动订阅组件：演示上下文与配置注入 ----
#[mmg_microbus::component]
#[derive(Default)]
struct Trader { cfg: Option<TraderCfg> }

#[mmg_microbus::component]
impl Trader {
    // 初始化阶段读取配置并保存到组件状态
    #[mmg_microbus::init]
    async fn setup(&mut self, cfg: &TraderCfg) -> anyhow::Result<()> { self.cfg = Some(cfg.clone()); Ok(()) }
    // 订阅 Tick；注入 &ComponentContext 与 &Tick（配置已在 #[init] 保存到状态）
    #[mmg_microbus::handle]
    async fn on_tick(
        &mut self,
    _ctx: &mmg_microbus::component::ComponentContext,
        tick: &Tick,
    ) -> Option<Price> {
        let min_tick = self.cfg.as_ref().map(|c| c.min_tick).unwrap_or(0);
        if min_tick == 0 || tick.0 % min_tick == 0 {
            // 将 Tick 转换成 Price；返回值即发布
            Some(Price(tick.0 as f64))
        } else { None }
    }

    // 简化：不区分来源；消息类型即订阅
    #[mmg_microbus::handle]
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

    // 停止钩子：框架停机时同步调用一次，返回值将自动发布
    #[mmg_microbus::stop]
    async fn on_stop(&self) -> Stopped { Stopped("bye") }
}

// ---- 实例使用字符串 ID（不再需要强类型实例标记/服务类型） ----

// 收集停止消息，展示返回值自动发布在停机时仍然有效
#[mmg_microbus::component]
#[derive(Default)]
struct Collector;
#[mmg_microbus::component]
impl Collector {
    #[mmg_microbus::handle]
    async fn on_stopped(&self, _ctx: &mmg_microbus::component::ComponentContext, s: &Stopped) {
        let _ = s.0; // 读取以避免告警
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // App 是唯一控制入口
    let mut app = App::new(Default::default());
    // 注册组件实例（类型安全）
    app.add_component::<Feeder>("feeder-1");
    app.add_component::<Trader>("trader-1");
    app.add_component::<Collector>("collector-1");
    // 注入强类型配置（可多项）
    app.config(TraderCfg {
        symbol: "BTCUSDT".into(),
        min_tick: 2,
    })
    .await?;

    app.start().await?;

    // 从外部发布消息：已移除对外发布 API（示例省略）

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    app.stop().await;
    Ok(())
}
