// animation 模块：绑定层（Binding）把时钟树和动画曲线显式连接到 hecs 组件。
//
// 设计原则：
// 1. 动画是纯函数 f(t) → 值，永不累积状态（由 xc-time::Curve::sample 保证）。
// 2. Binding 表取代旧的组件组合查询（query::<(&LocalClock, &CircleAnim, &mut Circle)>()，
//    旧动画组件已删除）。
// 3. BindingApplySystem 是唯一的动画驱动入口。

pub mod binding;
pub mod system;

pub use binding::{Binding, BindingTarget, BindingTable, SeekKind};
pub use system::BindingApplySystem;
