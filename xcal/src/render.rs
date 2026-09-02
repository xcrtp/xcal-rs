// render.rs

use crate::{
    scene::components::{Circle, FillSolid, Path, StrokeSolid},
    types::{Camera2D, Rect, Vec2},
};

pub trait Painter2D {
    fn center(&self) -> f32;
    fn scale(&self) -> f32;
    fn rect(&self) -> Rect;
    fn circle_stroke(&self, cicle: &Circle, stroke: &StrokeSolid);
    fn circle_fill(&self, cicle: &Circle, fill: &FillSolid);
    fn path_stroke(&self, path: &Path, stroke: &StrokeSolid);
    fn path_fill(&self, path: &Path, fill: &FillSolid);

    // default methods
    fn math_to_screen(&self, pos: Vec2, center: Vec2, scale: f32, rect: Rect) -> Vec2 {
        (
            rect.center().x + (pos.x - center.x) * scale,
            rect.center().y - (pos.y - center.y) * scale,
        )
            .into()
    }
}

/// 2D 坐标系转换 Trait（非消耗版本）。
///
/// 实现此 Trait 的对象可接受 [`Camera2D`] 相机参数，将自身在屏幕坐标系与
/// 逻辑坐标系之间相互转换，从而简化 2D Painter 的实现：调用方只需传入
/// 相机参数，由对象自行完成坐标变换，Painter 不必关心每个图元的细节。
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
/// 适用于所有权可移动的场景，例如迭代器内部转换、过程宏派生等。
///
/// 派生方式：在结构体上添加 `#[derive(xcal_macro::Transform2DInto)]`。
/// 字段可用 `#[transform(skip)]` 跳过转换、`#[transform(iter)]` 走迭代器转换。
pub trait Transform2DInto {
    /// 从屏幕坐标系转换到逻辑坐标系（消耗 self）。
    fn to_logical_into(self, camera: &Camera2D) -> Self;
    /// 从逻辑坐标系转换到屏幕坐标系（消耗 self）。
    fn to_screen_into(self, camera: &Camera2D) -> Self;
}

/// 自动为实现了 [`Transform2DInto`] 且 `Clone` 的类型派生 [`Transform2D`]。
/// 内部实现是先 `clone` 再调用 `*_into` 版本。
impl<T: Transform2DInto + Clone> Transform2D for T {
    fn to_logical(&self, camera: &Camera2D) -> Self {
        self.clone().to_logical_into(camera)
    }
    fn to_screen(&self, camera: &Camera2D) -> Self {
        self.clone().to_screen_into(camera)
    }
}

// ─── 底层 glam 类型的 Transform2DInto 实现 ─────────────────────────────────
// 由于 glam::Vec2 / glam::Vec3 是外部类型，但 Transform2DInto 是本 crate 的 trait，
// 孤儿规则允许为其实现。这些实现是所有 Vec2/Vec3 包装类型（在 types.rs 中通过
// `#[derive(xcal_macro::Transform2DInto)]` 派生）的底层依赖。

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
        // 对 3D 点做 2D 转换：x/y 走 2D 变换，z 维度保持原值。
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
// 不能直接派生：radius 需要按相机 scale 放缩（`/ scale` 或 `* scale`），
// 默认派生只能 move 或调用 `to_*_into`，无法表达这种算术关系。
impl Transform2DInto for Circle {
    fn to_logical_into(self, camera: &Camera2D) -> Self {
        Circle::new(self.center.to_logical_into(camera), self.radius / camera.scale)
    }

    fn to_screen_into(self, camera: &Camera2D) -> Self {
        Circle::new(self.center.to_screen_into(camera), self.radius * camera.scale)
    }
}

macro_rules! iter_render {
    ($f:ident($($p:ident: $t:ty),+) $w:ident,$self:ident ) => {
        $w
            .query::<($(&$t,)*)>()
            .into_iter()
            .for_each(|($($p),*)| {
                $self.painter.$f($($p,)*);
            });
    };
}

pub struct SceneRender<T: Painter2D> {
    painter: T,
}
impl<T: Painter2D> SceneRender<T> {
    pub fn render(&self, world: &hecs::World) {
        iter_render!(circle_stroke(circle: Circle, stroke: StrokeSolid) world, self);
        iter_render!(circle_fill(circle: Circle, fill: FillSolid) world,self);
        iter_render!(path_stroke(path: Path, stroke: StrokeSolid) world,self);
        iter_render!(path_fill(path: Path, fill: FillSolid) world,self);
    }
}

#[cfg(feature = "egui")]
pub mod egui_support {
    use egui::{Painter, Rect};

    pub struct Egui2dRender<'a> {
        painter: &'a Painter,
        rect: Rect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::components::{Circle, FillSolid, Path, StrokeSolid};
    use crate::types::{Rect, Vec2};

    struct MockPainter;

    impl Painter2D for MockPainter {
        fn center(&self) -> f32 {
            0.0
        }
        fn scale(&self) -> f32 {
            1.0
        }
        fn rect(&self) -> Rect {
            Rect::new((0.0, 0.0).into(), (100.0, 100.0).into())
        }
        fn circle_stroke(&self, _: &Circle, _: &StrokeSolid) {}
        fn circle_fill(&self, _: &Circle, _: &FillSolid) {}
        fn path_stroke(&self, _: &Path, _: &StrokeSolid) {}
        fn path_fill(&self, _: &Path, _: &FillSolid) {}
    }

    #[test]
    fn test_math_to_screen_basic() {
        let painter = MockPainter;
        let pos: Vec2 = (5.0, 5.0).into();
        let center: Vec2 = (0.0, 0.0).into();
        let rect = Rect::new((0.0, 0.0).into(), (100.0, 100.0).into());
        let screen = painter.math_to_screen(pos, center, 1.0, rect);
        assert_eq!(screen.x, 55.0);
        assert_eq!(screen.y, 45.0);
    }

    #[test]
    fn test_math_to_screen_with_scale() {
        let painter = MockPainter;
        let pos: Vec2 = (3.0, 4.0).into();
        let center: Vec2 = (0.0, 0.0).into();
        let rect = Rect::new((0.0, 0.0).into(), (800.0, 600.0).into());
        let screen = painter.math_to_screen(pos, center, 10.0, rect);
        assert_eq!(screen.x, 430.0);
        assert_eq!(screen.y, 260.0);
    }

    #[test]
    fn test_scene_render_circle_stroke() {
        let mut world = hecs::World::new();
        world.spawn((
            Circle::new((0.0, 0.0), 10.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
        ));
        let renderer = SceneRender { painter: MockPainter };
        renderer.render(&world);
    }

    #[test]
    fn test_scene_render_circle_fill() {
        let mut world = hecs::World::new();
        world.spawn((
            Circle::new((5.0, 5.0), 20.0),
            FillSolid::new((0, 255, 0, 128)),
        ));
        let renderer = SceneRender { painter: MockPainter };
        renderer.render(&world);
    }

    #[test]
    fn test_scene_render_path_stroke() {
        let mut world = hecs::World::new();
        let path = Path::new(
            vec![
                crate::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                crate::types::Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
            ],
            false,
        );
        world.spawn((path, StrokeSolid::new(1.0, (0, 0, 255, 255))));
        let renderer = SceneRender { painter: MockPainter };
        renderer.render(&world);
    }

    #[test]
    fn test_scene_render_empty_world() {
        let world = hecs::World::new();
        let renderer = SceneRender { painter: MockPainter };
        renderer.render(&world);
    }

    #[test]
    fn test_scene_render_mixed_entities() {
        let mut world = hecs::World::new();
        world.spawn((
            Circle::new((0.0, 0.0), 10.0),
            StrokeSolid::new(2.0, (255, 0, 0, 255)),
        ));
        world.spawn((
            Circle::new((5.0, 5.0), 20.0),
            FillSolid::new((0, 255, 0, 128)),
        ));
        let path = Path::new(
            vec![
                crate::types::Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
                crate::types::Vec3::new(glam::Vec3::new(1.0, 1.0, 0.0)),
            ],
            false,
        );
        world.spawn((path, FillSolid::new((0, 0, 255, 255))));
        let renderer = SceneRender { painter: MockPainter };
        renderer.render(&world);
    }
}
