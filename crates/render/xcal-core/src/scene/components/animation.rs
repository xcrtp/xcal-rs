// animation.rs
//
// 动画组件：ClockState / LocalClock / CircleAnim / PhaseAnim，
// 以及 3D 相机动画 OrbitAnim（子模块 orbit）。

/// 轨道相机动画（3D）。
pub mod orbit;

#[derive(Clone, Debug)]
pub enum ClockState {
    Step,
    Suspend,
    Loop,
}

#[derive(Debug, Clone, derive_builder::Builder, derive_new::new)]
pub struct LocalClock {
    pub parent: Option<hecs::Entity>,
    pub parent_offset: f32,
    pub start: f32,
    pub speed: f32,
    pub current: f32,
    pub state: ClockState,
}

/// 圆形半径 sin 振荡参数：`radius = base + amp * sin(t * omega)`。
/// 挂在带 `Circle` 的实体上，由 `AnimationApplySystem` 每帧写入 `Circle.radius`。
#[derive(Debug, Clone, derive_builder::Builder, derive_new::new)]
pub struct CircleAnim {
    pub base: f32,
    pub amp: f32,
    pub omega: f32,
}

/// FunctionX2Y 相位滑动参数：`phase = t * omega`，
/// 由 `SceneRecordSystem` 采样时取 `x + phase` 实现曲线水平平移。
/// `omega` 为常量；`phase` 由 `AnimationApplySystem` 每帧覆盖。
#[derive(Debug, Clone, derive_builder::Builder, derive_new::new)]
pub struct PhaseAnim {
    pub phase: f32,
    pub omega: f32,
}

pub use orbit::OrbitAnim;
