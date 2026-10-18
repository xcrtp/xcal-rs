//! 命令队列 / 事件总线：**泛型 capability**（命令枚举与事件枚举由实现方注入）。
//!
//! # 为什么泛型
//!
//! 命令与事件必须**类型分离**：
//!
//! | 能力 | 方法 | 语义 |
//! |---|---|---|
//! | [`CommandQueue<C>`] | `submit(&dyn Pod, C) -> Receipt` | 控制：FIFO、顺序确定、参与帧推进 |
//! | [`Broadcast<E>`] | `publish(&dyn Pod, E)` | 通知：异步、可丢、不参与帧推进 |
//! | [`EventSink<E>`] | `on_event(&dyn Pod, E)` | 订阅方实现 |
//!
//! 若退化成 `PodValue` 承载，`submit` 与 `publish` 的签名会**完全相同**，
//! 把事件当命令提交时编译器不会报错 —— §3.2 的硬约束就退化为纯约定。
//! 泛型参数让两者成为不同类型：`submit(C)` 与 `publish(E)` 的类型系统保证
//! **事件永远不可能被当命令提交**。
//!
//! # 回执（[`Receipt`]）
//!
//! `submit` 返回可等待回执：导出循环用 [`Receipt::wait`] 同步等一帧完成，
//! 不靠监听 `Finished` 事件推进；异步订阅方仍走事件总线观测。
//! 同步与异步因此走**同一条路径**，不分叉。
//!
//! # 主线程禁令（`Receipt::wait`）
//!
//! [`Receipt::wait`] 是**自旋让出时间片**的忙等，本身不驱动任何队列。
//!
//! - **主线程不得调用 `wait()`**：若回执要等的那条命令正由主线程的队列
//!   `pump` 推进（GUI / headless 的帧循环都属于这种），主线程在这里让出
//!   时间片 ⇒ **没有任何线程会再推进队列** ⇒ 死锁。
//! - **主线程必须等待时，用 [`Receipt::wait_pumping`]**：每轮循环调用一次
//!   传入的 `pump`，让队列继续推进；等待期间队列不会停摆。
//! - **`wait()` 的合法场景**：工作线程等待另一个线程驱动的队列；
//!   或导出二进制在**同步驱动**下等待（命令已在 `submit` 内完成，立即返回）。
//!
//! 运行期守护：debug 构建下，对待完成回执调用 `wait()` 时，若当前线程正是
//! 提交该回执的线程（最常见的就是主线程），会触发 `debug_assert`。
//! 命名区分：`wait`（纯自旋，不得在主线程用）/ `wait_pumping`（主线程可用）。
//!
//! # 不跨边界传闭包
//!
//! 事件发布由宿主（或 domain pod）按 broker 注册表找到提供 `EventSink<E>` 的
//! Pod，逐个 **typed** 调用其 `on_event`。全程无 `Arc<dyn Fn>` 过 Pod 边界、
//! 无 `unsafe`。
//!
//! # 可证伪：事件永远不能当命令提交
//!
//! ```compile_fail
//! use xc_pod::CommandQueue;
//!
//! #[derive(Clone, xc_pod_macro::PodType)]
//! struct Advance { dt: f32 }
//!
//! #[derive(Clone, xc_pod_macro::PodType)]
//! struct FrameFinished { frame: u32 }
//!
//! fn misuse(queue: CommandQueue<'_, Advance>, event: FrameFinished) {
//!     // E0308：期望 `Advance`，实际 `FrameFinished` ⇒ 编译失败。
//!     let _ = queue.submit(event);
//! }
//! ```
#![forbid(unsafe_code)]

use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::ThreadId;

use crate::capability::{Capability, CapabilityMeta, FnSig, Resolved};
use crate::pod::Pod;
use crate::type_name::IntoPodTypeName;
use crate::value::PodType;

/// `CommandQueue` 占用的保留 namespace。
pub const COMMAND_NAMESPACE: &str = "xc_pod::command";
/// `Broadcast` 占用的保留 namespace。
pub const BROADCAST_NAMESPACE: &str = "xc_pod::broadcast";
/// `EventSink` 占用的保留 namespace。
pub const EVENT_SINK_NAMESPACE: &str = "xc_pod::event_sink";

/// 命令回执：`submit` 返回，可同步等待该命令（含其推进的一帧）完成。
///
/// 驱动方在同一次 `submit` 调用内完成命令即置位；`wait()` 因此立即返回。
/// 异步驱动方可以保留回执，稍后再等。
///
/// # 零分配快路径
///
/// 同步驱动的常见路径用 [`Receipt::completed`]：**不分配**（只是一个枚举标签）。
/// 只有真正需要"稍后完成"的异步驱动才用 [`Receipt::pending`]（共享标志）。
#[derive(Clone, Debug)]
pub struct Receipt {
    state: ReceiptState,
}

#[derive(Clone, Debug)]
enum ReceiptState {
    /// 已完成（无分配快路径）。
    Done,
    /// 待完成（只有异步驱动才分配）。
    Pending(Arc<PendingState>),
}

#[derive(Debug)]
struct PendingState {
    done: AtomicBool,
    /// 提交该回执的线程（`wait()` 的主线程禁令用）。
    submitter: ThreadId,
}

impl Default for Receipt {
    fn default() -> Self {
        Self::completed()
    }
}

impl Receipt {
    /// 尚未完成的回执（**会分配**：共享完成标志 + 提交线程标识）。
    pub fn pending() -> Self {
        Self {
            state: ReceiptState::Pending(Arc::new(PendingState {
                done: AtomicBool::new(false),
                submitter: std::thread::current().id(),
            })),
        }
    }

    /// 已完成（构造即完成；**零分配**）。
    pub fn completed() -> Self {
        Self {
            state: ReceiptState::Done,
        }
    }

    /// 标记完成（驱动方调用；幂等；对已完成回执是 no-op）。
    pub fn complete(&self) {
        if let ReceiptState::Pending(state) = &self.state {
            state.done.store(true, Ordering::Release);
        }
    }

    /// 是否已完成。
    pub fn is_done(&self) -> bool {
        match &self.state {
            ReceiptState::Done => true,
            ReceiptState::Pending(state) => state.done.load(Ordering::Acquire),
        }
    }

    /// 当前线程是否就是提交该回执的线程（`Done` 回执恒为 `false`）。
    fn submitted_here(&self) -> bool {
        match &self.state {
            ReceiptState::Done => false,
            ReceiptState::Pending(state) => state.submitter == std::thread::current().id(),
        }
    }

    /// 同步等待完成（**纯自旋让出时间片，不驱动队列**）。
    ///
    /// # 主线程禁令
    ///
    /// **主线程不得调用本方法**：若回执要等的那条命令正由主线程的队列推进，
    /// 主线程在此让出时间片 ⇒ 队列停摆 ⇒ **死锁**。
    /// 主线程必须等待时改用 [`Receipt::wait_pumping`]（每轮 pump 一次队列）。
    ///
    /// 合法场景：工作线程等待另一个线程驱动的队列；或同步驱动下命令已在
    /// `submit` 内完成（本方法立即返回）。
    ///
    /// debug 构建下做了运行期守护：对待完成回执在**提交线程**上调用会触发
    /// `debug_assert`（最常见的提交线程就是主线程）。
    pub fn wait(&self) {
        debug_assert!(
            self.is_done() || !self.submitted_here(),
            "Receipt::wait() 在提交线程（通常是主线程）上等待未完成回执 ⇒ 队列无人推进 ⇒ 死锁。\
             主线程必须等待时改用 Receipt::wait_pumping(|| queue.pump())，\
             或把等待移到工作线程。"
        );
        while !self.is_done() {
            std::thread::yield_now();
        }
    }

    /// **主线程可用**的同步等待：每轮循环先执行一次 `pump`（驱动队列），再让出时间片。
    ///
    /// 适用：GUI / headless 帧循环在主线程上等待异步驱动的回执。
    /// `pump` 应尽量无阻塞（例如把队列中已到达的命令全部执行完）。
    pub fn wait_pumping<F: FnMut()>(&self, mut pump: F) {
        while !self.is_done() {
            pump();
            std::thread::yield_now();
        }
    }
}

type SubmitFn<C> = fn(&dyn Pod, C) -> Receipt;

/// 命令队列（泛型 capability）。
///
/// `C` 是**命令枚举**，需要 `#[derive(PodType)]`（提供 `SPEC` / 类型名）。
pub struct CommandQueue<'a, C>
where
    C: PodType + IntoPodTypeName,
{
    pod: &'a dyn Pod,
    submit: SubmitFn<C>,
}

impl<'a, C> CommandQueue<'a, C>
where
    C: PodType + IntoPodTypeName,
{
    /// 提交一条命令，返回可等待回执。
    pub fn submit(&self, command: C) -> Receipt {
        (self.submit)(self.pod, command)
    }

    /// 句柄持有的目标 Pod。
    pub fn pod(&self) -> &'a dyn Pod {
        self.pod
    }

    /// 签名清单（类型名取自 `C` 的 `PodType` 定义）。
    #[doc(hidden)]
    const SIGS: &'static [FnSig] = &[FnSig::new(
        COMMAND_NAMESPACE,
        "submit",
        &[<C as IntoPodTypeName>::NAME],
        "Receipt",
    )];
}

impl<'a, C> Capability<'a> for CommandQueue<'a, C>
where
    C: PodType + IntoPodTypeName,
{
    fn signatures() -> &'static [FnSig] {
        Self::SIGS
    }

    fn meta() -> CapabilityMeta {
        CapabilityMeta::new(Some("1"), Some("命令队列（FIFO，参与帧推进）"))
    }

    fn from_negotiated(pod: &'a dyn Pod, resolved: &[Resolved<'_>]) -> Option<Self> {
        Some(Self {
            pod,
            submit: *resolved
                .first()?
                .callable
                .downcast_ref::<SubmitFn<C>>()?,
        })
    }
}

type PublishFn<E> = fn(&dyn Pod, E);

/// 事件总线（泛型 capability）：只用于**观测**，不参与帧推进。
///
/// # 与 topic 机制的关系
///
/// `Broadcast<E>` 是**兼容入口**：它的 `publish` 只调用提供方自己的实现，
/// **本身不做扇出**。新代码的「广播 = 查询 + 迭代单播」见
/// [`crate::publish_topic`]（成员来自 `Broker::topic_members`，按 `PodId` 排序）。
pub struct Broadcast<'a, E>
where
    E: PodType + IntoPodTypeName,
{
    pod: &'a dyn Pod,
    publish: PublishFn<E>,
}

impl<'a, E> Broadcast<'a, E>
where
    E: PodType + IntoPodTypeName,
{
    /// 发布一条事件（可丢；不改变场景状态）。
    pub fn publish(&self, event: E) {
        (self.publish)(self.pod, event);
    }

    /// 句柄持有的目标 Pod。
    pub fn pod(&self) -> &'a dyn Pod {
        self.pod
    }

    #[doc(hidden)]
    const SIGS: &'static [FnSig] = &[FnSig::new(
        BROADCAST_NAMESPACE,
        "publish",
        &[<E as IntoPodTypeName>::NAME],
        "()",
    )];
}

impl<'a, E> Capability<'a> for Broadcast<'a, E>
where
    E: PodType + IntoPodTypeName,
{
    fn signatures() -> &'static [FnSig] {
        Self::SIGS
    }

    fn meta() -> CapabilityMeta {
        CapabilityMeta::new(Some("1"), Some("事件总线（异步、可丢、不参与帧推进）"))
    }

    fn from_negotiated(pod: &'a dyn Pod, resolved: &[Resolved<'_>]) -> Option<Self> {
        Some(Self {
            pod,
            publish: *resolved.first()?.callable.downcast_ref::<PublishFn<E>>()?,
        })
    }
}

type OnEventFn<E> = fn(&dyn Pod, E);

/// 事件订阅方（泛型 capability）：由观察者 Pod 提供。
pub struct EventSink<'a, E>
where
    E: PodType + IntoPodTypeName,
{
    pod: &'a dyn Pod,
    on_event: OnEventFn<E>,
    _marker: PhantomData<fn() -> E>,
}

impl<'a, E> EventSink<'a, E>
where
    E: PodType + IntoPodTypeName,
{
    /// 接收一条事件（由发布方逐个 typed 调用，不经过闭包）。
    pub fn on_event(&self, event: E) {
        (self.on_event)(self.pod, event);
    }

    /// 句柄持有的目标 Pod。
    pub fn pod(&self) -> &'a dyn Pod {
        self.pod
    }

    #[doc(hidden)]
    const SIGS: &'static [FnSig] = &[FnSig::new(
        EVENT_SINK_NAMESPACE,
        "on_event",
        &[<E as IntoPodTypeName>::NAME],
        "()",
    )];
}

impl<'a, E> Capability<'a> for EventSink<'a, E>
where
    E: PodType + IntoPodTypeName,
{
    fn signatures() -> &'static [FnSig] {
        Self::SIGS
    }

    fn meta() -> CapabilityMeta {
        CapabilityMeta::new(Some("1"), Some("事件订阅方（只做观测）"))
    }

    fn from_negotiated(pod: &'a dyn Pod, resolved: &[Resolved<'_>]) -> Option<Self> {
        Some(Self {
            pod,
            on_event: *resolved.first()?.callable.downcast_ref::<OnEventFn<E>>()?,
            _marker: PhantomData,
        })
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;

    /// 已完成回执：`wait()` 立即返回（同步驱动的零分配快路径）。
    #[test]
    fn wait_on_completed_receipt_returns_immediately() {
        Receipt::completed().wait();
    }

    /// 主线程等待异步回执：`wait_pumping` 每轮推进队列，直到完成。
    #[test]
    fn wait_pumping_drives_the_queue_to_completion() {
        let receipt = Receipt::pending();
        let mut pumps = 0;
        receipt.wait_pumping(|| {
            pumps += 1;
            if pumps == 3 {
                receipt.complete();
            }
        });
        assert!(receipt.is_done());
        assert_eq!(pumps, 3, "每轮必须 pump 一次，且完成即停止");
    }

    /// 在提交线程上等待未完成回执 ⇒ debug 构建触发守护（主线程死锁的形态）。
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "主线程")]
    fn wait_on_pending_receipt_from_submitter_panics_in_debug() {
        let receipt = Receipt::pending();
        receipt.wait();
    }
}
