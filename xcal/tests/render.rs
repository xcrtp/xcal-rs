//! render 模块集成测试
//!
//! 测试 Painter2D trait 的公开接口。
//! SceneRender 的测试在 render.rs 内联单元测试中覆盖（因其 painter 字段为私有）。

use xcal::render::Painter2D;
use xcal::scene::components::{Circle, FillSolid, Path, StrokeSolid};
use xcal::types::{Rect, Vec2};

/// 用于测试的 Mock Painter
struct MockPainter {
    center_val: f32,
    scale_val: f32,
    rect_val: Rect,
}

impl Painter2D for MockPainter {
    fn center(&self) -> f32 {
        self.center_val
    }
    fn scale(&self) -> f32 {
        self.scale_val
    }
    fn rect(&self) -> Rect {
        self.rect_val
    }
    fn circle_stroke(&self, _circle: &Circle, _stroke: &StrokeSolid) {}
    fn circle_fill(&self, _circle: &Circle, _fill: &FillSolid) {}
    fn path_stroke(&self, _path: &Path, _stroke: &StrokeSolid) {}
    fn path_fill(&self, _path: &Path, _fill: &FillSolid) {}
}

fn make_rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::new((x0, y0).into(), (x1, y1).into())
}

// ─── Painter2D trait 接口测试 ──────────────────────────────

#[test]
fn painter2d_center() {
    let p = MockPainter {
        center_val: 0.5,
        scale_val: 1.0,
        rect_val: make_rect(0.0, 0.0, 100.0, 100.0),
    };
    assert_eq!(p.center(), 0.5);
}

#[test]
fn painter2d_scale() {
    let p = MockPainter {
        center_val: 0.0,
        scale_val: 40.0,
        rect_val: make_rect(0.0, 0.0, 100.0, 100.0),
    };
    assert_eq!(p.scale(), 40.0);
}

#[test]
fn painter2d_rect() {
    let p = MockPainter {
        center_val: 0.0,
        scale_val: 1.0,
        rect_val: make_rect(10.0, 20.0, 110.0, 220.0),
    };
    let r = p.rect();
    let size = r.size();
    assert_eq!(size.x, 100.0);
    assert_eq!(size.y, 200.0);
}

// ─── Painter2D trait 默认方法 math_to_screen 测试 ──────────

#[test]
fn painter2d_math_to_screen_basic() {
    let p = MockPainter {
        center_val: 0.0,
        scale_val: 1.0,
        rect_val: make_rect(0.0, 0.0, 100.0, 100.0),
    };
    let pos: Vec2 = (5.0, 5.0).into();
    let center: Vec2 = (0.0, 0.0).into();
    let rect = make_rect(0.0, 0.0, 100.0, 100.0);
    // rect center = (50, 50)
    // screen = (50 + (5-0)*1, 50 - (5-0)*1) = (55, 45)
    let screen = p.math_to_screen(pos, center, 1.0, rect);
    assert_eq!(screen.x, 55.0);
    assert_eq!(screen.y, 45.0);
}

#[test]
fn painter2d_math_to_screen_with_scale() {
    let p = MockPainter {
        center_val: 0.0,
        scale_val: 10.0,
        rect_val: make_rect(0.0, 0.0, 800.0, 600.0),
    };
    let pos: Vec2 = (3.0, 4.0).into();
    let center: Vec2 = (0.0, 0.0).into();
    let rect = make_rect(0.0, 0.0, 800.0, 600.0);
    // rect center = (400, 300)
    // screen = (400 + 3*10, 300 - 4*10) = (430, 260)
    let screen = p.math_to_screen(pos, center, 10.0, rect);
    assert_eq!(screen.x, 430.0);
    assert_eq!(screen.y, 260.0);
}

#[test]
fn painter2d_math_to_screen_with_offset_center() {
    let p = MockPainter {
        center_val: 5.0,
        scale_val: 2.0,
        rect_val: make_rect(0.0, 0.0, 200.0, 200.0),
    };
    let pos: Vec2 = (10.0, 10.0).into();
    let center: Vec2 = (5.0, 5.0).into();
    let rect = make_rect(0.0, 0.0, 200.0, 200.0);
    // rect center = (100, 100)
    // screen = (100 + (10-5)*2, 100 - (10-5)*2) = (110, 90)
    let screen = p.math_to_screen(pos, center, 2.0, rect);
    assert_eq!(screen.x, 110.0);
    assert_eq!(screen.y, 90.0);
}

#[test]
fn painter2d_math_to_screen_negative_coords() {
    let p = MockPainter {
        center_val: 0.0,
        scale_val: 1.0,
        rect_val: make_rect(0.0, 0.0, 100.0, 100.0),
    };
    let pos: Vec2 = (-5.0, -5.0).into();
    let center: Vec2 = (0.0, 0.0).into();
    let rect = make_rect(0.0, 0.0, 100.0, 100.0);
    // rect center = (50, 50)
    // screen = (50 + (-5)*1, 50 - (-5)*1) = (45, 55)
    let screen = p.math_to_screen(pos, center, 1.0, rect);
    assert_eq!(screen.x, 45.0);
    assert_eq!(screen.y, 55.0);
}

#[test]
fn painter2d_math_to_screen_origin() {
    let p = MockPainter {
        center_val: 0.0,
        scale_val: 1.0,
        rect_val: make_rect(0.0, 0.0, 100.0, 100.0),
    };
    let pos: Vec2 = (0.0, 0.0).into();
    let center: Vec2 = (0.0, 0.0).into();
    let rect = make_rect(0.0, 0.0, 100.0, 100.0);
    // screen = rect center = (50, 50)
    let screen = p.math_to_screen(pos, center, 1.0, rect);
    assert_eq!(screen.x, 50.0);
    assert_eq!(screen.y, 50.0);
}
