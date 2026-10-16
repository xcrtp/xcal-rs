// types.rs — 场景层的数学类型出口（转发理念层）。
//
// M2.5：`Vec2` / `Vec3` / `Vec4` 由本地 newtype 改为**理念层转发**
// （此前它们是 `glam::*` 的 newtype ⇒ 与 `xc_notion::*` 是两个不同类型）。
//
// M3-2：`Camera2D` 与视口 `Rect` 已迁出本层 ——
// - `Camera2D`（连同 `Camera3D` / `Projection`）归**场景对象层** `xc-object`；
// - 视口版的 `Rect { min, max }`（私有字段 + 访问器）**已删除**：它与理念层
//   `Rect2D { min, max }` 字段完全相同，是纯重复定义，故 `Camera2D.rect`
//   现在直接用 `xc_notion::Rect2D`。
//
// 本模块现在只做"理念层数学类型的场景层转发"，不再定义任何类型。

pub use xc_notion::{Color32, Rect2D, Vec2, Vec3, Vec4};
