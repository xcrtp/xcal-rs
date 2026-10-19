// degrade.rs
//
// 「对象 → IR」的降级 trait：`ToShape2D`。
//
// **M2-3（解开成环的钥匙）**：接口降维到理念层 —— `ToShape2D` **不再吃
// `Camera2D`**（`Camera2D` 归场景对象层 `ze-object`，它依赖本层，IR 层若认识它
// 就成环），而是吃**调用点降级出的仿射矩阵** [`ze_notion::Transform2D`]。
//
// 精确形式（`Camera2D { center, scale, rect }` 的 world→screen）：
//
// ```text
// screen.x = rect.center.x + (world.x - center.x) * scale
// screen.y = rect.center.y - (world.y - center.y) * scale      // Y 翻转
// ```
//
// 作为 `Mat3`（列主序）：
//
// ```text
// col0 = [ s,  0, 0 ]
// col1 = [ 0, -s, 0 ]
// col2 = [ rc.x - c.x·s,  rc.y + c.y·s,  1 ]
// ```
//
// 该矩阵由 `Camera2D::to_transform2d()` 给出（回归基线：`Camera2D` 既有的
// `screen_to_world` / `world_to_screen` 往返测试仍须通过）。

use crate::shape::Shape2D;
use ze_notion::Transform2D;

/// 场景对象 → **屏幕空间** [`Shape2D`]。
///
/// 实现方（`ze-object` 的图元 newtype，或下游宿主的自定义图元）负责把自身几何
/// 经 `t` 映射到屏幕空间。`t` 是**相机降级矩阵**：其线性部分为 `diag(s, -s)`
/// （`s` = `Camera2D.scale`），故实现方可以从 `t.matrix.x_axis.x` 取回统一缩放
/// 因子（`Circle` 的半径、`Path2D::Arc` 的 radii 需要它）。
///
/// 公开以允许**下游 crate** 为新图元提供实现。
pub trait ToShape2D {
    fn to_shape2d(&self, t: &Transform2D) -> Shape2D;
}
