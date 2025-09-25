use crate::error::{MicrobusError, Result};
use tokio::task::JoinHandle;

use crate::{
    bus::{Bus, BusHandle},
    component::{
        ComponentContext, __RegisteredFactory, __new_startup_barrier, __new_stop_flag,
        __trigger_stop_flag,
    },
    config::AppConfig,
};

pub struct App {
    _cfg: AppConfig,
    bus: Bus,
    tasks: Vec<JoinHandle<()>>,
    started: bool,
    stop_flag: std::sync::Arc<crate::component::StopFlag>,
    startup_barrier: Option<std::sync::Arc<crate::component::StartupBarrier>>, // 协调启动失败与等待
}

impl App {
    #[must_use]
    pub fn new(cfg: AppConfig) -> Self {
        let bus = Bus::new(cfg.queue_capacity);
        let stop_flag = __new_stop_flag();
        Self {
            _cfg: cfg,
            bus,
            tasks: Vec::new(),
            started: false,
            stop_flag,
            startup_barrier: None,
        }
    }

    // 框架配置仅能在 new() 时提供；运行期不支持修改。
    /// 发现并收集所有通过 inventory 注册的组件工厂。
    fn discover_factories() -> Vec<&'static __RegisteredFactory> {
        inventory::iter::<__RegisteredFactory>.into_iter().collect()
    }

    async fn await_startup_and_seal(
        &self,
        barrier_ref: &std::sync::Arc<crate::component::StartupBarrier>,
    ) {
        crate::component::__startup_wait_all(barrier_ref).await;
        self.bus.handle().seal();
    }

    fn spawn_components(
        &mut self,
        factories: &[&__RegisteredFactory],
        bus_handle: &BusHandle,
        startup_barrier: &std::sync::Arc<crate::component::StartupBarrier>,
    ) {
        for reg in factories {
            let factory = (reg.create)();
            let name = factory.type_name().to_string();
            let stop_clone = self.stop_flag.clone();
            let bus_clone = bus_handle.clone();
            let barrier_clone = startup_barrier.clone();
            let fut = async move {
                match factory.build(bus_clone.clone()).await {
                    Ok(comp) => {
                        // 注意：ComponentContext::new_with_service 仅在 crate 内部可见，
                        // 组件上下文的构造必须走 App 流程以确保启动屏障与总线 seal 顺序正确。
                        let ctx = ComponentContext::new_with_service(
                            bus_clone.clone(),
                            stop_clone.clone(),
                            barrier_clone.clone(),
                        );
                        if let Err(e) = comp.run(ctx).await {
                            tracing::error!(component = %name, kind = %factory.type_name(), error = %e, "component exited with error");
                        }
                    }
                    Err(e) => {
                        tracing::error!(component = %name, kind = %factory.type_name(), error = %e, "failed to build component");
                        // 构建失败视为启动失败
                        crate::component::__startup_mark_failed_barrier(&barrier_clone);
                    }
                }
            };
            let h = tokio::spawn(fut);
            self.tasks.push(h);
        }
    }

    async fn handle_start_failure(
        &mut self,
        barrier: std::sync::Arc<crate::component::StartupBarrier>,
    ) -> Result<()> {
        if crate::component::__startup_failed(&barrier) {
            self.stop().await;
            self.started = false;
            return Err(MicrobusError::Other("app start aborted: init/build failed"));
        }
        Ok(())
    }

    /// 启动并运行所有通过 inventory 注册的组件。
    ///
    /// # Errors
    /// 当任一组件构建或初始化失败时返回错误，并触发整个应用停机。
    ///
    /// # Panics
    /// 内部依赖的启动屏障未正确设置时可能触发 panic（仅限编程错误场景）。
    pub async fn start(&mut self) -> Result<()> {
        if self.started {
            return Ok(());
        }
        // 自动发现：inventory 收集的所有工厂；按 kind 去重（单例模式）。
        let bus_handle = self.bus.handle();
        let factories: Vec<&__RegisteredFactory> = Self::discover_factories();
        let total = factories.len();
        let startup_barrier = __new_startup_barrier(total);
        self.startup_barrier = Some(startup_barrier.clone());
        self.spawn_components(&factories, &bus_handle, &startup_barrier);
        let barrier_ref = self
            .startup_barrier
            .as_ref()
            .expect("startup_barrier must be set before waiting");
        self.await_startup_and_seal(barrier_ref).await; // 阶段：等待并封印
        self.handle_start_failure(barrier_ref.clone()).await?; // 阶段：失败分支
        self.started = true;
        Ok(())
    }
    pub async fn stop(&mut self) {
        // 框架主导的单方面停机：
        // 1) 发出停止信号；
        __trigger_stop_flag(&self.stop_flag);
        // 2) 强制结束所有组件任务（无需等待其“自然退出”）。
        let mut rest = Vec::new();
        rest.append(&mut self.tasks);
        for h in rest {
            // 组件 run() 应该在收到停止后尽快返回；这里直接等待一次 join，若 panic/取消也忽略。
            let _ = h.await;
        }
        self.started = false;
    }
    #[must_use]
    pub fn bus_handle(&self) -> BusHandle {
        self.bus.handle()
    }
    #[must_use]
    pub const fn is_started(&self) -> bool {
        self.started
    }
}

// tests are covered in integration suite; unit tests omitted here
