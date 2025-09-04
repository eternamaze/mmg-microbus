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

mmg_microbus::easy_main!();
