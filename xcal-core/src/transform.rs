// transform.rs
//
// 数学部分：Transform2D / Transform2DInto trait
// + glam 底层类型的 Transform2DInto 实现
// + Circle 的手动 Transform2DInto 实现（需要 radius 按 scale 放缩）
//
// 注：Painter2D/DrawQueue/SceneRender 等"绘制"部分属于 xcal-render，
// 不放在这里。

use crate::scene::components::Circle;
use crate::types::{Camera2D, Vec2};

/// 2D 坐标系转换 Trait（非消耗版本）。
///
/// 实现此 Trait 的对象可接受 [`Camera2D`] 相机参数，将自身在屏幕坐标系与
/// 逻辑坐标系之间相互转换。
///
/// - 屏幕坐标系：视口左上角为原点，y 向下，单位为像素。
/// - 逻辑坐标系：[`Camera2D::center`] 为中心，y 向上，单位为世界单位。
///
/// 对实现 [`Transform2DInto`] 且 `Clone` 的类型，本 Trait 由 blanket impl 自动提供，
/// 不需要手动实现。
pub trait Transform2D {
    /// 从屏幕坐标系转换到逻辑坐标系。
    fn to_logical(&self, camera: &Camera2D) -> Self;
    /// 从逻辑坐标系转换到屏幕坐标系。
    fn to_screen(&self, camera: &Camera2D) -> Self;
}

/// 2D 坐标系转换 Trait（消耗 `self` 版本）。
///
/// 与 [`Transform2D`] 的区别：方法消耗 `self`，可以避免额外的 `clone`。
/// 派生方式：在结构体上添加 `#[derive(xcal_macro::Transform2DInto)]`。
pub trait Transform2DInto {
    /// 从屏幕坐标系转换到逻辑坐标系（消耗 self）。
    fn to_logical_into(self, camera: &Camera2D) -> Self;
    /// 从逻辑坐标系转换到屏幕坐标系（消耗 self）。
    fn to_screen_into(self, camera: &Camera2D) -> Self;
}

/// 自动为实现了 [`Transform2DInto`] 且 `Clone` 的类型派生 [`Transform2D`]。
impl<T: Transform2DInto + Clone> Transform2D for T {
    fn to_logical(&self, camera: &Camera2D) -> Self {
        self.clone().to_logical_into(camera)
    }
    fn to_screen(&self, camera: &Camera2D) -> Self {
        self.clone().to_screen_into(camera)
    }
}

// ─── 底层 glam 类型的 Transform2DInto 实现 ─────────────────────────────────

impl Transform2DInto for glam::Vec2 {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        let rect_center = camera.rect.center();
        glam::Vec2::new(
            (self.x - rect_center.x) / camera.scale + camera.center.x,
            (rect_center.y - self.y) / camera.scale + camera.center.y,
        )
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        let rect_center = camera.rect.center();
        glam::Vec2::new(
            rect_center.x + (self.x - camera.center.x) * camera.scale,
            rect_center.y - (self.y - camera.center.y) * camera.scale,
        )
    }
}

impl Transform2DInto for glam::Vec3 {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        let p: Vec2 = (self.x, self.y).into();
        let p = p.to_logical_into(camera);
        glam::Vec3::new(p.x, p.y, self.z)
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        let p: Vec2 = (self.x, self.y).into();
        let p = p.to_screen_into(camera);
        glam::Vec3::new(p.x, p.y, self.z)
    }
}

// ─── Circle 的 Transform2DInto 手动实现 ─────────────────────────────────────

impl Transform2DInto for Circle {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        Circle::new(
            self.center.to_logical_into(camera),
            self.radius / camera.scale,
        )
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        Circle::new(
            self.center.to_screen_into(camera),
            self.radius * camera.scale,
        )
    }
}
