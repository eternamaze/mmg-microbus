use mmg_microbus::prelude::*;

#[derive(Clone)]
struct Tick;

#[mmg_microbus::component]
struct C { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::component]
impl C {
    async fn on_tick(&mut self, _tick: &Tick) -> Result<()> { Ok(()) }
}

fn main() {}
