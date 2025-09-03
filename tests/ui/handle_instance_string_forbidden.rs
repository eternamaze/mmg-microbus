use mmg_microbus::prelude::*;

#[mmg_microbus::component]
struct C { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::handles]
impl C {
    #[mmg_microbus::handle(Tick, from=Src, instance="x")] // ERROR: string instance forbidden
    async fn on_tick(&mut self, _env: std::sync::Arc<Envelope<Tick>>) -> Result<()> { Ok(()) }
}

#[derive(Clone)]
struct Tick;
struct Src;

fn main() {}
