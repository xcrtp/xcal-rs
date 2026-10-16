// xc-object — 场景对象层（新层 `crates/object/`，M3 建立）
//
// 用户可见的**场景对象**：图元 newtype（`Circle` / `Path` / `Line` / `Rect`）、
// 摆放组件（`Transform` / `ZOrder`）与**装配对象**（`Axes2D` / `FunctionGraph`）。
// 它们把"理念层的数学值"封装成"可放进场景、可被描述符驱动样式"的对象，
// 并实现渲染 IR 的降级 trait（`ToShape2D`）与契约层的样式描述符接口（`Describable`）。
//
// 分层位置：
//
// ```text
// xc-notion          理念层      数学值 + 代数运算（Circle2D / Path2D / Rect2D / …）
// xc-render-ir       渲染 IR     Shape2D / Style2D / ToShape2D / ResolveStyle
// xc-scene-contract  契约层      Scene / ObjectId / Parent / Describable / *Desc
// xc-object          场景对象层  Circle / Path / Line / Rect / Transform    ← 本 crate
// xcal-scene         ECS 实现    组件存储 + 场景 System
// ```
//
// 为什么 `ToShape2D` 的 impl 必须在本 crate（**孤儿规则**）：
// trait 在 IR 层、图元在对象层 —— 只有"实现里出现本地类型"才合法。计划原定
// "M2-3 先降级接口、M3 再搬图元"，但两者在 Rust 里必须**同时**落地。
//
// 为什么本层依赖契约层：`#[derive(Describable)]` 在**本层的类型上**展开，
// 展开代码引用 `xc_scene_contract::Describable` / `*Desc` —— 这是**真实使用**，
// 不是为凑 FanIn 而声明的假依赖（假依赖正是 P0-5 删掉的那类疤痕）。

pub mod affine;
pub mod axes;
pub mod camera;
pub mod content_texture;
pub mod graph;
pub mod placement;
pub mod primitives;
pub mod zorder;

pub use affine::{map_path_affine, uniform_scale};
pub use axes::{AXIS_Z, Axes2D, AxesLine, GRID_Z, TickSpec};
pub use camera::{Camera2D, Camera2DHandle, Camera3D, Framing2D, Projection, spawn_camera2d};
pub use content_texture::{CONTENT_TEXTURE_HEIGHT, CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH};
pub use graph::{CONTENT_Z, FunctionGraph};
pub use placement::Transform;
pub use primitives::{Circle, Line, Path, Rect};
pub use zorder::ZOrder;
