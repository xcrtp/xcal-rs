//! Pod 层集成测试与基准的**共用设施**（mini pod，领域无关）。
//!
//! 本 crate 只服务测试与基准：
//! - 同时依赖核心组与两个语言适配器；
//! - **不被任何生产 crate 依赖**（`publish = false`，且没有反向边）；
//! - 不引入任何 `zela-*`。
//!
//! # mini pod（命令 / 事件枚举由它们自己注入泛型参数）
//!
//! | Pod | 提供 | 用途 |
//! |---|---|---|
//! | [`ClockPod`] | `CommandQueue<ClockCmd>` + `Broadcast<ClockEvent>` | 推进逻辑帧、产出事件 |
//! | [`LogPod`] | `EventSink<ClockEvent>` | 记录事件（验证扇出） |
//! | [`StatsPod`] | `EventSink<ClockEvent>` | 计数 / 记录指纹（验证扇出） |
//! | [`ControllerPod`] | 消费 `CommandQueue<ClockCmd>`（经 Injector） | 真实跨 Pod 命令提交 |
//!
//! # 可证伪：命令 / 事件类型分离
//!
//! 事件不能当命令提交：
//!
//! ```compile_fail
//! use ze_pod_core::CommandQueue;
//!
//! #[derive(Clone, ze_pod_macro::PodType)]
//! #[pod_type(name = "demo.Cmd")]
//! struct Cmd { n: u32 }
//!
//! #[derive(Clone, ze_pod_macro::PodType)]
//! #[pod_type(name = "demo.Ev")]
//! struct Ev { n: u32 }
//!
//! fn misuse(queue: CommandQueue<'_, Cmd>, event: Ev) {
//!     // E0308：期望 Cmd，实际 Ev
//!     let _ = queue.submit(event);
//! }
//! ```
//!
//! 命令不能当事件发布：
//!
//! ```compile_fail
//! use ze_pod_core::Broadcast;
//!
//! #[derive(Clone, ze_pod_macro::PodType)]
//! #[pod_type(name = "demo.Cmd")]
//! struct Cmd { n: u32 }
//!
//! #[derive(Clone, ze_pod_macro::PodType)]
//! #[pod_type(name = "demo.Ev")]
//! struct Ev { n: u32 }
//!
//! fn misuse(bus: Broadcast<'_, Ev>, command: Cmd) {
//!     // E0308：期望 Ev，实际 Cmd
//!     bus.publish(command);
//! }
//! ```
#![forbid(unsafe_code)]
#![allow(dead_code)] // capability 的 inherent 方法不全部调用。

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::sync::{Arc, Mutex};

use ze_pod_core::{
    Broadcast, Capability, CommandQueue, EventSink, FunctionId, Handle, Pod, PodError, PodHandle,
    PodId, QualifiedName, Receipt, RegistryPod, SeqKey, Sequenced, ServiceRequirement,
    negotiate_dyn,
};
use ze_pod_macro::capability;

// ─── 命令 / 事件（mini domain）─────────────────────────────────────────

/// 时钟命令（**命令**：参与帧推进）。
#[derive(Clone, Debug, PartialEq, Eq, ze_pod_macro::PodType)]
#[pod_type(name = "mini.ClockCmd")]
pub enum ClockCmd {
    /// 推进 `frames` 个逻辑帧。
    Tick {
        /// 帧数。
        frames: u32,
    },
    /// 空操作（只计一次命令派发）。
    Noop,
}

/// 时钟事件（**事件**：只用于观测）。
#[derive(Clone, Debug, PartialEq, Eq, ze_pod_macro::PodType)]
#[pod_type(name = "mini.ClockEvent")]
pub enum ClockEvent {
    /// 一帧完成（`frame` / `t` 都是逻辑量，不是墙钟）。
    Ticked {
        /// 逻辑帧号（从 1 开始）。
        frame: u64,
        /// 逻辑时间（本 mini pod 的 `t` 每帧 +1）。
        t: u64,
    },
}

// ─── ClockPod ──────────────────────────────────────────────────────────

/// 时钟 Pod：`CommandQueue<ClockCmd>` + `Broadcast<ClockEvent>`。
///
/// 同步执行（`submit` 内完成推进）；事件带逻辑键 `(frame, source, seq)`。
pub struct ClockPod {
    id: PodId,
    source: PodHandle,
    frame: Cell<u64>,
    t: Cell<u64>,
    seq: Cell<u64>,
    log: RefCell<Vec<Sequenced<ClockEvent>>>,
    published: RefCell<Vec<ClockEvent>>,
    functions: Vec<QualifiedName>,
    submit: fn(&dyn Pod, ClockCmd) -> Receipt,
    publish: fn(&dyn Pod, ClockEvent),
}

impl ClockPod {
    /// 自动分配 PodId。
    pub fn new() -> Self {
        Self::with_id(PodId::next())
    }

    /// 用**固定 PodId** 构造（确定性断言用：同源才能得到同键）。
    pub fn with_id(id: PodId) -> Self {
        let source = PodHandle::new(id);
        let functions = vec![
            <CommandQueue<'static, ClockCmd> as Capability<'static>>::signatures()[0]
                .qualified(),
            <Broadcast<'static, ClockEvent> as Capability<'static>>::signatures()[0].qualified(),
        ];
        Self {
            id,
            source,
            frame: Cell::new(0),
            t: Cell::new(0),
            seq: Cell::new(0),
            log: RefCell::new(Vec::new()),
            published: RefCell::new(Vec::new()),
            functions,
            submit: Self::submit_command,
            publish: Self::publish_event,
        }
    }

    /// 寻址句柄（排序键的 `source`）。
    pub fn source(&self) -> PodHandle {
        self.source
    }

    /// 当前逻辑帧号。
    pub fn frame(&self) -> u64 {
        self.frame.get()
    }

    /// 已产出的有序事件（**逻辑键 + 值**）。
    pub fn log(&self) -> Vec<Sequenced<ClockEvent>> {
        self.log.borrow().clone()
    }

    /// 已产出的事件值（不含键）。
    pub fn events(&self) -> Vec<ClockEvent> {
        self.log
            .borrow()
            .iter()
            .map(|item| item.value().clone())
            .collect()
    }

    /// 经兼容入口 `Broadcast<ClockEvent>::publish` 收到的值（仅用于对照）。
    pub fn published(&self) -> Vec<ClockEvent> {
        self.published.borrow().clone()
    }

    fn submit_command(pod: &dyn Pod, command: ClockCmd) -> Receipt {
        let this = pod
            .as_any()
            .downcast_ref::<Self>()
            .expect("submit 只由 ClockPod 提供");
        match command {
            ClockCmd::Tick { frames } => {
                for _ in 0..frames {
                    let frame = this.frame.get() + 1;
                    this.frame.set(frame);
                    let t = this.t.get() + 1;
                    this.t.set(t);
                    let key = SeqKey::new(frame, this.source, this.seq.get());
                    this.seq.set(this.seq.get() + 1);
                    this.log
                        .borrow_mut()
                        .push(Sequenced::new(key, ClockEvent::Ticked { frame, t }));
                }
            }
            ClockCmd::Noop => {}
        }
        Receipt::completed()
    }

    fn publish_event(pod: &dyn Pod, event: ClockEvent) {
        pod.as_any()
            .downcast_ref::<Self>()
            .expect("publish 只由 ClockPod 提供")
            .published
            .borrow_mut()
            .push(event);
    }
}

impl Default for ClockPod {
    fn default() -> Self {
        Self::new()
    }
}

impl Pod for ClockPod {
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
            "ClockPod 不支持类型擦除调用（{id}）；命令 / 事件走 typed capability"
        )))
    }
}

// ─── LogPod / StatsPod（EventSink）────────────────────────────────────

fn event_sink_name() -> QualifiedName {
    <EventSink<'static, ClockEvent> as Capability<'static>>::signatures()[0].qualified()
}

/// 记录事件（验证扇出：收到什么、按什么顺序）。
pub struct LogPod {
    id: PodId,
    registry: RegistryPod,
    entries: RefCell<Vec<ClockEvent>>,
}

impl LogPod {
    /// 自动分配 PodId。
    pub fn new() -> Self {
        Self::with_id(PodId::next())
    }

    /// 用固定 PodId 构造（扇出顺序断言用）。
    pub fn with_id(id: PodId) -> Self {
        let mut registry = RegistryPod::new();
        registry.register_fn1::<ClockEvent, ()>("ze_pod_core::event_sink::on_event", log_event);
        debug_assert!(registry.functions().contains(&event_sink_name()));
        Self {
            id,
            registry,
            entries: RefCell::new(Vec::new()),
        }
    }

    /// 已收到的事件（到达顺序）。
    pub fn entries(&self) -> Vec<ClockEvent> {
        self.entries.borrow().clone()
    }
}

impl Default for LogPod {
    fn default() -> Self {
        Self::new()
    }
}

fn log_event(pod: &dyn Pod, event: ClockEvent) {
    pod.as_any()
        .downcast_ref::<LogPod>()
        .expect("log_event 只由 LogPod 提供")
        .entries
        .borrow_mut()
        .push(event);
}

impl Pod for LogPod {
    fn id(&self) -> PodId {
        self.id
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn functions(&self) -> &[QualifiedName] {
        self.registry.functions()
    }
    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.registry.resolve(name)
    }
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        self.registry.call(id, args)
    }
}

/// 计数 + 记录逻辑时间指纹（验证两个订阅方收到同一序列）。
pub struct StatsPod {
    id: PodId,
    registry: RegistryPod,
    count: Cell<u32>,
    ticks: RefCell<Vec<u64>>,
}

impl StatsPod {
    /// 自动分配 PodId。
    pub fn new() -> Self {
        Self::with_id(PodId::next())
    }

    /// 用固定 PodId 构造。
    pub fn with_id(id: PodId) -> Self {
        let mut registry = RegistryPod::new();
        registry.register_fn1::<ClockEvent, ()>("ze_pod_core::event_sink::on_event", stat_event);
        Self {
            id,
            registry,
            count: Cell::new(0),
            ticks: RefCell::new(Vec::new()),
        }
    }

    /// 收到的事件数。
    pub fn count(&self) -> u32 {
        self.count.get()
    }

    /// 收到的逻辑时间序列。
    pub fn ticks(&self) -> Vec<u64> {
        self.ticks.borrow().clone()
    }
}

impl Default for StatsPod {
    fn default() -> Self {
        Self::new()
    }
}

fn stat_event(pod: &dyn Pod, event: ClockEvent) {
    let this = pod
        .as_any()
        .downcast_ref::<StatsPod>()
        .expect("stat_event 只由 StatsPod 提供");
    this.count.set(this.count.get() + 1);
    let ClockEvent::Ticked { t, .. } = event;
    this.ticks.borrow_mut().push(t);
}

impl Pod for StatsPod {
    fn id(&self) -> PodId {
        self.id
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn functions(&self) -> &[QualifiedName] {
        self.registry.functions()
    }
    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.registry.resolve(name)
    }
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        self.registry.call(id, args)
    }
}

// ─── ControllerPod（消费 CommandQueue<ClockCmd>）──────────────────────

/// 控制器 Pod：经 Injector 拿到 clock 服务，提交 `Tick` 命令。
pub struct ControllerPod {
    id: PodId,
    registry: RegistryPod,
    clock: RefCell<Option<Arc<dyn Pod>>>,
}

impl ControllerPod {
    /// 新建（需求：`clock` 服务提供 `CommandQueue<ClockCmd>`）。
    pub fn new() -> Self {
        let id = PodId::next();
        let mut registry = RegistryPod::new();
        registry.register_fn0_named::<Vec<ServiceRequirement>>(
            "ze_pod_core::injector::requirements",
            "ServiceRequirements",
            controller_requirements,
        );
        registry.register_fn2_named::<String, Arc<dyn Pod>, Result<(), PodError>>(
            "ze_pod_core::injector::inject",
            "ServiceName",
            "ServicePod",
            "InjectResult",
            controller_inject,
        );
        Self {
            id,
            registry,
            clock: RefCell::new(None),
        }
    }

    /// 已注入的 Service 名。
    pub fn injected(&self) -> Vec<String> {
        self.clock
            .borrow()
            .as_ref()
            .map(|_| vec!["clock".to_string()])
            .unwrap_or_default()
    }

    /// 经注入的 clock 提交 `Tick`（**真实的跨 Pod 泛型 capability 使用**）。
    pub fn tick(&self, frames: u32) -> Result<Receipt, PodError> {
        let clock = self.clock.borrow().clone().ok_or_else(|| {
            PodError::CallFailed("ControllerPod 尚未注入 clock 服务".to_string())
        })?;
        let queue: CommandQueue<'_, ClockCmd> = negotiate_dyn(&*clock)
            .ok_or_else(|| PodError::CallFailed("clock 未提供 CommandQueue<ClockCmd>".to_string()))?;
        Ok(queue.submit(ClockCmd::Tick { frames }))
    }
}

impl Default for ControllerPod {
    fn default() -> Self {
        Self::new()
    }
}

fn controller_requirements(_: &dyn Pod) -> Vec<ServiceRequirement> {
    vec![ServiceRequirement::required(
        "clock",
        &CommandQueue::<ClockCmd>::signatures()[0].qualified().to_string(),
    )]
}

fn controller_inject(
    pod: &dyn Pod,
    service: String,
    service_pod: Arc<dyn Pod>,
) -> Result<(), PodError> {
    let this = pod
        .as_any()
        .downcast_ref::<ControllerPod>()
        .expect("controller_inject 只由 ControllerPod 提供");
    if service == "clock" {
        *this.clock.borrow_mut() = Some(service_pod);
    }
    Ok(())
}

impl Pod for ControllerPod {
    fn id(&self) -> PodId {
        self.id
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn functions(&self) -> &[QualifiedName] {
        self.registry.functions()
    }
    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.registry.resolve(name)
    }
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        self.registry.call(id, args)
    }
}

// ─── 语言绑定用例（Lua / Python 共用同一套声明）──────────────────────

/// 绑定测试用的点。
#[derive(Clone, Debug, PartialEq, ze_pod_macro::PodType)]
#[pod_type(name = "mini.Dot")]
pub struct Dot {
    /// x。
    pub x: f32,
    /// y。
    pub y: f32,
    /// 半径。
    pub r: f32,
}

/// 绑定测试用的枚举（unit 变体 + 具名字段变体）。
#[derive(Clone, Debug, PartialEq, ze_pod_macro::PodType)]
#[pod_type(name = "mini.Kind")]
pub enum Kind {
    /// 常量。
    Const,
    /// 带权重。
    Weight {
        /// 权重值。
        value: f32,
    },
}

/// Lua / Python 两侧**共用**的能力声明。
#[capability(namespace = "mini", description = "语言绑定共用用例", bindings)]
pub struct MiniApi<'a> {
    /// 创建一个点组件。
    pub dot: fn(&'a dyn Pod, shape: Dot) -> Handle,
    /// 累加一个数值。
    pub emit: fn(&'a dyn Pod, value: f32) -> (),
    /// 读累计值。
    pub total: fn(&'a dyn Pod) -> f32,
    /// 记录一个枚举值。
    pub kind: fn(&'a dyn Pod, kind: Kind) -> (),
}

/// 绑定用例的录制结果（两种语言必须逐字段相等）。
#[derive(Debug, Default, PartialEq)]
pub struct MiniOutput {
    /// 建了几个点。
    pub dots: usize,
    /// 数值序列。
    pub values: Vec<f32>,
    /// 枚举标签序列。
    pub kinds: Vec<String>,
}

#[derive(Default)]
struct Recorded {
    dots: usize,
    values: Vec<f32>,
    kinds: Vec<String>,
}

/// 宿主侧 mini Service（Lua / Python 共用；不依赖任何 domain crate）。
///
/// `Recorded` 用 `Mutex`（不是 `RefCell`）⇒ 本类型可登记 `SyncPod` 徽章，
/// Python 闭包能直接捕获它的 `Arc<dyn SyncPod>`。
pub struct MiniRecorderPod {
    id: PodId,
    registry: RegistryPod,
    rec: Mutex<Recorded>,
}

impl MiniRecorderPod {
    /// 新建。
    pub fn new() -> Self {
        let id = PodId::next();
        let mut registry = RegistryPod::new();
        registry.register_fn1_named::<Dot, Handle>(
            "mini::dot",
            "mini.Dot",
            "ze_pod_core.Handle",
            mini_dot,
        );
        registry.register_fn1::<f32, ()>("mini::emit", mini_emit);
        registry.register_fn0::<f32>("mini::total", mini_total);
        registry.register_fn1::<Kind, ()>("mini::kind", mini_kind);
        registry.register_erased(ze_pod_lua::RESET_PATH, &[], "()", |pod, _| {
            *mini(pod).rec.lock().expect("录制表锁中毒") = Recorded::default();
            Ok(Box::new(()))
        });
        registry.register_erased(
            ze_pod_lua::FINISH_PATH,
            &[],
            "Result<Box<dyn Any>, String>",
            |pod, _| Ok(Box::new(mini(pod).finish().map(|out| Box::new(out) as Box<dyn Any>))),
        );
        Self {
            id,
            registry,
            rec: Mutex::new(Recorded::default()),
        }
    }

    fn finish(&self) -> Result<MiniOutput, String> {
        let rec = self.rec.lock().expect("录制表锁中毒");
        Ok(MiniOutput {
            dots: rec.dots,
            values: rec.values.clone(),
            kinds: rec.kinds.clone(),
        })
    }
}

impl Default for MiniRecorderPod {
    fn default() -> Self {
        Self::new()
    }
}

fn mini(pod: &dyn Pod) -> &MiniRecorderPod {
    pod.as_any()
        .downcast_ref::<MiniRecorderPod>()
        .expect("MiniRecorderPod")
}

fn mini_dot(pod: &dyn Pod, shape: Dot) -> Handle {
    let this = mini(pod);
    this.rec.lock().expect("录制表锁中毒").dots += 1;
    Handle::new(this.id, (shape.x + shape.y + shape.r) as u64, 1)
}

fn mini_emit(pod: &dyn Pod, value: f32) {
    mini(pod)
        .rec
        .lock()
        .expect("录制表锁中毒")
        .values
        .push(value);
}

fn mini_total(pod: &dyn Pod) -> f32 {
    mini(pod)
        .rec
        .lock()
        .expect("录制表锁中毒")
        .values
        .iter()
        .sum()
}

fn mini_kind(pod: &dyn Pod, kind: Kind) {
    let label = match kind {
        Kind::Const => "Const".to_string(),
        Kind::Weight { value } => format!("Weight({value})"),
    };
    mini(pod)
        .rec
        .lock()
        .expect("录制表锁中毒")
        .kinds
        .push(label);
}

impl Pod for MiniRecorderPod {
    fn id(&self) -> PodId {
        self.id
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn functions(&self) -> &[QualifiedName] {
        self.registry.functions()
    }
    fn resolve(&self, name: &QualifiedName) -> Option<&(dyn Any + Send + Sync)> {
        self.registry.resolve(name)
    }
    fn call(&self, id: FunctionId, args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        let name = self
            .functions()
            .get(id.get() as usize)
            .ok_or_else(|| PodError::UnknownFunction(format!("函数编号 {id}")))?;
        match name.name() {
            "__reset" => {
                *self.rec.lock().expect("录制表锁中毒") = Recorded::default();
                Ok(Box::new(()))
            }
            "__finish" => Ok(Box::new(
                self.finish().map(|out| Box::new(out) as Box<dyn Any>),
            )),
            _ => self.registry.call(id, args),
        }
    }
}

// ─── 基准专用：无分配的命令 Pod 与 typed / erased 对照 ────────────────

/// 基准命令（无事件产出 ⇒ 只测命令派发 / 推进本身）。
#[derive(Clone, Debug, PartialEq, Eq, ze_pod_macro::PodType)]
#[pod_type(name = "bench.CounterCmd")]
pub enum CounterCmd {
    /// 推进 N（只改计数，不分配）。
    Tick {
        /// 帧数。
        frames: u32,
    },
    /// 空操作。
    Noop,
}

/// 基准用的计数器 Pod（`Tick` 只做 `frame += n`，无 Vec 分配）。
pub struct CounterPod {
    id: PodId,
    frame: Cell<u64>,
    functions: Vec<QualifiedName>,
    submit: fn(&dyn Pod, CounterCmd) -> Receipt,
}

impl CounterPod {
    /// 新建。
    pub fn new() -> Self {
        Self {
            id: PodId::next(),
            frame: Cell::new(0),
            functions: vec![
                <CommandQueue<'static, CounterCmd> as Capability<'static>>::signatures()[0]
                    .qualified(),
            ],
            submit: counter_submit,
        }
    }

    /// 当前计数。
    pub fn frame(&self) -> u64 {
        self.frame.get()
    }
}

impl Default for CounterPod {
    fn default() -> Self {
        Self::new()
    }
}

fn counter_submit(pod: &dyn Pod, command: CounterCmd) -> Receipt {
    let this = pod
        .as_any()
        .downcast_ref::<CounterPod>()
        .expect("CounterPod");
    if let CounterCmd::Tick { frames } = command {
        // 逐帧推进（模拟真实 driver 的每帧工作），不折叠成一次加法 ——
        // 基准要能区分「推进成本」与「命令派发成本」。
        for _ in 0..frames {
            this.frame.set(this.frame.get() + 1);
        }
    }
    Receipt::completed()
}

impl Pod for CounterPod {
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
        (*name == self.functions[0]).then_some(&self.submit as &(dyn Any + Send + Sync))
    }
    fn call(&self, id: FunctionId, _args: &[&dyn Any]) -> Result<Box<dyn Any>, PodError> {
        Err(PodError::CallFailed(format!(
            "CounterPod 不支持类型擦除调用（{id}）"
        )))
    }
}

/// typed 快路径 vs erased 慢路径的对照能力。
#[capability(namespace = "bench", description = "typed / erased 对照")]
pub struct MathApi<'a> {
    /// `x * 2`。
    pub double: fn(&'a dyn Pod, f32) -> f32,
}

/// 构造同时具备 typed / erased 两路径的注册表 Pod。
pub fn math_registry() -> RegistryPod {
    let mut pod = RegistryPod::new();
    pod.register_fn1::<f32, f32>("bench::double", double_fn);
    pod
}

fn double_fn(_: &dyn Pod, x: f32) -> f32 {
    x * 2.0
}
