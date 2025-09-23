//! 单文件示例：展示最小框架语义 + 六类返回值自动发布集合。
//! 返回类型支持：
//! - () / Result<()> : 不发布
//! - T / Result<T> : 成功发布一条
//! - Option<T> / Result<Option<T>> : Some(T) 发布

use mmg_microbus::prelude::*;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

// ---- 消息类型 ----
#[derive(Clone, Debug)]
struct Tick(pub u64);
#[derive(Clone, Debug)]
struct Price(pub u64);
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stopped(&'static str);

// ---- 模块级原子计数器，避免函数体内声明静态导致 clippy::items_after_statements ----
static CNT: AtomicU64 = AtomicU64::new(0);
static P: AtomicU32 = AtomicU32::new(0);
static R: AtomicU64 = AtomicU64::new(0);
static Q: AtomicU32 = AtomicU32::new(0);

// ---- 主动消息源组件：定时发布 Tick，并演示多种返回类型 ----
#[mmg_microbus::component]
#[derive(Default)]
struct Feeder;

#[mmg_microbus::component]
impl Feeder {
    // 展示主动函数：无限循环（框架协作式 yield，函数每完成一次立即再调度）
    // 1) 返回 T
    #[mmg_microbus::active]
    async fn tick(&self, _ctx: &mmg_microbus::component::ComponentContext) -> Tick {
        tokio::task::yield_now().await;
        let n = CNT.fetch_add(1, Ordering::Relaxed) + 1;
        Tick(n)
    }

    // 2) 返回 Option<T>
    #[mmg_microbus::active]
    async fn maybe_price(&self) -> Option<Price> {
        tokio::task::yield_now().await;
        // 只发布偶数 tick 对应的 Price
        let n: u32 = P.fetch_add(1, Ordering::Relaxed) + 1;
        if n % 2 == 0 {
            Some(Price(u64::from(n)))
        } else {
            None
        }
    }

    // 3) 返回 Result<T>
    #[mmg_microbus::active]
    async fn result_tick(&self) -> Result<Tick> {
        tokio::task::yield_now().await;
        let n = R.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(Tick(10_000 + n))
    }

    // 4) 返回 Result<Option<T>>
    #[mmg_microbus::active]
    async fn result_maybe(&self) -> Result<Option<Price>> {
        tokio::task::yield_now().await;
        let n: u32 = Q.fetch_add(1, Ordering::Relaxed) + 1;
        if n % 3 == 0 {
            Ok(Some(Price(1000 + u64::from(n))))
        } else {
            Ok(None)
        }
    }

    // 5) 返回 () （不发布）
    #[mmg_microbus::active]
    async fn heartbeat(&self) {
        /* no-op */
        tokio::task::yield_now().await;
    }

    // 6) 返回 Result<()> （不发布；错误只记录 Warn）
    #[mmg_microbus::active]
    async fn maybe_err(&self) -> Result<()> {
        tokio::task::yield_now().await;
        // 构造一个永远 Ok 的示例；可改成 Err(anyhow!("boom")) 观察 warn
        Ok(())
    }
}

// ---- 被动订阅组件：演示上下文与配置注入 ----
#[mmg_microbus::component]
#[derive(Default)]
struct Trader {
    symbol: String,
    min_tick: u64,
}

#[mmg_microbus::component]
impl Trader {
    // 初始化阶段读取配置并保存到组件状态
    #[mmg_microbus::init]
    async fn setup(&mut self) -> Result<()> {
        tokio::task::yield_now().await;
        // 组件自行获取其“配置”：此处硬编码，真实场景可读 env / 文件 / 其它消息
        self.symbol = "BTCUSDT".into();
        self.min_tick = 2;
        Ok(())
    }
    // 订阅 Tick；注入 &ComponentContext 与 &Tick（配置已在 #[init] 保存到状态） -> 返回 Option<T>
    #[mmg_microbus::handle]
    async fn on_tick(
        &self,
        _ctx: &mmg_microbus::component::ComponentContext,
        tick: &Tick,
    ) -> Option<Price> {
        tokio::task::yield_now().await;
        let min_tick = self.min_tick;
        if min_tick == 0 || tick.0 % min_tick == 0 {
            // 将 Tick 转换成 Price；返回值即发布
            Some(Price(tick.0))
        } else {
            None
        }
    }

    // 返回 Result<()> （不发布）
    #[mmg_microbus::handle]
    async fn on_price_binance(
        &self,
        _ctx: &mmg_microbus::component::ComponentContext,
        price: &Price,
    ) -> Result<()> {
        tokio::task::yield_now().await;
        let symbol = self.symbol.as_str();
        tracing::info!(target = "example.all", symbol = %symbol, price = price.0);
        Ok(())
    }

    // 返回 Result<()> 另一示例
    #[mmg_microbus::handle]
    async fn on_any_price(
        &self,
        _ctx: &mmg_microbus::component::ComponentContext,
        _p: &Price,
    ) -> Result<()> {
        tokio::task::yield_now().await;
        // 这里不做过滤，任意来源价格都会触发
        Ok(())
    }

    // 停止钩子：返回 T 被发布
    #[mmg_microbus::stop]
    async fn on_stop(&self) -> Stopped {
        tokio::task::yield_now().await;
        Stopped("bye")
    }
}

// 收集停止消息，展示停机时返回值仍会发布
#[mmg_microbus::component]
#[derive(Default)]
struct Collector;
#[mmg_microbus::component]
impl Collector {
    #[mmg_microbus::handle]
    async fn on_stopped(&self, _ctx: &mmg_microbus::component::ComponentContext, s: &Stopped) {
        tokio::task::yield_now().await;
        let _ = s.0; // 读取以避免告警
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<()> {
    // App 是唯一控制入口
    let mut app = App::new(mmg_microbus::config::AppConfig::default());
    // 单例自动发现：无需显式 add_component
    // 已移除外部配置注入：组件将在 #[init]/#[active(once)] 中自行完成需求初始化

    app.start().await?;

    // 从外部发布消息：已移除对外发布 API（示例省略）

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    app.stop().await;
    Ok(())
}
