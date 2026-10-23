// animation — ECS 侧（B3-1b 后）
//
// 数据/词汇（Binding / Timeline / Target 及其实现）已下沉 `ze-scene-core::animation`；
// 本层只留**驱动 System**（它们需要 `ExecutorContext` / hecs 世界）。
//
// 路径兼容：`zela_scene_ecs::animation::{binding, target, timeline}` 与
// `zela_scene_ecs::animation::{Binding, Target, …}` 全部转发自核心层。

pub mod camera_drive;
pub mod morph;
pub mod system;

#[cfg(test)]
mod target_tests;

pub use camera_drive::CameraDriveSystem;
pub use morph::MorphApplySystem;
pub use system::BindingApplySystem;

pub use ze_scene_core::animation::{binding, target, timeline};
pub use ze_scene_core::animation::{
    Binding, BindingTable, Camera2DFraming, CameraEye, CircleRadius, MorphT, OpacityValue, Phase,
    RevealRange, SeekKind, Target, TargetCtor, TargetKind, Timeline, TransformChannel,
    TransformChannelValue,
};
