# mmg-microbus 使用手册（最新版）

本手册是唯一权威文档，聚焦“写业务即配置”的强类型自动化微总线。内容仅呈现最终能力，不包含开发历史或兼容路径描述。

## 设计理念
- 业务代码是最强意图表达；框架自动识别语义并完成订阅/分发/配置注入。
- 注解只在代码表达力不足时补充元信息（来源过滤、实例、生命周期钩子）。
- 框架不侵入业务逻辑：类型安全、自动化、零接线。

## 10 行上手
```rust
use mmg_microbus::prelude::*;

#[derive(Clone, Debug)] struct Tick(pub u64);

#[mmg_microbus::component]
struct App { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::handles]
impl App {
  #[mmg_microbus::handle(Tick)]
  async fn on_tick(&mut self, tick: &Tick) -> anyhow::Result<()> {
  println!("tick {}", tick.0);
    Ok(())
  }
}
#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
  let mut app = mmg_microbus::prelude::App::new(Default::default());
  app.add_component::<App>("app");
  app.start().await?;
  // ...
  app.stop().await;
  Ok(())
}
```

## 方法即订阅（唯一路径）
1) 标注组件：`#[component] struct S { id: ComponentId }`
2) 写处理函数：`#[handles] impl S { async fn on_xxx(&mut self, ...T...) -> Result<()> }`
3) 可选过滤：`#[handle(T, from=Kind, instance=MarkerType)]`（`MarkerType` 需实现 `InstanceMarker`）
4) 启动：自行提供 Tokio 入口，使用 `App::new + add_component::<T>(id) + start()/stop()` 进行生命周期控制。

签名即语义：
- 参数形态：`&T`
- 可注入：`&ComponentContext`
- 当签名无法推断时显式写 `#[handle(T)]`

## 组件是一等公民：主动消息源
`#[handles]` 是语法糖，会为你的结构体自动生成 `Component::run` 并把消息分发到标注的方法，适合“被动响应”型组件。要实现“主动推送”的消息源（例如定时采集/轮询外部系统），请直接实现组件的 `run()`，在其中使用 `ctx.publish(...)` 主动向总线发消息；生命周期方面可使用“自动随停订阅/优雅睡眠”避免显式处理关停逻辑。

最小示例：
```rust
#[derive(Clone, Debug)]
struct Tick(pub u64);

#[mmg_microbus::component]
struct Feeder { id: mmg_microbus::bus::ComponentId }

#[async_trait::async_trait]
impl mmg_microbus::component::Component for Feeder {
  fn id(&self) -> &mmg_microbus::bus::ComponentId { &self.id }
  async fn run(self: Box<Self>, mut ctx: mmg_microbus::component::ComponentContext) -> anyhow::Result<()> {
    let mut n = 0u64;
    let mut intv = tokio::time::interval(std::time::Duration::from_millis(200));
    loop {
      tokio::select! {
  _ = intv.tick() => { n += 1; ctx.publish(Tick(n)).await; }
  // 可选：若有等待场景，优先使用 ctx.graceful_sleep(..) 或 auto 订阅的 recv()
      }
    }
    Ok(())
  }
}

#[mmg_microbus::component]
struct Trader { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::handles]
impl Trader {
  #[mmg_microbus::handle(Tick, from=Feeder)]
  async fn on_tick(&mut self, _tick: &Tick) -> anyhow::Result<()> {
    Ok(())
  }
}
```
要点：
- 主动源写自定义 `run`，消费方仍可用 `#[handles]` 订阅；这体现了“组件是一等公民，handlers 只是语法糖”的设计。
- IoC 友好停机：若使用手写 `run`，可通过 `ctx.subscribe_*_auto()` 获取“自动随停订阅”，或用 `sub.recv_or_shutdown(&ctx.shutdown)`；也可用 `ctx.graceful_sleep(dur)` 在停机时提前返回，避免显式处理 `shutdown` 分支。

## 配置即对象（启动前一次性注入）
- 在 handler 形参中直接声明 `&MyCfg` 即可自动注入配置对象。
- 通过 `app.provide_config(MyCfg { ... }).await?` 在启动前注入一次；运行期不支持热更新。

示例：
```rust
#[derive(Clone)]
struct MyCfg { queue_capacity: usize }

#[mmg_microbus::handles]
impl Worker {
  async fn on_tick(&mut self, _t: &Tick, cfg: &MyCfg) -> anyhow::Result<()> {
    // 使用 cfg
    Ok(())
  }
}

let mut app = App::new(Default::default());
app.provide_config(MyCfg { queue_capacity: 256 }).await?;
```

## 运行语义与性能
- 发送：阻塞直送（不丢包）；小型有界队列仅吸收调度抖动。
- 清理：按需清理；检测到 `sender` 关闭即修剪。
- 性能：单订阅快路径；`SmallVec` 降低 fanout 分配；最后一次 `move` 避免多余 `Arc` 克隆。
 

## 强类型路由与模式订阅
- 唯一地址模型：`Address { service: Option<KindId>, instance: Option<ComponentId> }`
- 精确地址：`Address::of_instance::<S, I>()`；模式：`Address::for_kind::<S>()` / `Address::any()`。

示例：
```rust
use mmg_microbus::bus::Address;
let mut sub = ctx.subscribe_pattern::<Tick>(Address::for_kind::<Feeder>()).await;
let mut sub = ctx.subscribe_pattern_auto::<Tick>(Address::for_kind::<Feeder>()).await;
while let Some(tick) = sub.recv().await { /* use tick; 自动随停 */ }
```

## 宏快速参考（用户可用）
- #[component]
  - 目标：注册一个组件工厂，使框架能发现并实例化组件。
  - 要求：结构体需含字段 `id: ComponentId`；可选 `cfg` 字段（用于保存外部配置）。
- #[handles]
  - 目标：为一个 `impl` 块内的处理方法生成 `Component::run` 与订阅/分发逻辑。
  - 形参注入：可接受 `&ComponentContext`，以及消息参数 `&T`。
  - 与 #[handle] 联合使用（见下）。
- #[handle(T, from=Kind, instance=Marker)]
  - 目标：为方法声明处理的消息类型与（可选）来源过滤；`Marker` 为实现了 `InstanceMarker` 的零尺寸类型。
  - 备注：当方法签名无法推断 T 时必须显式写 `T`。
// 配置注入无需宏；直接在方法签名以 `&CfgType` 声明即可。

## 故障排查
- 没收到消息：
  - 方法参数需为 `&T`；必要时显式写 `#[handle(T)]`。
  - 检查过滤是否过严（from/instance）。
- 配置未生效：确认在 handler 签名中使用 `&CfgType`，并已在启动前通过 `app.provide_config(CfgType { .. })` 注入。

## 边界（刻意不做）
- 同进程强类型总线；不含网络/跨进程。
- 无背压/重试/幂等器；业务慢则阻塞自身发送。
- 仅强类型寻址；不支持字符串主题或动态类型擦除路径。

## 示例
- 示例：
  - `examples/final_showcase.rs`（来源过滤、实例约束、上下文注入、配置与外部发布）。
  - `examples/active_source.rs`（组件是一等公民；自定义 run 主动推送消息，其他组件用 #[handles] 订阅处理）。

---

本手册即为本项目唯一权威说明书。
