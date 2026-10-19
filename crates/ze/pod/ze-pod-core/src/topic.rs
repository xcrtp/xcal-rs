//! topic 机制：**广播降级为「查询 + 迭代单播」**。
//!
//! # 形态
//!
//! ```text
//! 发布方 ──Broker::topic_members(topic)──► [PodHandle]（按 PodId 升序）
//!             │
//!             └─ 逐个 negotiate_on_handle::<EventSink<E>>(handle)
//!                    └─ typed 调用 on_event(event.clone())
//! ```
//!
//! - [`EventSink<E>`] 保持为**单播接收端**（每个订阅方一个 Pod）；
//! - 本模块的 [`publish`] 是**普通辅助函数**：不是 capability、不是 pod；
//! - 扇出顺序 = [`Broker::topic_members`] 的返回顺序（按 `PodId` 升序），
//!   **发布方不得硬编码订阅者句柄列表**（否则解耦退化）。
//!
//! # 与 `Broadcast<E>` 的关系
//!
//! `Broadcast<E>` 是既有的 typed 兼容入口：它的 `publish` 只调用
//! **提供方自己的实现**，本身不做扇出。P4 之后 `zela-scene` 的 Driver pod
//! 已改走本模块（成员来自 Broker 查询），`Broadcast` 只保留给尚未迁移的调用方。
//!
//! 新代码应使用本模块：成员来自 Broker 查询，而不是发布方持有的 `Arc<dyn Pod>` 列表。

use crate::broker::{Broker, BrokerError};
use crate::command::EventSink;
use crate::handle::PodHandle;
use crate::type_name::IntoPodTypeName;
use crate::value::PodType;

/// 一次扇出的结果（可审计：投递数 + 实际顺序）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishReport {
    /// 实际投递的订阅方数量。
    pub delivered: usize,
    /// 扇出顺序（**按 `PodId` 升序**，与 `topic_members` 一致）。
    pub order: Vec<PodHandle>,
}

/// 向 topic 的**全部成员**逐个 typed 单播 `event`。
///
/// 成员来自 [`Broker::topic_members`]；每个成员**必须**提供
/// [`EventSink<E>`]，否则返回明确的 [`BrokerError::UnsupportedCapability`]
/// （不静默跳过：声明了 topic 却不实现接收端是装配错误）。
///
/// 订阅方按 `PodId` 升序接收 —— 与 Broker 的节点声明顺序无关。
pub fn publish<E>(broker: &Broker, topic: &str, event: &E) -> Result<PublishReport, BrokerError>
where
    E: PodType + IntoPodTypeName + Clone,
{
    let order = broker.topic_members(topic);
    for &handle in &order {
        let sink: EventSink<'_, E> = broker.negotiate_on_handle(handle)?;
        sink.on_event(event.clone());
    }
    Ok(PublishReport {
        delivered: order.len(),
        order,
    })
}
