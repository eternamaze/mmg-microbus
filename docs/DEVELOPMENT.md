# mmg-microbus 开发文档（维护者）

本文件面向维护者，记录实现细节与演进要点；用户请阅读 `docs/MANUAL.md`。

## 核心设计要点
- 组件模型：`#[component]` 为 struct+impl 生成 Component::run，方法即订阅（`#[handle]`）与主动函数（`#[active]`）。
- 配置：启动前 `App::config(T)` 冻结到只读仓库；仅 `#[init]` 可按 &T 注入。
- 总线：KindId+ComponentId 强类型路由；Address 表达两种语义：类型级（任意来源）与精确实例；发送阻塞直送，不丢包，小向量优化。
// 约束：已移除 from=Kind 路由约束；仅保留基于实例字符串的可选过滤。

## 代码约定
- 业务侧仅使用 `ctx.publish`；订阅 API 不对外暴露。
- 不做“订阅需有发布者”的强制检查，允许孤立订阅。
- 组件任务由 App 启动，停止时发送 shutdown 信号并等待自然退出。

## 目录结构与宏
- `microbus-macros`: 过程宏，生成 run/订阅/active 调度与路由约束。
- `src/app.rs`: App 生命周期、配置冻结、组件实例化。
- `src/bus.rs`: 路由与发送实现（类型级订阅 + 精确实例订阅）。
- `src/component.rs`: Component trait、上下文与自动订阅工具。
- `src/registry.rs`: 路由约束注册表（inventory）。

## 维护提示
- 添加新的宏参数时需同步更新 `MANUAL.md`。
- 保持 `AppConfig` 精简，避免遗留无效字段；变更启动流程时确保对 `config_many` 的幂等语义不变。
- 如引入指标/调试钩子，应默认关闭并避免侵入业务路径。

