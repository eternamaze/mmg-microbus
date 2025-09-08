use mmg_microbus::prelude::*;

#[derive(Clone)]
struct Tick;

#[mmg_microbus::component]
#[derive(Default)]
struct C;

#[mmg_microbus::component]
impl C {
    #[mmg_microbus::handle]
    async fn on_tick(&mut self, _ctx: &mmg_microbus::component::ComponentContext, _tick: &Tick) -> Result<()> { Ok(()) }
}

fn main() {}
