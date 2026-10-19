// progress.rs — Driver 事件订阅方（A5：从 app 归位到 UI 层）。
//
// 它只服务 UI 可观测性（health overlay / 进度显示）：只记录 `FrameFinished`，
// 不碰命令队列、不推进时间。app 只负责把它挂到 Driver 上（壳子的"事件订阅"职责）。

use std::any::Any;
use std::cell::Cell;

use ze_pod::{Capability, EventSink, FunctionId, Pod, PodError, PodId, QualifiedName};

use zela_scene::DriverEvent;

/// 事件订阅方：提供 `EventSink<DriverEvent>`，只记录 `FrameFinished`。
pub struct FrameProgress {
    id: PodId,
    functions: Vec<QualifiedName>,
    on_event: fn(&dyn Pod, DriverEvent),
    frames: Cell<u32>,
    time: Cell<f32>,
}

impl FrameProgress {
    pub fn new() -> Self {
        Self {
            id: PodId::next(),
            functions: vec![
                <EventSink<'static, DriverEvent> as Capability<'static>>::signatures()[0]
                    .qualified(),
            ],
            on_event: Self::record,
            frames: Cell::new(0),
            time: Cell::new(0.0),
        }
    }

    fn record(pod: &dyn Pod, event: DriverEvent) {
        let this = pod
            .as_any()
            .downcast_ref::<Self>()
            .expect("on_event 只由 FrameProgress 提供");
        if let DriverEvent::FrameFinished { frame, t } = event {
            this.frames.set(frame);
            this.time.set(t);
        }
    }

    /// 已完成帧数。
    pub fn frames(&self) -> u32 {
        self.frames.get()
    }

    /// 帧末全局时间（秒）。
    pub fn time(&self) -> f32 {
        self.time.get()
    }
}

impl Default for FrameProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl Pod for FrameProgress {
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
        (*name == self.functions[0]).then_some(&self.on_event as &(dyn Any + Send + Sync))
    }

    fn call(
        &self,
        id: FunctionId,
        _args: &[&dyn Any],
    ) -> std::result::Result<Box<dyn Any>, PodError> {
        Err(PodError::CallFailed(format!(
            "FrameProgress 不支持类型擦除调用（{id}）"
        )))
    }
}
