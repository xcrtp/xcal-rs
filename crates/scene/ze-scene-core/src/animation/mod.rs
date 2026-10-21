// animation — 动画**数据与词汇**（B3-1b 从 `zela-scene` 下沉）
//
// 设计原则（与迁移前一致）：
// 1. 动画是纯函数 f(t) → 值，永不累积状态（由 ze-time::Curve::sample 保证）。
// 2. Binding 表取代旧的组件组合查询；BindingApplySystem 是唯一驱动入口（在 ECS 层）。
// 3. 驱动目标是 `Arc<dyn Target>`（P0 类型擦除）—— 新增目标不改枚举、不改 System。
// 4. 写场景经 [`crate::scene_write::SceneWrite`]（类型擦除）⇒ 本模块不认识 hecs。
//
// ECS 侧（`zela-scene`）保留三个 System：`BindingApplySystem` / `CameraDriveSystem` /
// `MorphApplySystem`，并通过 re-export 保持 `zela_scene::animation::*` 路径不变。

pub mod binding;
pub mod target;
pub mod timeline;

pub use binding::{Binding, BindingTable, SeekKind};
pub use target::{
    Camera2DFraming, CameraEye, CircleRadius, MorphT, OpacityValue, Phase, RevealRange, Target,
    TargetCtor, TargetKind, TransformValue,
};
pub use timeline::Timeline;
