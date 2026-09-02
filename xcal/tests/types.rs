//! types 模块集成测试

use xcal::types::{Color32, I8Vec4, Rect, U8Vec4, Vec2, Vec3, Vec4};

// ─── Vec2 测试 ─────────────────────────────────────────────

#[test]
fn vec2_from_tuple() {
    let v: Vec2 = (1.0_f32, 2.0).into();
    assert_eq!(v.x, 1.0);
    assert_eq!(v.y, 2.0);
}

#[test]
fn vec2_from_glam() {
    let v: Vec2 = glam::Vec2::new(5.0, 6.0).into();
    assert_eq!(v.x, 5.0);
    assert_eq!(v.y, 6.0);
}

#[test]
fn vec2_new() {
    let v = Vec2::new(glam::Vec2::new(3.0, 4.0));
    assert_eq!(v.x, 3.0);
    assert_eq!(v.y, 4.0);
}

#[test]
fn vec2_into_egui_pos2() {
    let v: Vec2 = (10.0, 20.0).into();
    let p: egui::Pos2 = v.into();
    assert_eq!(p, egui::pos2(10.0, 20.0));
}

#[test]
fn vec2_from_egui_pos2() {
    let p = egui::pos2(7.0, 8.0);
    let v: Vec2 = p.into();
    assert_eq!(v.x, 7.0);
    assert_eq!(v.y, 8.0);
}

#[test]
fn vec2_clone_copy() {
    let a: Vec2 = (1.0, 2.0).into();
    let b = a;
    let c = a.clone();
    assert_eq!(a.x, b.x);
    assert_eq!(a.x, c.x);
}

// ─── Vec3 测试 ─────────────────────────────────────────────

#[test]
fn vec3_from_tuple() {
    let v: Vec3 = (1.0_f32, 2.0, 3.0).into();
    assert_eq!(v.x, 1.0);
    assert_eq!(v.y, 2.0);
    assert_eq!(v.z, 3.0);
}

#[test]
fn vec3_from_glam() {
    let v: Vec3 = glam::Vec3::new(1.0, 2.0, 3.0).into();
    assert_eq!(v.x, 1.0);
    assert_eq!(v.y, 2.0);
    assert_eq!(v.z, 3.0);
}

#[test]
fn vec3_new() {
    let v = Vec3::new(glam::Vec3::new(4.0, 5.0, 6.0));
    assert_eq!(v.x, 4.0);
    assert_eq!(v.y, 5.0);
    assert_eq!(v.z, 6.0);
}

// ─── Vec4 测试 ─────────────────────────────────────────────

#[test]
fn vec4_from_tuple() {
    let v: Vec4 = (1.0_f32, 2.0, 3.0, 4.0).into();
    assert_eq!(v.x, 1.0);
    assert_eq!(v.y, 2.0);
    assert_eq!(v.z, 3.0);
    assert_eq!(v.w, 4.0);
}

#[test]
fn vec4_from_glam() {
    let v: Vec4 = glam::Vec4::new(1.0, 2.0, 3.0, 4.0).into();
    assert_eq!(v.x, 1.0);
    assert_eq!(v.y, 2.0);
    assert_eq!(v.z, 3.0);
    assert_eq!(v.w, 4.0);
}

// ─── I8Vec4 测试 ────────────────────────────────────────────

#[test]
fn i8vec4_from_tuple() {
    let v: I8Vec4 = (1_i8, 2, 3, 4).into();
    assert_eq!(v.x, 1);
    assert_eq!(v.y, 2);
    assert_eq!(v.z, 3);
    assert_eq!(v.w, 4);
}

#[test]
fn i8vec4_from_glam() {
    let v: I8Vec4 = glam::I8Vec4::new(5, 6, 7, 8).into();
    assert_eq!(v.x, 5);
    assert_eq!(v.y, 6);
    assert_eq!(v.z, 7);
    assert_eq!(v.w, 8);
}

// ─── U8Vec4 测试 ────────────────────────────────────────────

#[test]
fn u8vec4_from_tuple() {
    let v: U8Vec4 = (1_u8, 2, 3, 4).into();
    assert_eq!(v.x, 1);
    assert_eq!(v.y, 2);
    assert_eq!(v.z, 3);
    assert_eq!(v.w, 4);
}

#[test]
fn u8vec4_from_glam() {
    let v: U8Vec4 = glam::U8Vec4::new(5, 6, 7, 8).into();
    assert_eq!(v.x, 5);
    assert_eq!(v.y, 6);
    assert_eq!(v.z, 7);
    assert_eq!(v.w, 8);
}

// ─── Color32 测试 ───────────────────────────────────────────

#[test]
fn color32_from_tuple() {
    let c: Color32 = (10_u8, 20, 30, 40).into();
    assert_eq!(c.r(), 10);
    assert_eq!(c.g(), 20);
    assert_eq!(c.b(), 30);
    assert_eq!(c.a(), 40);
}

#[test]
fn color32_from_egui() {
    let ec = egui::Color32::from_rgba_premultiplied(10, 20, 30, 40);
    let c: Color32 = ec.into();
    assert_eq!(c.r(), 10);
    assert_eq!(c.g(), 20);
    assert_eq!(c.b(), 30);
    assert_eq!(c.a(), 40);
}

#[test]
fn color32_into_egui() {
    let c: Color32 = (10_u8, 20, 30, 40).into();
    let ec: egui::Color32 = c.into_egui_color32();
    assert_eq!(ec.r(), 10);
    assert_eq!(ec.g(), 20);
    assert_eq!(ec.b(), 30);
    assert_eq!(ec.a(), 40);
}

#[test]
fn color32_round_trip() {
    let original = egui::Color32::from_rgba_premultiplied(100, 150, 200, 255);
    let xcal_color: Color32 = original.into();
    let back: egui::Color32 = xcal_color.into_egui_color32();
    assert_eq!(back, original);
}

// ─── Rect 测试 ──────────────────────────────────────────────

#[test]
fn rect_new() {
    let min: Vec2 = (0.0, 0.0).into();
    let max: Vec2 = (10.0, 20.0).into();
    let r = Rect::new(min, max);
    let size = r.size();
    assert_eq!(size.x, 10.0);
    assert_eq!(size.y, 20.0);
}

#[test]
fn rect_center() {
    let min: Vec2 = (0.0, 0.0).into();
    let max: Vec2 = (10.0, 20.0).into();
    let r = Rect::new(min, max);
    let center = r.center();
    assert_eq!(center.x, 5.0);
    assert_eq!(center.y, 10.0);
}

#[test]
fn rect_from_egui() {
    let er = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 20.0));
    let r: Rect = er.into();
    let size = r.size();
    assert_eq!(size.x, 10.0);
    assert_eq!(size.y, 20.0);
}

#[test]
fn rect_into_egui() {
    let min: Vec2 = (0.0, 0.0).into();
    let max: Vec2 = (10.0, 20.0).into();
    let r = Rect::new(min, max);
    let er: egui::Rect = r.into_egui_rect();
    assert_eq!(er.min, egui::pos2(0.0, 0.0));
    assert_eq!(er.max, egui::pos2(10.0, 20.0));
}

#[test]
fn rect_size_with_negative_coords() {
    let min: Vec2 = (-5.0, -10.0).into();
    let max: Vec2 = (5.0, 10.0).into();
    let r = Rect::new(min, max);
    let size = r.size();
    assert_eq!(size.x, 10.0);
    assert_eq!(size.y, 20.0);
    let center = r.center();
    assert_eq!(center.x, 0.0);
    assert_eq!(center.y, 0.0);
}
