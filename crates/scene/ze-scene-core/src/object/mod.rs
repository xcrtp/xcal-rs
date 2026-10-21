// object — 场景对象（原 `ze-object` crate，B3-1b 并入核心层）
//
// 用户可见的**场景对象**：图元 newtype（`Circle` / `Path` / `Line` / `Rect` / `Arrow`）、
// 摆放组件（`Transform` / `ZOrder`）与**装配对象**（`Axes2D` / `FunctionGraph`）。
//
// 为什么 `ToShape2D` 的 impl 必须在本模块（**孤儿规则**）：
// trait 在 IR 层、图元在对象层 —— 只有"实现里出现本地类型"才合法。
//
// 为什么本层依赖契约层：`#[derive(Describable)]` 在**本层的类型上**展开，
// 展开代码引用 `ze_scene_contract::Describable` / `*Desc` —— 真实使用，不是假依赖。

pub mod affine;
pub mod axes;
pub mod camera;
pub mod content_texture;
pub mod graph;
pub mod layer;
pub mod placement;
pub mod primitives;
pub mod zorder;

pub use affine::{map_path_affine, uniform_scale};
pub use axes::{AXIS_Z, Axes2D, AxesLine, GRID_Z, TickSpec};
pub use camera::{
    Camera2D, Camera2DHandle, Camera3D, Camera3DHandle, FPS_PITCH_LIMIT, FpsView, Framing2D,
    Projection, spawn_camera2d, spawn_camera3d,
};
pub use content_texture::{CONTENT_TEXTURE_HEIGHT, CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH};
pub use graph::{CONTENT_Z, FunctionGraph};
pub use layer::{LayerBlend, LayerId, LayerKind, TextureHandle};
pub use placement::Transform;
pub use primitives::{Arrow, Circle, Line, Path, Rect};
pub use zorder::ZOrder;
