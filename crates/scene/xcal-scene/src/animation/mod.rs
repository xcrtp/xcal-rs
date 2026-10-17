// animation 模块：绑定层（Binding）把时钟树和动画曲线显式连接到 hecs 组件。
//
// 设计原则：
// 1. 动画是纯函数 f(t) → 值，永不累积状态（由 xc-time::Curve::sample 保证）。
// 2. Binding 表取代旧的组件组合查询（query::<(&LocalClock, &CircleAnim, &mut Circle)>()，
//    旧动画组件已删除）。
// 3. BindingApplySystem 是唯一的动画驱动入口。
// 4. 驱动目标是 `Arc<dyn Target>`（P0 类型擦除）—— 新增目标不改枚举、不改 System。
// 5. Timeline（P3）把"先…再…"的顺序编排编译成 `ClockNode + Binding`，求值层零改动。

pub mod binding;
pub mod morph;
pub mod system;
pub mod target;
pub mod timeline;

pub use binding::{Binding, BindingTable, SeekKind};
pub use morph::MorphApplySystem;
pub use system::BindingApplySystem;
pub use target::{
    Camera2DFraming, CameraEye, CircleRadius, MorphT, OpacityValue, Phase, RevealRange, Target,
    TargetCtor, TransformValue,
};
pub use timeline::Timeline;
