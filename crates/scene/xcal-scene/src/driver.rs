//! Driver pod：**命令驱动**的场景推进器（P4）。
//!
//! 持有 `Scheduler`（World + Resources）+ 帧计数，对外提供两个泛型 capability：
//!
//! | 能力 | 命令 / 事件 | 语义 |
//! |---|---|---|
//! | [`CommandQueue<DriverCommand>`] | `Advance` / `Batch` / `Seek` / `Reset` | 控制：FIFO、确定、参与帧推进 |
//! | [`Broadcast<DriverEvent>`] | `FrameFinished` / `Advanced` / `Error` | 通知：只做观测 |
//!
//! # 为什么是 Pod
//!
//! 「谁推进了时间」从调用栈里消失，变成显式的一条 `Advance(dt)` 命令。
//! GUI 与 headless **共用同一个 Driver pod**（`submit` 同一条命令），
//! 于是导出路径与实时路径在结构上不可能分叉。
//!
//! # 确定性
//!
//! `Scheduler::run_frame(dt)` 接受精确 `dt`：导出用固定 `dt = 1/fps`，
//! 比实时墙钟更确定。命令序列可录制（[`DriverPod::commands`]），
//! 用于比对导出与实时两条路径**过程相同**（不只是像素相同）。
//!
//! # 事件订阅
//!
//! 发布事件时按 [`EventSink<DriverEvent>`] 逐个 **typed** 调用订阅方，
//! 不跨 Pod 边界传闭包、无 `unsafe`。

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::sync::Arc;

use xc_pod::{
    Broadcast, Capability, CommandQueue, EventSink, FunctionId, Pod, PodError, PodId,
    QualifiedName, Receipt, negotiate_dyn,
};
use xcal_schedule::scheduler::Scheduler;

use crate::TimeState;
use crate::director_state::DirectorState;

/// 驱动命令（命令枚举由 domain 注入泛型 capability）。
#[derive(Clone, Debug, PartialEq, xc_pod_macro::PodType)]
#[pod_type(name = "xcal.driver.DriverCommand")]
pub enum DriverCommand {
    /// 推进一帧（精确 `dt` 秒）。
    Advance {
        /// 帧时长（秒）。
        dt: f32,
    },
    /// 连续推进 `frames` 帧（每帧同一 `dt`）。
    Batch {
        /// 帧数。
        frames: u32,
        /// 每帧时长（秒）。
        dt: f32,
    },
    /// 把全局时间拨到 `t`（不推进调度）。
    Seek {
        /// 目标时间（秒）。
        t: f64,
    },
    /// 回到初始状态（时间 0、恢复播放、帧计数归零）。
    Reset,
}

/// 驱动事件（只用于观测，不参与帧推进）。
#[derive(Clone, Debug, PartialEq, xc_pod_macro::PodType)]
#[pod_type(name = "xcal.driver.DriverEvent")]
pub enum DriverEvent {
    /// 一帧完成（`frame` 从 1 开始，`t` 是帧末全局时间）。
    FrameFinished {
        /// 已完成的帧数。
        frame: u32,
        /// 帧末全局时间（秒）。
        t: f32,
    },
    /// 一条命令已执行（`dt` 为该命令推进的时长；Seek / Reset 为 0）。
    Advanced {
        /// 推进时长（秒）。
        dt: f32,
    },
    /// 执行出错（明确消息，不静默）。
    Error {
        /// 错误消息。
        message: String,
    },
}

type SubmitFn = fn(&dyn Pod, DriverCommand) -> Receipt;
type PublishFn = fn(&dyn Pod, DriverEvent);

/// 命令驱动的场景推进器。
pub struct DriverPod {
    id: PodId,
    scheduler: RefCell<Scheduler>,
    frame: Cell<u32>,
    sinks: RefCell<Vec<Arc<dyn Pod>>>,
    commands: RefCell<Vec<DriverCommand>>,
    events: RefCell<Vec<DriverEvent>>,
    submit: SubmitFn,
    publish: PublishFn,
    functions: Vec<QualifiedName>,
}

impl DriverPod {
    /// 用已装配的调度器构造。
    pub fn new(scheduler: Scheduler) -> Self {
        let functions = vec![
            <CommandQueue<'static, DriverCommand> as Capability<'static>>::signatures()[0]
                .qualified(),
            <Broadcast<'static, DriverEvent> as Capability<'static>>::signatures()[0].qualified(),
        ];
        Self {
            id: PodId::next(),
            scheduler: RefCell::new(scheduler),
            frame: Cell::new(0),
            sinks: RefCell::new(Vec::new()),
            commands: RefCell::new(Vec::new()),
            events: RefCell::new(Vec::new()),
            submit: Self::submit_command,
            publish: Self::publish_event,
            functions,
        }
    }

    /// 宿主侧便捷入口：提交通道与协商出的 [`CommandQueue::submit`] **同一条实现**。
    ///
    /// 装配期协商一次后即可长期持有；宿主拿不到 `&'a dyn Pod` 的生命周期时用本方法。
    pub fn submit(&self, command: DriverCommand) -> Receipt {
        Self::submit_command(self, command)
    }

    /// 挂一个事件订阅方（提供 [`EventSink<DriverEvent>`] 的 Pod）。
    pub fn attach_sink(&self, sink: Arc<dyn Pod>) {
        self.sinks.borrow_mut().push(sink);
    }

    /// 已执行的命令序列（确定性比对用：导出路径 == 实时路径）。
    pub fn commands(&self) -> Vec<DriverCommand> {
        self.commands.borrow().clone()
    }

    /// 已发布的事件序列（本机观测；订阅方另有自己的副本）。
    pub fn events(&self) -> Vec<DriverEvent> {
        self.events.borrow().clone()
    }

    /// 已完成帧数。
    pub fn frame(&self) -> u32 {
        self.frame.get()
    }

    /// 只读借用调度器（视图构建 / 可观测性）。
    pub fn with_scheduler<R>(&self, f: impl FnOnce(&Scheduler) -> R) -> R {
        f(&self.scheduler.borrow())
    }

    /// 可变借用调度器（渲染提交 / 帧末清理）。
    ///
    /// 这是宿主侧通道（App 持有具体的 `Arc<DriverPod>`），**不是** Pod 能力：
    /// 场景状态的推进只允许经 [`CommandQueue<DriverCommand>`]。
    pub fn with_scheduler_mut<R>(&self, f: impl FnOnce(&mut Scheduler) -> R) -> R {
        f(&mut self.scheduler.borrow_mut())
    }

    fn submit_command(pod: &dyn Pod, command: DriverCommand) -> Receipt {
        let driver = pod
            .as_any()
            .downcast_ref::<DriverPod>()
            .expect("submit 只由 DriverPod 提供");
        driver.commands.borrow_mut().push(command.clone());
        let receipt = Receipt::pending();
        match command {
            DriverCommand::Advance { dt } => driver.run_frames(1, dt.max(0.0) as f64),
            DriverCommand::Batch { frames, dt } => {
                driver.run_frames(frames, dt.max(0.0) as f64);
            }
            DriverCommand::Seek { t } => driver.seek(t),
            DriverCommand::Reset => driver.reset(),
        }
        receipt.complete();
        receipt
    }

    fn publish_event(pod: &dyn Pod, event: DriverEvent) {
        let driver = pod
            .as_any()
            .downcast_ref::<DriverPod>()
            .expect("publish 只由 DriverPod 提供");
        driver.dispatch_event(event);
    }

    fn run_frames(&self, frames: u32, dt: f64) {
        for _ in 0..frames {
            self.scheduler.borrow_mut().run_frame(dt as f32);
            let frame = self.frame.get() + 1;
            self.frame.set(frame);
            let t = self
                .scheduler
                .borrow()
                .resources()
                .get::<TimeState>()
                .map(|state| state.elapsed)
                .unwrap_or(0.0);
            self.dispatch_event(DriverEvent::Advanced { dt: dt as f32 });
            self.dispatch_event(DriverEvent::FrameFinished { frame, t });
        }
    }

    fn seek(&self, t: f64) {
        {
            let mut scheduler = self.scheduler.borrow_mut();
            let resources = scheduler.resources_mut();
            if let Some(state) = resources.get_mut::<DirectorState>() {
                state.director.seek(t);
            }
            if let Some(state) = resources.get_mut::<TimeState>() {
                state.elapsed = t as f32;
            }
        }
        self.dispatch_event(DriverEvent::Advanced { dt: 0.0 });
    }

    fn reset(&self) {
        {
            let mut scheduler = self.scheduler.borrow_mut();
            let resources = scheduler.resources_mut();
            if let Some(state) = resources.get_mut::<DirectorState>() {
                state.director.seek(0.0);
                state.director.resume();
            }
            if let Some(state) = resources.get_mut::<TimeState>() {
                state.elapsed = 0.0;
            }
        }
        self.frame.set(0);
        self.dispatch_event(DriverEvent::Advanced { dt: 0.0 });
    }

    fn dispatch_event(&self, event: DriverEvent) {
        self.events.borrow_mut().push(event.clone());
        // 先克隆订阅方列表，避免 on_event 回调里再借用 sinks。
        let sinks = self.sinks.borrow().clone();
        for sink in sinks {
            if let Some(sink) = negotiate_dyn::<EventSink<DriverEvent>>(sink.as_ref()) {
                sink.on_event(event.clone());
            }
        }
    }
}

impl Pod for DriverPod {
    fn id(&self) -> PodId {
        self.id
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn functions(&self) -> &[QualifiedName] {
        &self.functions
    }

    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        if *name == self.functions[0] {
            Some(&self.submit)
        } else if *name == self.functions[1] {
            Some(&self.publish)
        } else {
            None
        }
    }

    fn call(&self, id: FunctionId, _args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        Err(PodError::CallFailed(format!(
            "DriverPod 不支持类型擦除调用（{id}）；命令 / 事件必须走 typed capability"
        )))
    }
}

/// `DriverCommand` / `DriverEvent` 的类型名（协商签名的一部分）。
pub fn command_type_name() -> &'static str {
    <DriverCommand as xc_pod::IntoPodTypeName>::NAME
}

/// 事件类型名（协商签名的一部分）。
pub fn event_type_name() -> &'static str {
    <DriverEvent as xc_pod::IntoPodTypeName>::NAME
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_pod::{Broadcast, CommandQueue};
    use xcal_schedule::resource::Resources;

    fn scheduler() -> Scheduler {
        let resources = Resources::new();
        Scheduler::new(hecs::World::new(), resources)
    }

    fn driver_pod() -> Arc<DriverPod> {
        Arc::new(DriverPod::new(scheduler()))
    }

    #[test]
    fn advance_records_frames_and_events() {
        let pod = driver_pod();
        let queue: CommandQueue<'_, DriverCommand> = pod.negotiate().expect("命令队列可协商");
        queue.submit(DriverCommand::Advance { dt: 0.5 }).wait();
        queue
            .submit(DriverCommand::Batch {
                frames: 3,
                dt: 0.25,
            })
            .wait();

        assert_eq!(pod.commands().len(), 2);
        assert_eq!(pod.frame(), 4);
        assert!(matches!(
            pod.events().last(),
            Some(DriverEvent::FrameFinished { frame: 4, .. })
        ));
    }

    #[test]
    fn events_are_dispatched_to_typed_sinks() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use xc_pod::RegistryPod;

        static HITS: AtomicU32 = AtomicU32::new(0);
        fn count_event(_: &dyn Pod, _event: DriverEvent) {
            HITS.fetch_add(1, Ordering::Relaxed);
        }

        let pod = driver_pod();
        HITS.store(0, Ordering::Relaxed);
        let mut sink = RegistryPod::new();
        // 订阅方只需要提供 on_event 的 typed 签名。
        sink.register_fn1::<DriverEvent, ()>("xc_pod::event_sink::on_event", count_event);
        pod.attach_sink(Arc::new(sink));

        let bus: Broadcast<'_, DriverEvent> = pod.negotiate().expect("事件总线可协商");
        bus.publish(DriverEvent::Advanced { dt: 0.1 });
        assert_eq!(HITS.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn seek_and_reset_do_not_advance_frames() {
        let pod = driver_pod();
        let queue: CommandQueue<'_, DriverCommand> = pod.negotiate().expect("命令队列可协商");
        queue.submit(DriverCommand::Seek { t: 2.0 }).wait();
        assert_eq!(pod.frame(), 0);
        queue.submit(DriverCommand::Reset).wait();
        assert_eq!(pod.frame(), 0);
    }

    /// **确定性验收**：同一组命令跑两遍 ⇒ 命令序列与事件序列逐条相同。
    ///
    /// 导出用固定 `dt = 1/fps`；录制命令序列可在**无 GPU 环境**验证
    /// 「导出路径 == 实时路径」的**过程**（像素比对只能验证结果）。
    #[test]
    fn command_and_event_sequences_are_deterministic() {
        fn run() -> (Vec<DriverCommand>, Vec<DriverEvent>) {
            let pod = driver_pod();
            for _ in 0..30 {
                pod.submit(DriverCommand::Advance { dt: 1.0 / 30.0 }).wait();
            }
            (pod.commands(), pod.events())
        }

        let first = run();
        let second = run();
        assert_eq!(first.0, second.0, "命令序列必须逐条相同");
        assert_eq!(first.1, second.1, "事件序列必须逐条相同");
        assert_eq!(first.0.len(), 30);
    }

    #[test]
    fn type_names_are_part_of_signatures() {
        assert_eq!(
            CommandQueue::<DriverCommand>::signatures()[0]
                .qualified()
                .to_string(),
            format!(
                "xc_pod::command::submit({}) -> Receipt",
                command_type_name()
            )
        );
        assert_eq!(
            Broadcast::<DriverEvent>::signatures()[0]
                .qualified()
                .to_string(),
            format!(
                "xc_pod::broadcast::publish({}) -> ()",
                event_type_name()
            )
        );
    }
}
