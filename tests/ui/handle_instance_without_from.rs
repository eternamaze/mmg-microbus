use mmg_microbus::prelude::*;

#[mmg_microbus::component]
struct C { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::component]
impl C {
    #[mmg_microbus::handle(Tick, instance="x")] // ERROR: instance without from
    async fn on_tick(&mut self, _tick: &Tick) -> Result<()> { Ok(()) }
}

#[derive(Clone)]
struct Tick;

fn main() {}
