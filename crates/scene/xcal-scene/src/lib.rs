// xcal-scene: 场景层
// 包含：错误、类型(数学+相机)、Transform2D trait 数学部分、
//      scene 中的 marker/mesh/animation 子模块 + Circle/Path/Line/Scene 等纯逻辑组件、
//      animation（绑定表 + 场景 System）、director_state（导演状态 + 场景 System）、
//      domain（场景语义组件）、utils 宏
//
// ECS 调度内核（resource/access/context/system/scheduler）与 Telemetry 已抽离到
// 独立的 `xcal-schedule` crate。

pub mod animation;
pub mod contract;
pub mod director_state;
pub mod domain;
pub mod errors;
pub mod scene;
pub mod transform;
pub mod time_state;
pub mod types;
pub mod utils;

pub use errors::Error;
pub use scene::{Scene, comp, components, primitives};
pub use transform::{Transform2D, Transform2DInto};
pub use time_state::TimeState;
pub use xc_time::{AnimValue, ClockForest, ClockId, ClockNode, Curve, Director, PlayMode};
pub use animation::{BindingTable, Binding, BindingTarget, SeekKind};
pub use director_state::{DirectorAdvanceSystem, DirectorState};
pub use types::{Color32, Rect2D, Vec2, Vec3, Vec4};
// 相机（M3-2）与摆放组件（M3-4）归场景对象层 `xc-object`：本层**不转发**，
// 消费方直接 `use xc_object::{Camera2D, Camera3D, Projection, Transform}`。
// `#[macro_export]` 已经把 auto_into 宏导出到 crate 根（与模块无关），
// 下游通过 `#[macro_use]` 或 `xcal_scene::auto_into!` 访问。

pub type Result<T> = std::result::Result<T, Error>;

// 为 xc-macro 派生宏生成的 `xcal_scene::` 路径提供别名。
// xc-macro 现在生成 `xcal_scene::transform::Transform2DInto` 等。
#[allow(unused_extern_crates)]
extern crate self as xcal_scene;
