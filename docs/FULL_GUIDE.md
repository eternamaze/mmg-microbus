# mmg-microbus 全流程全特性文档（权威参考）

本篇为框架的全流程、全特性、全出入口说明。与实现一一对应，作为唯一且权威的参考文档。

## 设计目标与核心理念
- 写业务即配置：出现 `&T` 即订阅类型 T；返回值即发布。
- 极简强约束：去除寻址/实例过滤/外部发布与订阅；仅保留“按类型路由”。
- 业务解耦：业务类型移除注解即可独立编译（未使用 Context 时）。
- 只读上下文：不提供停机协作与副作用能力。

## 名词与约定
- 消息 Message：任意业务类型 T（建议 `Debug` 便于排错）。
- 组件 Component：带有 `#[component]` 注解的 struct 及其 impl。
- 应用 App：组件的容器与调度者，负责配置注入、生命周期管理、总线路由。
- 上下文 ComponentContext：只读工具集合；无停机协作能力。

## 生命周期全流程
1) 组装
- 构造 App：`let mut app = App::new(Default::default());`
- 注册组件：`app.add_component::<MyComp>("id");`（id 为运行期唯一字符串；业务结构体不含 id 字段）。
- 注入配置（可选、可多种类型）：`app.config(MyCfg { .. }).await?`（同类型后写覆盖，最后值生效）。

2) 启动
- `app.start().await?`：执行下列步骤：
  - 初始化：为每个组件调用 `#[init]`（若存在）。若该组件声明了 `&Cfg` 但未注入，启动失败返回错误。
  - 订阅装配：扫描组件方法签名，凡 `#[handle]` 且含单个 `&T`，即向总线注册对 T 的订阅。
  - 主动任务调度：为每个 `#[active]` 方法创建调度器（支持 `interval_ms`/`times`/`immediate`/`once`）。

3) 运行期
- 被动消费：总线按“消息类型”将消息 fanout 给所有订阅者；对应 `#[handle]` 以消息 `&T` 为入参被调用。
- 主动生产：`#[active]` 周期（或一次）执行；其返回值按规则自动发布到总线。
- 返回值即发布：任意被注解的方法返回值会发布：
  - `T` -> 发布 `T`
  - `Option<T>` -> `Some(T)` 才发布
  - `Result<T, E>` -> `Ok(T)` 才发布
  - `Result<Option<T>, E>` -> `Ok(Some(T))` 才发布

4) 停止
- `app.stop().await`：框架直接下达停止指令；若组件提供 `#[stop]` 则同步调用一次，返回即视为结束，随后总线丢弃该组件；未提供则直接丢弃。

## 宏与方法签名契约（出入口）
- `#[component]`（struct 与 impl 上）：
  - struct 必须实现 `Default` 以便框架构造；不得包含 id 字段。
  - impl 中的方法可使用以下注解：

- `#[handle]`（被动）：
  - 形参：可选 `&ComponentContext` + 恰好一个业务消息 `&T`；不允许多个业务参数；顺序不敏感；最多一个 Context。
  - 返回：见“返回值即发布”。

- `#[active]`（主动）：
  - 形参：仅可选 `&ComponentContext`；不允许业务 `&T` 参数。
  - 调度参数：`interval_ms = N`，`times = N`，`immediate`，`once`（等效 `times=1, immediate=true`）。
  - 返回：见“返回值即发布”。

- `#[init]`（初始化）：
  - 形参：可选 `&ComponentContext` + 恰好一个 `&Cfg`（业务配置类型）。
  - 行为：App 启动前从配置仓库取出 `Cfg` 调用该方法；缺失则启动失败。
  - 建议：将 `Cfg` 克隆/拷贝到组件状态以供后续使用。
  - 返回：见“返回值即发布”。

- `#[stop]`（停止）：
  - 形参：仅可选 `&ComponentContext`。
  - 返回：见“返回值即发布”。

注意：所有注解方法均为 async（框架统一以异步调度）。

## 总线与路由机制
- 唯一路由键：消息类型 `T`。
- 订阅登记：编译期通过宏生成注册代码；运行期在 `start()` 时完成。
- 投递策略：对每个 T，fanout 到所有订阅者；每个订阅者独立处理其收到的消息实例。
- 不提供：Address/ServiceAddr/实例过滤/外部发布与订阅 API。

## ComponentContext（只读能力）
- 不提供任何停机协作 API（无 graceful_sleep、无 ticker）。
- 不提供 `spawn` / 外部发布/订阅 等会产生副作用的能力。

## 配置注入模型
- 框架配置（AppConfig）：仅在 `App::new(AppConfig)` 提供；不再通过 `app.config(..)` 识别框架配置类型。
- 业务配置：`app.config(Cfg { .. }).await?`；同类型多次写入后写覆盖（建议在日志层给出覆盖提示）。
- 读取：仅框架在调用 `#[init]` 时按声明类型读取；业务运行期不可随意读取。
- 失败：若组件声明了 `#[init](&Cfg)` 但实际缺失该类型配置，`start()` 返回错误。

## 停机（非协作）
- 框架单方面控制停机：调用 `stop()` 即结束；如提供 `#[stop]` 则同步调用一次后立刻丢弃组件。

## 使用示例（最小闭环）
```rust
use mmg_microbus::prelude::*;

#[derive(Clone, Debug)] struct Tick(pub u64);
#[derive(Clone, Debug, Default)] struct Cfg { n: u64 }

#[mmg_microbus::component]
#[derive(Default)]
struct AppComp { cfg: Option<Cfg> }

#[mmg_microbus::component]
impl AppComp {
  #[mmg_microbus::init]
  async fn init(&mut self, _ctx: &mmg_microbus::component::ComponentContext, cfg: &Cfg) {
    self.cfg = Some(cfg.clone());
  }

  #[mmg_microbus::active(immediate, interval_ms=100, times=3)]
  async fn tick(&mut self, _ctx: &mmg_microbus::component::ComponentContext) -> Option<Tick> {
    Some(Tick(self.cfg.as_ref().map(|c| c.n).unwrap_or(0)))
  }

  #[mmg_microbus::handle]
  async fn on_tick(&mut self, _ctx: &mmg_microbus::component::ComponentContext, t: &Tick) {
    eprintln!("tick {}", t.0);
  }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
  let mut app = App::new(Default::default());
  app.add_component::<AppComp>("app");
  app.config(Cfg { n: 42 }).await?;
  app.start().await?;
  app.stop().await;
  Ok(())
}
```

## 诊断与常见错误
- 启动报缺配置：某组件 `#[init]` 声明了 `&Cfg`，但未通过 `app.config(Cfg{..})` 注入。
- 方法签名报错：`#[handle]` 必须为（可选 Context + 恰好一个 `&T`）；`#[active]` 不允许业务参数；最多一个 Context。
- 收不到消息：确认组件已添加并完成 `start()`；确保消息类型匹配并存在生产方（主动函数返回或其他 handler 返回）。

## 边界与非目标
- 仅进程内；不含网络/IPC。
- 不提供幂等器/重试；慢消费者会形成自身背压。
- 不提供字符串主题/动态类型擦除的路由。

## 文档约定
- 本文（FULL_GUIDE）为机制与出入口的权威总览；一切以本文为准。
