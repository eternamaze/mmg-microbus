# 组件生命周期重构设计（init/active/handle/stop）

目标
- 框架全权掌控组件生命周期：初始化 -> 业务 -> 停止
- 组件编写者只需通过注解声明函数，框架自动推断参数/消息并调用
- 移除对“协作式退出”的依赖，Stop 由框架统一触发并等待完成

术语
- 主动业务（active）：由框架驱动的循环/定时任务
- 被动业务（handle）：由消息驱动的回调

注解与语义
- #[component]：用于 struct 与 impl，注册类型并生成运行时 glue
- #[init]：初始化回调，run() 主循环前调用一次，签名仅允许（接收器 self）+（可选一个 &CfgType）。不允许注入 `&ComponentContext`，也不支持多个配置类型。
- #[active(...)]：主动回调，参数仅允许 (&ComponentContext)，返回值自动发布为消息
- #[handle(T, from=ServiceType?, instance=?)]：被动回调，参数为 (&ComponentContext?, &T)，返回值自动发布为消息
- #[stop]：停止回调，组件主循环退出后调用一次，用于清理资源

参数推断与约束
- &ComponentContext：框架上下文
- &CfgType 仅在 #[init] 支持注入（且最多一个）；缺失时记录错误并跳过该回调。#[handle]/#[active] 不支持配置形参，请在 #[init] 中将配置保存到组件状态。
- 被动回调消息参数 &T：构成订阅；返回值类型构成发布；若订阅类型从未被框架内任何回调返回（无发布者），编译期将报错（后续版本提供静态分析；当前版本以运行期警告代替）

生命周期流程
- App::start：构造并运行组件任务
- 组件 run()：
  1) 调用所有 #[init]
  2) 建立 handle 订阅与 active tickers，进入主循环
  3) 接收到停机信号后跳出主循环
  4) 调用所有 #[stop]
- App::stop：发送停机信号，等待组件任务自然退出（不再 abort/linger）

迁移指南
- 旧：依赖 ctx.ticker()/AutoSubscription + 协作式退出
- 新：继续保留 handle/active 写法；若有收尾逻辑，请添加 #[stop] 方法
- 若组件无 init/stop，则框架调用为空动作

配置提供与初始化时机
- 使用 `app.config(T{..})` 或 `app.config_many(|a| Box::pin(async move { a.config(A{..}).await?; a.config(B{..}).await }))` 在启动前仅“提供参数”。
- 真正的“初始化”发生在 `app.start()` 的组件 run 开始之前，由框架统一调用 #[init] 并按类型注入参数，然后才进入主循环。
- 幂等语义：start 后调用 `config`/`config_many` 将被忽略并打印警告；start 前同类型多次仅保留最后一次并打印覆盖警告。

后续改进（计划）
- 提供编译期检查：被动订阅的类型若无发布者，编译失败
- 在 #[active] 支持 once/interval richer 组合
- 为 #[stop] 增加超时与并发策略可配置项

兼容性
- 对现有 #[component]/#[handle]/#[active] 兼容；新增 #[init]/#[stop]
- App::stop 行为变更：从 linger+abort 改为等待自然退出

测试建议
- 单元测试覆盖：
  - 组件提供 init/stop 的调用顺序
  - 停机时 active/handle 跳出后顺序进入 stop
  - stop 中的错误不会阻塞其他组件

  清理计划（逐项执行）
  - 移除 AppConfig.shutdown_linger_ms 的对外使用语义文档（代码中仍保留字段以兼容旧构造，但 stop 不再使用）
  - 删除旧的示例与文档中关于“协作式退出”的篇幅，统一指向 #[init]/#[stop]
  - 搜索仓库内对 ctx.ticker()/race_shutdown 的使用，必要时改为 #[active] 实现
  - 为所有内置示例组件补充空的 #[init]/#[stop] 以示范规范
