# 统一组件与生命周期收敛（临时设计稿）

目的：
- 用单一 #[component] 注解统一“完整组件/语法糖组件”。
- 以函数签名表达订阅语义：入参 &T 即订阅 T；可注入 &ComponentContext。配置仅在 #[init] 注入并由组件状态持有。
- 返回值即响应：返回 T/Result<T> 自动发布；返回 ()/Result<()> 不发布，仅记录错误。
- 生命周期低侵入：start/stop + ctx.{subscribe_*_auto, ticker, graceful_sleep, spawn_until_shutdown}。
- 路由对外按“类型为主、实例可选”，内部仍可保留 Address 以实现。

范围：
- 宏：#[component] 支持 struct 与 impl；#[handle(T, from=Kind, instance=..)] 仅补充无法推断的来源过滤。instance 支持 marker 或字符串名。
- 订阅约束：一个 `#[handle]` 方法仅订阅一个消息类型（签名中只允许一个 `&T`）。
- 配线策略：放宽“订阅需有发布者”的检查；允许孤立订阅。
- 运行库：私有化 ComponentContext 字段；新增访问器；新增高层生命周期助手；保留强类型总线实现。
- 文档：收敛到统一注解与低侵入 API；移除 #[handles] 用法。

未完成/下一步：
- 单例默认与多实例冲突解析：在 #[component(...)] 支持 instance="name"；宏按“类型即默认实例”推断路由，出现多实例时要求显式 instance。
- 示例与测试全面迁移到 #[component]；移除旧 UI 测试对 #[handles] 的引用。
- 对外 API 隐藏 Address（保留内部），对使用者提供基于类型/可选实例的订阅构造器糖。

迁移提示：
- 将 `#[handles] impl S` 改为 `#[component] impl S`。
- 需要发布返回值时，将 `Ok(Out)` 换成直接 `Ok(Out)`（宏自动发布），无需额外调用；若需不发布返回值，返回 `()` 或 `Result<()>`。

此文档为开发期临时说明，重构完毕后删除。
