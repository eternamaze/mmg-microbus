mmg-microbus · 极简强类型自动化微总线  
[![CI](https://github.com/eternamaze/mmg-microbus/actions/workflows/ci.yml/badge.svg)](https://github.com/eternamaze/mmg-microbus/actions/workflows/ci.yml)

唯一路径：写业务函数即订阅（强类型、零接线）。完整说明见 `docs/MANUAL.md`。

10 行上手
```rust
use mmg_microbus::prelude::*;

#[derive(Clone, Debug)] struct Tick(pub u64);

#[mmg_microbus::component]
struct App { id: mmg_microbus::bus::ComponentId }

#[mmg_microbus::handles]
impl App {
  async fn on_tick(&mut self, env: std::sync::Arc<Envelope<Tick>>) -> anyhow::Result<()> {
    println!("tick {}", env.msg.0); Ok(())
  }
}

mmg_microbus::easy_main!();
```

完整示例：见 `examples/final_showcase.rs`（来源过滤、实例约束、上下文/`ScopedBus` 注入、配置与自发布）。

使用要点
- 参数形态：`&Envelope<T>`/`Arc<Envelope<T>>` 或 `&T`/`Arc<T>`；可注入 `&ComponentContext` 与 `&ScopedBus`。
- 过滤注解：使用类型标记 `#[handle(T, from=ServiceType, instance=MarkerType)]`（`MarkerType` 必须实现 `InstanceMarker`）。
- 配置注入：`#[configure(MyCfg)] + impl Configure<MyCfg>`；`App.config(cfg)` 启动前初始化或运行期热更（pause→broadcast→barrier→resume）。
- 运行语义：阻塞直送不丢包；按需清理，无周期扫描；单订阅快路径与小向量优化。
- 指标特性：`bus-metrics` 关闭为零成本；开启后记录发布/投递/延迟/暂停等待/fanout 等。

边界
- 同进程强类型总线；不含网络/跨进程。
- 无背压/重试/幂等器；业务慢则阻塞自身发送。
- 强类型寻址；不支持字符串主题与动态类型擦除路径。

许可证
- MIT
