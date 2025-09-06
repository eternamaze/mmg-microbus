//! 主动消息源示例：自定义 run 循环定时发布 Tick，由 Trader 订阅处理

#[derive(Clone, Debug)]
struct Tick(pub u64);

#[mmg_microbus::component]
struct Feeder {
    id: mmg_microbus::bus::ComponentId,
}

#[async_trait::async_trait]
impl mmg_microbus::component::Component for Feeder {
    fn id(&self) -> &mmg_microbus::bus::ComponentId {
        &self.id
    }
    async fn run(
        self: Box<Self>,
        mut ctx: mmg_microbus::component::ComponentContext,
    ) -> anyhow::Result<()> {
        let mut n = 0u64;
        let mut intv = tokio::time::interval(std::time::Duration::from_millis(200));
        loop {
            tokio::select! {
                _ = intv.tick() => { n += 1; ctx.publish(Tick(n)).await; }
                _ = ctx.shutdown.changed() => { break; }
            }
        }
        Ok(())
    }
}

#[mmg_microbus::component]
struct Trader {
    id: mmg_microbus::bus::ComponentId,
}

#[mmg_microbus::handles]
impl Trader {
    #[mmg_microbus::handle(Tick, from=Feeder)]
    async fn on_tick(&mut self, tick: &Tick) -> anyhow::Result<()> {
        tracing::info!(target = "example.active", tick = tick.0);
        Ok(())
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    // 显式注册组件实例（类型安全）
    let mut app = mmg_microbus::prelude::App::new(Default::default());
    app.add_component::<Trader>("trader-1");
    app.start().await?;
    // 运行一小段时间后退出
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    app.stop().await;
    Ok(())
}
