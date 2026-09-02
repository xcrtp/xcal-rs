// render.rs

use crate::{
    scene::components::{Circle, FillSolid, Path, StrokeSolid},
    types::{Rect, Vec2},
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
