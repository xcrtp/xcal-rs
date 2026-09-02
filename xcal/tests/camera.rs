//! app 模块 Camera 集成测试

use egui::{Pos2, Rect};
use xcal::app::Camera;

fn make_rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
    Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
}

// ─── Camera 默认值测试 ──────────────────────────────────────

#[test]
fn camera_default_values() {
    let c = Camera::default();
    assert_eq!(c.center, Pos2::new(0.0, 0.0));
    assert_eq!(c.scale, 40.0);
}

#[test]
fn camera_can_be_cloned() {
    let c = Camera::default();
    let c2 = c.clone();
    assert_eq!(c.center, c2.center);
    assert_eq!(c.scale, c2.scale);
}

// ─── 坐标变换测试 ──────────────────────────────────────────

#[test]
fn camera_math_to_screen_origin() {
    let c = Camera::default();
    let rect = make_rect(0.0, 0.0, 800.0, 600.0);
    // center=(0,0), scale=40, rect center=(400,300)
    // math_to_screen(0,0) = (400 + 0, 300 - 0) = (400, 300)
    let p = c.math_to_screen(0.0, 0.0, rect);
    assert_eq!(p, Pos2::new(400.0, 300.0));
}

#[test]
fn camera_math_to_screen_positive() {
    let c = Camera::default();
    let rect = make_rect(0.0, 0.0, 800.0, 600.0);
    // math_to_screen(1,1) = (400+40, 300-40) = (440, 260)
    let p = c.math_to_screen(1.0, 1.0, rect);
    assert_eq!(p, Pos2::new(440.0, 260.0));
}

#[test]
fn camera_math_to_screen_negative() {
    let c = Camera::default();
    let rect = make_rect(0.0, 0.0, 800.0, 600.0);
    // math_to_screen(-1,-1) = (400-40, 300+40) = (360, 340)
    let p = c.math_to_screen(-1.0, -1.0, rect);
    assert_eq!(p, Pos2::new(360.0, 340.0));
}

#[test]
fn camera_screen_to_math_origin() {
    let c = Camera::default();
    let rect = make_rect(0.0, 0.0, 800.0, 600.0);
    // screen_to_math(400, 300) = (0 + (400-400)/40, 0 - (300-300)/40) = (0, 0)
    let (mx, my) = c.screen_to_math(400.0, 300.0, rect);
    assert!((mx).abs() < 1e-5);
    assert!((my).abs() < 1e-5);
}

#[test]
fn camera_screen_to_math_offset() {
    let c = Camera::default();
    let rect = make_rect(0.0, 0.0, 800.0, 600.0);
    // screen_to_math(440, 260) = (0 + (440-400)/40, 0 - (260-300)/40) = (1, 1)
    let (mx, my) = c.screen_to_math(440.0, 260.0, rect);
    assert!((mx - 1.0).abs() < 1e-5);
    assert!((my - 1.0).abs() < 1e-5);
}

// ─── 往返一致性测试 ────────────────────────────────────────

#[test]
fn camera_round_trip() {
    let c = Camera::default();
    let rect = make_rect(0.0, 0.0, 800.0, 600.0);
    for &(mx, my) in &[(0.0, 0.0), (3.5, -2.0), (-10.0, 10.0), (0.01, 0.01)] {
        let screen = c.math_to_screen(mx, my, rect);
        let (mx2, my2) = c.screen_to_math(screen.x, screen.y, rect);
        assert!(
            (mx - mx2).abs() < 1e-3,
            "x round trip failed: {} -> {} -> {}",
            mx,
            screen.x,
            mx2
        );
        assert!(
            (my - my2).abs() < 1e-3,
            "y round trip failed: {} -> {} -> {}",
            my,
            screen.y,
            my2
        );
    }
}

#[test]
fn camera_round_trip_with_offset_center() {
    let mut c = Camera::default();
    c.center = Pos2::new(5.0, -3.0);
    c.scale = 20.0;
    let rect = make_rect(0.0, 0.0, 800.0, 600.0);
    for &(mx, my) in &[(5.0, -3.0), (0.0, 0.0), (12.5, 7.0)] {
        let screen = c.math_to_screen(mx, my, rect);
        let (mx2, my2) = c.screen_to_math(screen.x, screen.y, rect);
        assert!((mx - mx2).abs() < 1e-3);
        assert!((my - my2).abs() < 1e-3);
    }
}

// ─── 长度变换测试 ──────────────────────────────────────────

#[test]
fn camera_length_to_screen() {
    let c = Camera::default();
    assert_eq!(c.length_to_screen(1.0), 40.0);
    assert_eq!(c.length_to_screen(2.5), 100.0);
    assert_eq!(c.length_to_screen(0.0), 0.0);
    assert_eq!(c.length_to_screen(-1.0), -40.0);
}

#[test]
fn camera_screen_to_length() {
    let c = Camera::default();
    assert_eq!(c.screen_to_length(40.0), 1.0);
    assert_eq!(c.screen_to_length(100.0), 2.5);
    assert_eq!(c.screen_to_length(0.0), 0.0);
}

#[test]
fn camera_length_round_trip() {
    let c = Camera::default();
    for &len in &[0.0, 1.0, 2.5, -3.0, 100.0] {
        let screen = c.length_to_screen(len);
        let back = c.screen_to_length(screen);
        assert!((len - back).abs() < 1e-5, "round trip failed for {}", len);
    }
}

#[test]
fn camera_length_with_custom_scale() {
    let mut c = Camera::default();
    c.scale = 100.0;
    assert_eq!(c.length_to_screen(1.0), 100.0);
    assert_eq!(c.screen_to_length(100.0), 1.0);
}

// ─── Camera 手动构造测试 ────────────────────────────────────

#[test]
fn camera_custom_construction() {
    let c = Camera {
        center: Pos2::new(10.0, 20.0),
        scale: 50.0,
    };
    assert_eq!(c.center, Pos2::new(10.0, 20.0));
    assert_eq!(c.scale, 50.0);
}

#[test]
fn camera_custom_math_to_screen() {
    let c = Camera {
        center: Pos2::new(0.0, 0.0),
        scale: 50.0,
    };
    let rect = make_rect(0.0, 0.0, 100.0, 100.0);
    // rect center = (50, 50), scale = 50
    // math_to_screen(1,0) = (50 + 50, 50 - 0) = (100, 50)
    let p = c.math_to_screen(1.0, 0.0, rect);
    assert_eq!(p, Pos2::new(100.0, 50.0));
}
