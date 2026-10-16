// xc-object — 场景对象层（新层 `crates/object/`，M3 建立）
//
// 用户可见的**场景对象**：图元 newtype（`Circle` / `Path` / `Line` / `Rect`）。
// 它们把"理念层的数学值"封装成"可放进场景、可被描述符驱动样式"的对象，
// 并实现渲染 IR 的降级 trait（`ToShape2D`）。
//
// 分层位置：
//
// ```text
// xc-notion          理念层      数学值 + 代数运算（Circle2D / Path2D / Rect2D / …）
// xc-render-ir       渲染 IR     Shape2D / Style2D / ToShape2D / ResolveStyle
// xc-scene-contract  契约层      Scene / ObjectId / Describable / *Desc
// xc-object          场景对象层  Circle / Path / Line / Rect                     ← 本 crate
// xcal-scene         ECS 实现    组件存储 + 场景 System（Camera2D 暂留此处）
// ```
//
// 为什么 `ToShape2D` 的 impl 必须在本 crate（**孤儿规则**）：
// trait 在 IR 层、图元在对象层 —— 只有"实现里出现本地类型"才合法。计划原定
// "M2-3 先降级接口、M3 再搬图元"，但两者在 Rust 里必须**同时**落地（否则 `impl
// ToShape2D for Circle` 既不在 IR 层（类型不本地）也不在旧的 xcal-render
// （trait 不本地））。见交付说明。
//
// 本轮**尚未迁入**（延后，见交付说明）：`Camera2D` / `Camera3D` / `Projection` /
// `Transform` 摆放组件 / `Describable` 派生宏。故本层当前不依赖 `xc-scene-contract`。

pub mod affine;
pub mod primitives;

pub use affine::{map_path_affine, uniform_scale};
pub use primitives::{Circle, Line, Path, Rect};
