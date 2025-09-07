# mmg-microbus 使用手册（最新版）

本手册是唯一权威文档，聚焦“写业务即配置”的强类型自动化微总线。仅呈现最终能力，不包含兼容路径或历史实现。

## 设计理念
- 业务代码是最强意图表达；框架自动识别语义并完成订阅/分发；配置仅在 #[init] 阶段注入。
- 注解只在代码表达力不足时补充元信息（来源过滤、实例、生命周期钩子）。
- 框架不侵入业务逻辑：类型安全、自动化、零接线。

## 10 行上手
```rust
use mmg_microbus::prelude::*;

#[derive(Clone, Debug)] struct Tick(pub u64);

#[mmg_microbus::component]
struct App { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::component]
impl App {
  #[mmg_microbus::handle(Tick)]
  async fn on_tick(&mut self, tick: &Tick) -> anyhow::Result<()> {
    println!("tick {}", tick.0);
    Ok(())
  }
}
#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
  let mut app = App::new(Default::default());
  app.add_component::<App>("app");
  app.start().await?;
  // ...
  app.stop().await;
  Ok(())
}
```

## 方法即订阅（唯一路径）
1) 标注组件：`#[component] struct S { id: ComponentId }`
2) 写处理函数：`#[component] impl S { #[handle(T)] async fn on_xxx(&mut self, &T) -> Result<()> }`
3) 可选过滤：`#[handle(T, from=Kind, instance=MarkerType)]`（`MarkerType` 需实现 `InstanceMarker`）
4) 启动：自行提供 Tokio 入口，使用 `App::new + add_component::<T>(id) + start()/stop()` 进行生命周期控制。

签名即语义：
- 参数形态：`&T`
- 可注入：`&ComponentContext`
- 当签名无法推断时显式写 `#[handle(T)]`

## 组件是一等公民：主动消息源
`#[component]` 标注在 impl 上会为你的结构体自动生成 `Component::run` 并把消息分发到方法，适合“被动响应”型组件；同一组件内也可写主动函数 `#[active(..)]`，由框架调度循环，主动/被动统一为“组件是一等公民”。

示例：
```rust
#[derive(Clone, Debug)]
struct Tick(pub u64);

#[mmg_microbus::component]
struct Feeder { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::component]
impl Feeder {
  #[mmg_microbus::active(interval_ms=200, immediate=true)]
  async fn pump(&self, ctx: &mmg_microbus::component::ComponentContext) -> anyhow::Result<()> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    ctx.publish(Tick(n)).await;
    Ok(())
  }
}

#[mmg_microbus::component]
struct Trader { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::component]
impl Trader {
  #[mmg_microbus::handle(Tick, from=Feeder)]
  async fn on_tick(&mut self, _tick: &Tick) -> anyhow::Result<()> {
    Ok(())
  }
}
```

要点：
- 主动函数无需手写 `run`，由 #[active] 调度；消费方用 #[handle] 即可处理。
- IoC 友好停机：主动与被动均会在 App 停止时自动退出；无需在业务侧书写显式 shutdown 分支。

## 配置即对象（仅在 #[init] 注入，一次性）
- 在启动前通过 `app.config(MyCfg { ... }).await?`（或 `app.config_many(|a| Box::pin(async move { a.config(CfgA{..}).await?; a.config(CfgB{..}).await }))`）提供配置到全局配置仓库。
- 仅 #[init] 支持以 `&MyCfg` 形参接收配置（最多一个配置类型）；请在 #[init] 中把配置保存到组件状态，供后续 #[handle]/#[active] 使用。

示例：
```rust
#[derive(Clone)]
struct MyCfg { queue_capacity: usize }

#[mmg_microbus::component]
struct Worker { id: mmg_microbus::bus::ComponentId, cfg: Option<MyCfg> }

#[mmg_microbus::component]
impl Worker {
  #[mmg_microbus::init]
  async fn init(&mut self, cfg: &MyCfg) -> anyhow::Result<()> {
    self.cfg = Some(cfg.clone());
    Ok(())
  }

  #[mmg_microbus::handle(Tick)]
  async fn on_tick(&mut self, _t: &Tick) -> anyhow::Result<()> {
    let cap = self.cfg.as_ref().map(|c| c.queue_capacity).unwrap_or(0);
    // 使用 cap 进行业务处理
    Ok(())
  }
}

let mut app = App::new(Default::default());
app.config(MyCfg { queue_capacity: 256 }).await?;

// 或批量提供（薄包装，内部逐项调用 config）：
app.config_many(|a| Box::pin(async move {
  a.config(MyCfg { queue_capacity: 256 }).await?;
  Ok(())
})).await?;
```

## 运行语义与性能
- 发送：阻塞直送（不丢包）；小型有界队列仅吸收调度抖动。
- 清理：按需清理；检测到 `sender` 关闭即修剪。
- 性能：单订阅快路径；`SmallVec` 降低 fanout 分配；最后一次 `move` 避免多余 `Arc` 克隆。
 

## 强类型路由与模式订阅
- 唯一地址模型：`Address { service: Option<KindId>, instance: Option<ComponentId> }`
- 精确地址：`Address::of_instance::<S, I>()`；模式：`Address::for_kind::<S>()` / `Address::any()`。

（内部机制）框架使用模式订阅与强类型路由，但业务代码无需直接调用订阅 API。

## 宏快速参考（用户可用）
- #[component]
  - 目标：注册一个组件工厂，使框架能发现并实例化组件。
  - 要求：结构体需含字段 `id: ComponentId`；其余字段可自定义（如用于保存配置）。
- #[active(interval_ms=.., times=.., immediate=..)]
  - 目标：声明主动函数的调度策略；由框架生成 ticker 驱动的循环。
  - 形参注入：仅可接受 `&ComponentContext`。
- #[handle(T, from=Kind, instance=Marker)]
  - 目标：为方法声明处理的消息类型与（可选）来源过滤；`Marker` 为实现了 `InstanceMarker` 的零尺寸类型。
  - 备注：当方法签名无法推断 T 时必须显式写 `T`。
// 配置注入通过 #[init] 完成；#[handle]/#[active] 不支持 `&CfgType` 形参。

### 订阅规则与检查策略（重要）
- 单函数单订阅：一个 `#[handle]` 方法只能订阅一个消息类型。方法签名中仅允许一个 `&T`（可选再加 `&ComponentContext`）。
- 配线检查放宽：框架允许订阅的消息在运行时没有任何发布者，应用可正常启动；如需约束来源，请使用 `from=/instance=` 过滤。

## 启停收敛（面向业务的低侵入）
- 启动/停止：仅使用 `App::start().await?` 与 `app.stop().await`。
- 被动组件：写 `#[handle]` 方法体；生命周期与订阅由框架自动生成的 `run` 托管。
- 主动组件：写 `#[active(..)]` 方法；无需手写循环与关停分支。

## 故障排查
- 没收到消息：
  - 方法参数需为 `&T`；必要时显式写 `#[handle(T)]`。
  - 检查过滤是否过严（from/instance）。
- 配置未生效：确认组件实现了 `#[init]` 接收 `&CfgType` 并把配置保存到状态，且在启动前通过 `app.config(CfgType { .. })` 注入。

## 边界（刻意不做）
- 同进程强类型总线；不含网络/跨进程。
- 无背压/重试/幂等器；业务慢则阻塞自身发送。
- 仅强类型寻址；不支持字符串主题或动态类型擦除路径。

## 配置幂等语义（重要）
- start 之后调用 `config`/`config_many` 将被忽略并打印警告（不生效）。
- start 之前，同一配置类型多次提供时，仅最后一次生效，并打印覆盖警告（便于定位误用）。

## 示例
- 示例：
  - `examples/all_in_one.rs`（来源过滤、实例约束、上下文注入、通过 #[init] 注入配置与外部发布，以及 #[active] 主动循环示例）。

## 进一步阅读
- 设计期望与使用规范：见 `docs/EXPECTATIONS.md`（注解放置规则、类型化注入、多实例语义、生命周期等）。

---

本手册即为本项目唯一权威说明书。
