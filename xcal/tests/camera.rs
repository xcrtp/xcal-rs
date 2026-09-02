//! app 模块 AppCamera 集成测试
//!
//! AppCamera 现仅负责交互状态（center/scale），坐标转换由 [`xcal::types::Camera2D`]
//! 配合 [`xcal::render::Transform2D`] 完成。本文件同时覆盖两者。

use egui::Pos2;
use xcal::app::AppCamera;
use xcal::render::Transform2D;
use xcal::types::{Camera2D, Rect, Vec2};

fn make_rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::new((x0, y0).into(), (x1, y1).into())
}

// ─── app::AppCamera 状态测试 ──────────────────────────────────

#[test]
fn camera_default_values() {
    let c = AppCamera::default();
    assert_eq!(c.center, Pos2::new(0.0, 0.0));
    assert_eq!(c.scale, 40.0);
}

#[test]
fn camera_can_be_cloned() {
    let c = AppCamera::default();
    let c2 = c.clone();
    assert_eq!(c.center, c2.center);
    assert_eq!(c.scale, c2.scale);
}

#[test]
fn camera_custom_construction() {
    let c = AppCamera {
        center: Pos2::new(10.0, 20.0),
        scale: 50.0,
    };
    assert_eq!(c.center, Pos2::new(10.0, 20.0));
    assert_eq!(c.scale, 50.0);
}

// ─── Camera2D + Transform2D 坐标转换测试 ───────────────────
//
// 转换关系（见 types::Camera2D 文档）：
// - 逻辑 → 屏幕：screen = rect.center + (logical - center) * scale，y 取反
// - 屏幕 → 逻辑：logical = (screen - rect.center) / scale + center，y 取反

fn default_camera2d() -> Camera2D {
    // center=(0,0), scale=40, rect=(0,0)-(800,600) → rect.center=(400,300)
    Camera2D::new((0.0, 0.0).into(), 40.0, make_rect(0.0, 0.0, 800.0, 600.0))
}

#[test]
fn camera2d_to_screen_origin() {
    let cam = default_camera2d();
    let screen: Vec2 = Vec2::from((0.0, 0.0)).to_screen(&cam);
    assert_eq!(screen.x, 400.0);
    assert_eq!(screen.y, 300.0);
}

#[test]
fn camera2d_to_screen_positive() {
    let cam = default_camera2d();
    // (1,1) → (400+40, 300-40) = (440, 260)
    let screen: Vec2 = Vec2::from((1.0, 1.0)).to_screen(&cam);
    assert_eq!(screen.x, 440.0);
    assert_eq!(screen.y, 260.0);
}

#[test]
fn camera2d_to_screen_negative() {
    let cam = default_camera2d();
    // (-1,-1) → (400-40, 300+40) = (360, 340)
    let screen: Vec2 = Vec2::from((-1.0, -1.0)).to_screen(&cam);
    assert_eq!(screen.x, 360.0);
    assert_eq!(screen.y, 340.0);
}

#[test]
fn camera2d_to_logical_origin() {
    let cam = default_camera2d();
    // (400,300) → (0,0)
    let logical: Vec2 = Vec2::from((400.0, 300.0)).to_logical(&cam);
    assert!(logical.x.abs() < 1e-5);
    assert!(logical.y.abs() < 1e-5);
}

#[test]
fn camera2d_to_logical_offset() {
    let cam = default_camera2d();
    // (440,260) → (1,1)
    let logical: Vec2 = Vec2::from((440.0, 260.0)).to_logical(&cam);
    assert!((logical.x - 1.0).abs() < 1e-5);
    assert!((logical.y - 1.0).abs() < 1e-5);
}

#[test]
fn camera2d_round_trip() {
    let cam = default_camera2d();
    for &(mx, my) in &[(0.0, 0.0), (3.5, -2.0), (-10.0, 10.0), (0.01, 0.01)] {
        let screen: Vec2 = Vec2::from((mx, my)).to_screen(&cam);
        let back: Vec2 = screen.to_logical(&cam);
        assert!((mx - back.x).abs() < 1e-3, "x round trip: {} -> {} -> {}", mx, screen.x, back.x);
        assert!((my - back.y).abs() < 1e-3, "y round trip: {} -> {} -> {}", my, screen.y, back.y);
    }
}

#[test]
fn camera2d_round_trip_with_offset_center() {
    let cam = Camera2D::new(
        (5.0, -3.0).into(),
        20.0,
        make_rect(0.0, 0.0, 800.0, 600.0),
    );
    for &(mx, my) in &[(5.0, -3.0), (0.0, 0.0), (12.5, 7.0)] {
        let screen: Vec2 = Vec2::from((mx, my)).to_screen(&cam);
        let back: Vec2 = screen.to_logical(&cam);
        assert!((mx - back.x).abs() < 1e-3);
        assert!((my - back.y).abs() < 1e-3);
    }
}

// ─── 长度变换测试（scale 直接换算）────────────────────────

#[test]
fn camera2d_length_to_screen() {
    let cam = default_camera2d();
    assert_eq!(1.0 * cam.scale, 40.0);
    assert_eq!(2.5 * cam.scale, 100.0);
}

#[test]
fn camera2d_screen_to_length() {
    let cam = default_camera2d();
    assert_eq!(40.0 / cam.scale, 1.0);
    assert_eq!(100.0 / cam.scale, 2.5);
}
