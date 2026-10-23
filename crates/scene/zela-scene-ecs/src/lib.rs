// zela-scene: 场景层
// 包含：错误、类型(数学+相机)、Transform2D trait 数学部分、
//      scene 中的 marker/mesh/animation 子模块 + Circle/Path/Line/Scene 等纯逻辑组件、
//      animation（绑定表 + 场景 System）、director_state（导演状态 + 场景 System）、
//      domain（场景语义组件）、utils 宏
//
// ECS 调度内核（resource/access/context/system/scheduler）与 Telemetry 已抽离到
// 独立的 `zela-schedule` crate。

pub mod animation;
pub mod camera_input;
pub mod composer;
pub mod contract;
pub mod deferred;
pub mod director_state;
pub mod driver;
pub mod errors;
pub mod geometry;
pub mod input;
pub mod lifetime;
pub mod register;
pub mod scene;
pub mod transform_hierarchy;
pub mod utils;

// B3-1c：语言无关的 Pod 边界已独立为 `ze-scene-api`；`zela_scene_ecs::pod` 路径保持不变。
pub use ze_scene_api as pod;

// B3-1b：内容编译宿主适配已下沉核心层；`zela_scene_ecs::content` 路径保持不变。
pub use ze_scene_core::content;

// B3-1b：纯数据 / 对象 / 组件已下沉 `ze-scene-core`；这里保留原公开路径。
pub use ze_scene_core::{domain, types};
pub use ze_scene_core::{Color32, Rect2D, Vec2, Vec3, Vec4};
pub use ze_scene_core::{TimeState, Transform2D, Transform2DInto};

pub use animation::{
    Binding, BindingTable, SeekKind, Target, TargetKind, Timeline, TransformChannel,
    TransformChannelValue,
};
pub use camera_input::{
    CameraInputSystem, apply_command_2d, apply_command_3d, apply_fps, apply_orbit, apply_pan_zoom,
};
pub use composer::{Composer, ComposerQueue, ComposerSystem, full_recompute};
pub use deferred::{
    DeferredApplySystem, DeferredCmd, DeferredCommands, DynamicObjects, ReplaySystem,
};
pub use director_state::{DirectorAdvanceSystem, DirectorState};
pub use driver::{DriverCommand, DriverEvent, DriverPod};
pub use errors::Error;
pub use geometry::{AxesRefreshSystem, FunctionGraphRefreshSystem};
pub use input::{ParamPanelSystem, TimeControlSystem};
pub use lifetime::{
    LifetimeEvent, LifetimeEventKind, LifetimeEvents, LifetimeFlushSystem, LifetimeTracker,
};
// B3-1b：数学函数 Pod 与持久化已下沉核心层；路径保持不变。
pub use ze_scene_core::{math_pod, persist};
pub use ze_scene_core::math_pod::{SharedPod, builtin_f32, function_from_pod, math_pod};
pub use ze_scene_core::persist::{
    PersistError, TaggedComponent, deserialize_components, serialize_components,
};
pub use scene::{
    components, comp, primitives, Scene,
};
// B3-1b：脚本数据（SceneScript / SceneEncoder）已下沉核心层；路径保持不变。
pub use ze_scene_core::scene_script;
pub use ze_scene_core::scene_script::{
    Handle, HandleRange, SceneEncoder, SceneRecipe, SceneScript, ScriptError, handle_ref,
};

#[cfg(test)]
mod scene_script_tests;
pub use transform_hierarchy::{
    TransformHierarchyError, TransformHierarchySystem, WorldTransforms,
};
// `Transform2D` 的模块路径（`ze_scene_core::transform`；本层转发以便下游少一个依赖）。
pub use ze_scene_core::transform;
pub use ze_time::{AnimValue, ClockForest, ClockId, ClockNode, Curve, Director, Interp, PlayMode};
// 相机与摆放组件归核心层对象模块：本层**不转发**具体类型，
// 消费方直接 `use ze_scene_core::{Camera2D, Camera3D, Projection, Transform}`。
// `#[macro_export]` 已经把 auto_into 宏导出到 crate 根（与模块无关），
// 下游通过 `#[macro_use]` 或 `zela_scene_ecs::auto_into!` 访问。

pub type Result<T> = std::result::Result<T, Error>;

// 本 crate 内部模块里的 `zela_scene_ecs::…` 绝对路径需要自别名（Rust 2018 起
// 路径不以自身 crate 名开头解析）。ze-macro 生成的是 `ze_scene_core::…`，不依赖它。
#[allow(unused_extern_crates)]
extern crate self as zela_scene_ecs;
