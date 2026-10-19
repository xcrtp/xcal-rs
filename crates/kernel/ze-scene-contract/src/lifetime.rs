// lifetime.rs — 生命周期追踪的 **opt-in 标记**（S1）。
//
// 零大小标记组件：只有挂上它的实体才产生 Spawn / Despawn 生命周期事件。
// 未打标记的实体**不产生事件、不产生开销**（默认路径零行为变化）。
//
// 为什么是标记而不是全局开关：场景里绝大多数对象（如 `Axes2D` 每帧重建的
// 39 条线段）不需要生命周期语义；标记把"要不要事件"变成对象自身的声明，
// 而不是全局配置。

/// 生命周期追踪标记（零大小）。
///
/// 挂在需要 Spawn / Despawn 事件的实体上；Group 只给容器自身打标记即可
/// （子实体不产生事件）。
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct LifetimeTracked;
