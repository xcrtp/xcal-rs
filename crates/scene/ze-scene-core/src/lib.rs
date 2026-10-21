// ze-scene-core — 场景核心层（B3-1）
//
// 定位：**契约的"概念侧"**——场景对象、组件与纯逻辑。它不认识 ECS（hecs），
// 也不认识任何 `zela-*` 实现层；`zela-scene`（ECS 实现）依赖它，反之不成立。
//
// ```text
// ze-scene-contract   契约层（Scene / ObjectId / Describable / *Desc）
//         ↑
// ze-scene-core       本 crate：对象 + 组件 + 纯逻辑（零 hecs、零 zela-*）
//         ↑
// zela-scene          ECS 实现（world / resources / System 接线）
// ```
//
// # 与 `ze-object` 的关系
// 原 `ze-object` 的全部内容已并入 [`object`]；`ze-object` 现在是本 crate 的
// re-export 壳层（D6），下游 `ze_scene_core::*` 路径保持不变。
//
// # 为什么 `object` 里的模块在根也被 re-export
// `object/*.rs` 内部的 `crate::camera` / `crate::primitives` 等路径是搬迁前
// （`ze-object` 根）的写法；根级 `pub use` 让这些路径继续成立，搬迁零改写。

pub mod object;

// 组件 / 图元 / 纯数据（B3-1b 从 `zela-scene` 下沉）。
pub mod animation;
pub mod camera_input;
pub mod components;
pub mod composer;
/// 内容编译的宿主侧适配（排版引擎 → IR → 世界坐标轮廓）。
pub mod content;
pub mod domain;
/// 内置数学函数（Pod 提供）与持久化时的名字壳回绑。
pub mod math_pod;
/// 内置网格生成器（原 `zela-scene::scene::primitives`；`object::primitives` 是图元 newtype，
/// 两者同名不同义，故此处改名 `mesh_primitives`）。
pub mod mesh_primitives;
pub mod persist;
pub mod registry;
pub mod scene_script;
pub mod scene_write;
pub mod time_state;
pub mod transform;
pub mod types;
/// 每帧派生的世界矩阵容器（生产在 ECS 层，消费在渲染层）。
pub mod world_transforms;

// 让 `object/*.rs` 内部的 `crate::{affine, camera, …}` 继续解析（见文件头）。
pub use object::{affine, axes, camera, content_texture, graph, layer, placement, primitives, zorder};

pub use object::{
    AXIS_Z, Arrow, Axes2D, AxesLine, CONTENT_TEXTURE_HEIGHT, CONTENT_TEXTURE_ID,
    CONTENT_TEXTURE_WIDTH, CONTENT_Z, Camera2D, Camera2DHandle, Camera3D, Camera3DHandle, Circle,
    FPS_PITCH_LIMIT, FpsView, Framing2D, FunctionGraph, GRID_Z, LayerBlend, LayerId, LayerKind,
    Line, Path, Projection, Rect, TextureHandle, TickSpec, Transform, ZOrder, map_path_affine,
    spawn_camera2d, spawn_camera3d, uniform_scale,
};

pub use time_state::TimeState;
pub use transform::{Transform2D, Transform2DInto};
pub use types::{Color32, Rect2D, Vec2, Vec3, Vec4};
pub use world_transforms::WorldTransforms;

