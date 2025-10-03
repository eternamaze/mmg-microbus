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
### 返回值即发布：统一“归约到 T 或 ()”模型

框架在运行期只区分两类最终行为：
1. 产生一条业务消息 `T` 并发布。
2. 不产生消息（等价于返回 `()`）。

所有返回值类型（含包装 / 动态）都会按规则被“解包并归约”到上述两类之一。

当前支持的发布契约统一归约为“7 类返回值语义族”，其本质仍是：能产出业务消息 `T` 或不产出（视为 `()`）。下表是与“总线契约指南（短版）”一致的权威映射：

| 类别 | 返回值原型 | 归约结果 | 发布行为 |
|------|------------|----------|----------|
| 1 | `()` / `Result<()>` | 空 | 不发布（Err -> warn） |
| 2 | `T` / `Result<T,E>` | `T` | 发布单条 `T`（Err -> warn） |
| 3 | `Option<T>` / `Result<Option<T>,E>` | Some -> `T`; None -> 空 | Some 发布，None 不发布 |
| 4 | `ErasedEvent` / `Option<ErasedEvent>` / `Vec<ErasedEvent>` (+ `Result<_>`) | 展开为 0..n 条真实 `U` | 逐个发布；空 = 不发布 |
| 5 | `Box<dyn Any + Send + Sync>` / `Arc<dyn Any + Send + Sync>` | downcast 成功 -> `U` | 成功发布；失败静默丢弃 |
| 6 | `Option<Box<dyn Any>>` / `Option<Arc<dyn Any>>` (+ `Result<_>`) | Some -> 按 5；None -> 空 | 成功分支同 5 |
| 7 | `Result<动态族, E>`（动态族 = 4/5/6 之一） | Ok -> 继续按其内部族；Err -> 空 | Err 记录 warn，不发布 |

补充说明（统一）：
1. 所有包装（Result / Option / Any / ErasedEvent / Vec）只是过渡层；最终只落在 “发布一条或多条具体 T” 与 “不发布” 两种。
2. 动态族（Any 路径）在运行时只做一次 `TypeId` 精确匹配 + downcast；失败静默（保证弱类型实验不影响生产稳定订阅）。
3. `ErasedEvent` 设计用于“一次函数返回里需要发布多种静态类型”场景；通过函数指针携带发布路径，downcast 后复用静态快路径。
4. `Vec<ErasedEvent>` 的元素按向量顺序依次发布；不保证与其他并行 active 的跨类型全序（若业务需要全局顺序，应在调用方自定义的上层串行入口完成）。
5. 所有运行期 `Err`（除 init）降级为 warn；不触发停机；使治理逻辑与业务解耦。
6. 上层系统若需要“源发事件串行化”（例如统一驱动或回放），应在其自身边界实现；microbus 仅提供类型 fanout，不提供跨类型排序与外部发布接口。
7. 若需要跨多类型条件分支试验，快速阶段可用 Any；进入稳定阶段需迁移到明确结构体或 `ErasedEvent` 列举，提升可审计性。

不支持（拒绝构建或忽略）示例：
- 嵌套包装（如 `Option<Vec<T>>` 内再含 `ErasedEvent`）——保持最小语义闭包；需要请先在业务层拆解。
- 自定义容器（非 `Vec`）的 `ErasedEvent` 集合——暂未纳入契约集合，待确有需求再 add-only 扩展。

### 设计原则补充（统一与短版指南）
1. 包装类型 = 归约工具；终点只剩 `T` 或 空。
2. 弱类型 Any 不引入新路由维度：仍按内部真实 `TypeId` 执行 fanout。
3. 若上层需要可重放确定性，应自行建立单一串行事件输入（例如集中驱动器）；microbus 不构建跨类型全序。
4. Any 适用于“快速探索 / 多类型条件输出”阶段；稳定后应显式化（结构体或 ErasedEvent 列举）。
5. `ErasedEvent` 保障多类型批量返回仍保留静态安全与快路径性能。
6. 错误治理：除 init 外的 `Err` 降级 warn；回放与实时路径行为一致（可复现）。

### 性能与安全
1. 静态路径：直接索引订阅表，零额外装箱。
2. Any 路径：一次 HashMap 查询 + downcast；无订阅时立即返回（静默）。
3. ErasedEvent：downcast 成功后复用静态 publish 快路径（在 sealed 阶段不再清理订阅快照，减少锁竞争）。
4. panic 仅限编程期错误（ErasedEvent 指针与数据不匹配）；运行期语义错误不使用 panic。
5. 上层串行化（如集中驱动器）才是全局重放保障；microbus 不试图解决跨类型排序或一致性日志，这些属于调用方架构职责。

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
- 唯一路由键：消息类型 `T`（静态）+ 运行时从 `Box/Arc<dyn Any>` 或 `ErasedEvent` 下钻出的实际 `T`。
- 订阅登记：编译期通过宏生成注册代码；运行期在 `start()` 时完成。
- 投递策略：对每个静态 T fanout；动态消息（Any / ErasedEvent）在归约后再进入同一静态路径。
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
- 动态返回未发布：检查返回的 `Box/Arc<dyn Any>` 实际内部类型是否已被任何 `#[handle]` 订阅；若无订阅静默丢弃属预期；需要时请添加显式订阅或改用显式 `ErasedEvent`。
- Vec<ErasedEvent> 未触发发布：确认向量非空；空向量即“无输出”语义。

## 边界与非目标
- 仅进程内（不含网络/IPC）。
- 不含幂等或重试；慢消费者产生背压。
- 不含字符串主题或动态类型擦除路由。

## 文档约定
- 本文（FULL_GUIDE）为机制与出入口的权威总览；一切以本文为准。
