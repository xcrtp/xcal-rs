//! render 模块集成测试
//!
//! 测试 Painter2D trait 的公开接口（当前仅 [`Painter2D::paint`]）。
//! Shape/Style 分发与 DrawQueue 的排序/归并测试在 render.rs 内联单元测试中覆盖。

use std::cell::RefCell;

use xcal::render::Painter2D;
use xcal::scene::components::{Circle, FillSolid, Line, Path, StrokeSolid};
use xcal::types::Vec3;
use xcal::{render::Shape, render::Style};

/// 记录型 Painter：把每次 `paint` 收到的 (Shape 变体, Style 变体) 记成字符串。
/// 用于断言 Painter2D 实现对 (Shape, Style) 的处理。
#[derive(Default)]
struct RecordingPainter {
    calls: RefCell<Vec<&'static str>>,
}

impl Painter2D for RecordingPainter {
    fn paint(&self, shape: &Shape, style: &Style) {
        let s = match (shape, style) {
            (Shape::Circle(_), Style::Fill(_)) => "circle_fill",
            (Shape::Circle(_), Style::Stroke(_)) => "circle_stroke",
            (Shape::Path(_), Style::Fill(_)) => "path_fill",
            (Shape::Path(_), Style::Stroke(_)) => "path_stroke",
            (Shape::Line(_), Style::Fill(_)) => "line_fill",
            (Shape::Line(_), Style::Stroke(_)) => "line_stroke",
            (Shape::Rect(_), Style::Fill(_)) => "rect_fill",
            (Shape::Rect(_), Style::Stroke(_)) => "rect_stroke",
        };
        self.calls.borrow_mut().push(s);
    }
}

fn mk_circle(x: f32) -> Circle {
    Circle::new((x, 0.0), 1.0)
}
fn mk_stroke() -> StrokeSolid {
    StrokeSolid::new(1.0, (0, 0, 0, 255))
}
fn mk_fill() -> FillSolid {
    FillSolid::new((0, 0, 0, 255))
}
fn mk_path() -> Path {
    Path::new(
        vec![
            Vec3::new(glam::Vec3::new(0.0, 0.0, 0.0)),
            Vec3::new(glam::Vec3::new(1.0, 0.0, 0.0)),
        ],
        false,
    )
}
fn mk_line() -> Line {
    Line::new((0.0, 0.0), (1.0, 1.0))
}

// ─── Painter2D::paint 分发测试 ─────────────────────────────

#[test]
fn painter2d_paint_circle_stroke() {
    let p = RecordingPainter::default();
    p.paint(&Shape::Circle(mk_circle(0.0)), &Style::Stroke(mk_stroke()));
    assert_eq!(*p.calls.borrow(), vec!["circle_stroke"]);
}

#[test]
fn painter2d_paint_circle_fill() {
    let p = RecordingPainter::default();
    p.paint(&Shape::Circle(mk_circle(0.0)), &Style::Fill(mk_fill()));
    assert_eq!(*p.calls.borrow(), vec!["circle_fill"]);
}

#[test]
fn painter2d_paint_path_stroke() {
    let p = RecordingPainter::default();
    p.paint(&Shape::Path(mk_path()), &Style::Stroke(mk_stroke()));
    assert_eq!(*p.calls.borrow(), vec!["path_stroke"]);
}

#[test]
fn painter2d_paint_path_fill() {
    let p = RecordingPainter::default();
    p.paint(&Shape::Path(mk_path()), &Style::Fill(mk_fill()));
    assert_eq!(*p.calls.borrow(), vec!["path_fill"]);
}

#[test]
fn painter2d_paint_line_stroke() {
    let p = RecordingPainter::default();
    p.paint(&Shape::Line(mk_line()), &Style::Stroke(mk_stroke()));
    assert_eq!(*p.calls.borrow(), vec!["line_stroke"]);
}

#[test]
fn painter2d_paint_sequence_preserved() {
    let p = RecordingPainter::default();
    p.paint(&Shape::Circle(mk_circle(1.0)), &Style::Stroke(mk_stroke()));
    p.paint(&Shape::Path(mk_path()), &Style::Fill(mk_fill()));
    p.paint(&Shape::Line(mk_line()), &Style::Stroke(mk_stroke()));
    assert_eq!(
        *p.calls.borrow(),
        vec!["circle_stroke", "path_fill", "line_stroke"]
    );
}
