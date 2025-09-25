# mmg-microbus 全流程全特性文档（权威参考）

本篇为框架的全流程、全特性、全出入口说明。与实现一一对应，作为唯一且权威的参考文档。

## 设计目标与核心理念
- 写业务即配置：出现 `&T` 即订阅类型 T；返回值即发布。
- 极简强约束：仅按类型路由（无寻址、实例过滤、外部发布或订阅接口）。
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
- 组件单例自动发现：凡使用 `#[component]` 标注的结构体会在编译期登记并于 `start()` 自动实例化一次。
- （已移除）外部业务配置注入能力：不再有 `app.config()`；组件初始化所需数据由组件自身在 `#[init]` 内获取（硬编码 / 读取文件 / 环境变量等）。

2) 启动
- `app.start().await?`：
  - 初始化阶段：为每个组件调用其 `#[init]` 方法（若存在）。不再解析任何外部配置参数，`#[init]` 只能接受 `(self/&mut self)` + 可选 `&ComponentContext`。
  - 启动屏障：所有组件完成初始化与订阅装配后，统一越过启动屏障进入运行态；若任一 `#[init]` 返回错误，将标记启动失败，`start()` 立刻停止全局并返回 `Err`，不会进入运行期。
  - 订阅装配：扫描 `#[handle]` 方法签名建立类型级订阅。
  - 主动任务调度：`#[active]` 进入循环；`#[active(once)]` 启动后执行一次。

3) 运行期
- 被动消费：总线按“消息类型”将消息 fanout 给所有订阅者；对应 `#[handle]` 以消息 `&T` 为入参被调用。
- 主动生产：`#[active]` 周期（或一次）执行；其返回值按规则自动发布到总线。
- 返回值即发布（支持六类最小自然集合）：
  - `()` / `Result<()>` ：不发布；错误以 warn 记录。
  - `T` / `Result<T, E>` ：成功发布一条 `T`。
  - `Option<T>` / `Result<Option<T>, E>` ：`Some(T)` 成功才发布；`None` 静默。
  - 其它包装暂不支持（保持语义最小闭包）；需要扩展将通过新增注解或显式类型引入，而非隐式推断。

4) 停止
- `app.stop()`：设置内部原子停止标志并开始关闭全部组件（同步函数，不可 `await`）。
- 框架提供了 stop 钩子宏，stop 钩子的语义是同步方式的：一旦组件的 stop 钩子返回，等价于组件承认可以被退栈离开作用域的方式直接销毁；如果组件没有提供 stop 函数钩子，代表组件承认被随时强制退栈删除。
  - 合同（同步语义，禁止后台）：`#[stop]` 必须是同步函数（不得标记 async），只能修改内存内状态并释放资源（依赖 drop 语义），不得执行任何阻塞或网络 I/O，也不得启动任何新的后台任务；返回即表示组件可被直接丢弃。

## 宏与方法签名契约（出入口）
- `#[component]`（struct 与 impl 上）：
  - struct 必须实现 `Default` 以便框架构造；不得包含 id 字段。
  - impl 中的方法可使用以下注解：

- `#[handle]`（被动）：
  - 形参：可选 `&ComponentContext` + 恰好一个业务消息 `&T`；不允许多个业务参数；顺序不敏感；最多一个 Context。
  - 返回：见“返回值即发布”。

- `#[active]`（主动）：
  - 形参：仅可选 `&ComponentContext`；不允许业务 `&T` 参数。
  - 形式：
    - `#[active]` 无限循环：函数每次完成后立即再次调度（不做框架层退让；只有函数内部的 `await` 才会让出）。
    - `#[active(once)]` 单次执行：启动后执行一次，不再进入循环。
  - 不支持其它参数（出现即编译错误）。
  - 返回：见“返回值即发布”。

- `#[init]`（初始化）：
  - 形参：仅允许 `(self 或 &mut self)` 加可选 `&ComponentContext`。
  - 行为：框架在组件 run 进入主循环前调用一次；组件内部自行获取或构造所需配置；框架不感知来源与形式。
  - 返回：同统一六类（返回值即发布）。若返回 `Result::Err`，视为“启动失败”，应用不会进入运行期，`app.start()` 返回该错误。

- `#[stop]`（停止）：
  - 形参：仅可选 `&ComponentContext`。
  - 必须为同步函数（禁止 async）。
  - 返回：见“返回值即发布”。错误将记录为 warn，并不会影响全局停止流程。

注意：所有注解方法均为 async（框架统一以异步调度）。

## 总线与路由机制
- 唯一路由键：消息类型 `T`。
- 订阅登记：编译期通过宏生成注册代码；运行期在 `start()` 时完成。
- 投递策略：对每个 T，fanout 到所有订阅者；每个订阅者独立处理其收到的消息实例。
- 路由接口仅限类型 fanout（无地址、实例过滤、外部发布或订阅接口）。

## ComponentContext（只读能力）
- 无协作停机 / 取消接口：只在内部 stop 通知到达后退出。
- 无任意发布 / 动态订阅接口：路由绑定全部在启动阶段静态生成。
- 无反射逃逸：不提供 `as_any` 之类方法。

额外说明：启动屏障由框架内部管理，不对业务开放 API；其作用是确保“全部组件完成初始化与订阅装配后再统一进入运行期”。

## （已移除）外部业务配置注入
原先的 `app.config()` / `ConfigStore` / `#[init](&Cfg)` 模式已删除。现在：
- 若组件需要外部参数，可：
  1. 在 `#[init]` 中读取环境变量/文件；
  2. 使用编译期常量；
  3. 通过其它组件发布的消息在运行期渐进获取；
- 框架不再提供缺配置检测或存储。

## 停机（非协作）
- 停止：调用 `stop()` 结束；若存在 `#[stop]` 则调用后结束。`#[stop]` 必须遵循严格同步语义（只做内存态清理与释放；禁止后台动作）。

## 错误语义对齐（重要）
- 仅 `#[init]` 的错误会导致启动失败并使 `app.start()` 返回 `Err`。
- 运行期的 `#[handle]` / `#[active]` / `#[stop]` 的 `Result::Err` 会被记录为 `warn`，但不会打断系统运行或停止流程；其成功分支返回值仍按“返回即发布”的规则投递。

## 使用示例（最小闭环）
```rust
use mmg_microbus::prelude::*;

#[derive(Clone, Debug)] struct Tick(pub u64);

#[mmg_microbus::component]
#[derive(Default)]
struct AppComp { counter: std::sync::atomic::AtomicU64 }

#[mmg_microbus::component]
impl AppComp {
  #[mmg_microbus::init]
  async fn init(&self, _ctx: &mmg_microbus::component::ComponentContext) {
    // 自行初始化内部状态，这里无需外部配置
  }

  #[mmg_microbus::active] // 无限循环
  async fn tick(&self, _ctx: &mmg_microbus::component::ComponentContext) -> Option<Tick> {
    let v = self.counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    Some(Tick(v))
  }

  #[mmg_microbus::active(once)]
  async fn bootstrap(&self) -> Option<Tick> {
    // 单次执行：发布一次初始 Tick，然后不再调度
    Some(Tick(0))
  }

  #[mmg_microbus::handle]
  async fn on_tick(&mut self, _ctx: &mmg_microbus::component::ComponentContext, t: &Tick) {
    eprintln!("tick {}", t.0);
  }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> mmg_microbus::error::Result<()> {
  let mut app = App::new(Default::default());
  app.start().await?;
  app.stop();
  Ok(())
}
```

## 诊断与常见错误
- 方法签名报错：`#[handle]` 必须为（可选 Context + 恰好一个 `&T`）；`#[active]` 不允许业务参数；最多一个 Context。
- 收不到消息：确认组件已添加并完成 `start()`；确保消息类型匹配并存在生产方（主动函数返回或其它 handler 返回）。

## 边界与非目标
- 仅进程内（不含网络/IPC）。
- 不含幂等或重试；慢消费者产生背压。
- 不含字符串主题或动态类型擦除路由。

## 文档约定
- 本文（FULL_GUIDE）为机制与出入口的权威总览；一切以本文为准。
