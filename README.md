mmg-microbus · 极简强类型自动化微总线  
[![CI](https://github.com/eternamaze/mmg-microbus/actions/workflows/ci.yml/badge.svg)](https://github.com/eternamaze/mmg-microbus/actions/workflows/ci.yml)

唯一路径：写业务函数即订阅（强类型、零接线）。完整说明见 `docs/MANUAL.md`。

10 行上手
```rust
use mmg_microbus::prelude::*;

#[derive(Clone, Debug)] struct Tick(pub u64);

#[mmg_microbus::component]
struct App { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::component]
impl App {
  #[mmg_microbus::handle(Tick)]
  async fn on_tick(&mut self, tick: &Tick) -> anyhow::Result<()> {
  println!("tick {}", tick.0); Ok(())
  }
}
#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
  let mut app = mmg_microbus::prelude::App::new(Default::default());
  app.add_component::<App>("app");
  app.start().await?;
  // 业务发布/运行...
  app.stop().await;
  Ok(())
}
```

完整示例：见 `examples/all_in_one.rs`（来源过滤、实例约束、主动/被动函数、配置注入与自发布）。

使用要点
- 默认参数形态：`&T`；可按需注入 `&ComponentContext`。
- 过滤注解：使用类型标记 `#[handle(T, from=ServiceType, instance=MarkerType)]`（`MarkerType` 必须实现 `InstanceMarker`）。
- 配置注入：在 handler 形参中直接声明 `&MyCfg`；启动前通过 `app.provide_config(MyCfg { .. }).await?` 一次性注入（运行期不支持热更新，无需序列化/反序列化）。
- 运行语义：阻塞直送不丢包；按需清理，无周期扫描；单订阅快路径与小向量优化。
- 生命周期：由 `App` 统一启停；通常无需手写 `run`。主动函数使用 `#[active(..)]`，被动函数使用 `#[handle(..)]`，返回值自动发布。

更多规范
- 期望与约束见 `docs/EXPECTATIONS.md`（注解放置规则、类型化注入、多实例语义、生命周期等）。

推荐默认路径（面向使用者的一致心智）
- 发布：在 handler 中通过 `ComponentContext::publish(msg)`；外部通过 `App::bus_handle().publish(Address::of_instance::<S, I>(), msg)`。
- 订阅（被动 handler）：参数统一为 `&T`，按需使用 `#[handle(T, ...)]` 做来源过滤。
- 主动函数：使用 `#[active(interval_ms=.., times=.., immediate=..)]` 定义循环行为。

API 要点（单一路径）
- 仅 `&T` 形参；不支持 Envelope/ScopedBus。
- 统一地址模型 `Address`（对使用者默认按类型路由）；无需直接调用订阅 API。

边界
- 同进程强类型总线；不含网络/跨进程。
- 无背压/重试/幂等器；业务慢则阻塞自身发送。
- 强类型寻址；不支持字符串主题与动态类型擦除路径。

许可证
- MIT
