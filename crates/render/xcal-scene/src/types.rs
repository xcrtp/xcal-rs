// types.rs
//
// 数学包装类型：Vec2, Vec3, Vec4, I8Vec4, U8Vec4, Color32, Rect, Camera2D。

macro_rules! export_type {
    ($(#[$m:meta])? $v:vis $name:ident($ty:ty)) => {
        #[derive(derive_more::From, derive_more::Deref, derive_more::DerefMut)]
        $(#[$m])?
        $v struct $name($ty);
    };
}
macro_rules! export_vec {
    ($id:ident) => {
        export_type! {
            #[derive(Clone,Copy,derive_new::new,Debug)]
           pub $id (glam::$id)
        }
    };
    ($id:ident, $($extra:meta),*) => {
        export_type! {
            #[derive(Clone,Copy,derive_new::new,Debug, $($extra),*)]
           pub $id (glam::$id)
        }
    };
}

export_vec!(Vec2, xc_macro::Transform2DInto);

impl From<(f32, f32)> for Vec2 {
    fn from((x, y): (f32, f32)) -> Self {
        glam::Vec2::new(x, y).into()
    }
}

export_vec!(Vec3, xc_macro::Transform2DInto);

impl From<(f32, f32, f32)> for Vec3 {
    fn from((x, y, z): (f32, f32, f32)) -> Self {
        glam::Vec3::new(x, y, z).into()
    }
}

export_vec!(Vec4);

impl From<(f32, f32, f32, f32)> for Vec4 {
    fn from((x, y, z, w): (f32, f32, f32, f32)) -> Self {
        glam::Vec4::new(x, y, z, w).into()
    }
}
export_vec!(I8Vec4);

impl From<(i8, i8, i8, i8)> for I8Vec4 {
    fn from((x, y, z, w): (i8, i8, i8, i8)) -> Self {
        glam::I8Vec4::new(x, y, z, w).into()
    }
}

export_vec!(U8Vec4);
impl From<(u8, u8, u8, u8)> for U8Vec4 {
    fn from((x, y, z, w): (u8, u8, u8, u8)) -> Self {
        glam::U8Vec4::new(x, y, z, w).into()
    }
}

// Color32 的规范定义位于 xcal-geometry（纯数据层），此处再导出，使 ECS 组件、
// 2D 变换体系与渲染层共用同一类型。egui/wgpu 等后端转换由各后端 crate 负责
// （孤儿规则：xcal-scene 不能为外部类型实现外部 trait）。
pub use xc_geometry::Color32;

#[derive(Debug, Clone, Copy, derive_new::new, xc_macro::Transform2DInto)]
pub struct Rect {
    min: Vec2,
    max: Vec2,
}

// 公开访问器以便 Rect 字段在私有情况下仍可使用
impl Rect {
    pub fn min(&self) -> Vec2 {
        self.min
    }
    pub fn max(&self) -> Vec2 {
        self.max
    }
}

impl Rect {
    pub fn size(&self) -> Vec2 {
        (*self.max - *self.min).abs().into()
    }
    pub fn center(&self) -> Vec2 {
        ((*self.max + *self.min) / 2.0).into()
    }
}

/// 2D 相机参数：用于在屏幕坐标系和逻辑坐标系之间转换。
///
/// - 屏幕坐标系：以视口左上角为原点，y 向下（Y-down），单位为像素。
/// - 逻辑坐标系：以 `center` 为相机中心，y 向上（Y-up，数学惯例），单位为世界单位。
///
/// `scale` 方向约定（易踩坑）：**1 世界单位 = `scale` 像素**，scale 越大
/// 物体越大；这与未来统一相机的正交投影 `extent`（垂直半高，越大看到越多、
/// 物体**越小**）方向**相反**——薄壳层（Camera2D = Camera3D + Orthographic）
/// 负责两者换算。屏幕 Y-down ↔ 世界 Y-up 的翻转只在 Transform2D 的
/// glam::Vec2 实现中存在一份，本类型不另造换算。
#[derive(Debug, Clone, Copy, derive_new::new)]
pub struct Camera2D {
    /// 逻辑坐标系中相机中心位置
    pub center: Vec2,
    /// 缩放系数：1 世界单位 = `scale` 像素（越大物体越大）
    pub scale: f32,
    /// 屏幕视口矩形（像素）
    pub rect: Rect,
}

impl Camera2D {
    /// 屏幕坐标 → 场景（逻辑）坐标。UI 适配器把原生指针事件换算为
    /// 场景语义事件时使用。
    pub fn screen_to_world(&self, p: Vec2) -> Vec2 {
        use crate::transform::Transform2D;
        p.to_logical(self)
    }

    /// 场景（逻辑）坐标 → 屏幕坐标。
    pub fn world_to_screen(&self, p: Vec2) -> Vec2 {
        use crate::transform::Transform2D;
        p.to_screen(self)
    }
}

#[cfg(test)]
mod tests {

    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_vec2_from_tuple() {
        let v: Vec2 = (1.0_f32, 2.0).into();
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);
    }

    #[test]
    fn test_vec3_from_tuple() {
        let v: Vec3 = (1.0_f32, 2.0, 3.0).into();
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);
        assert_eq!(v.z, 3.0);
    }

    #[test]
    fn test_vec4_from_tuple() {
        let v: Vec4 = (1.0_f32, 2.0, 3.0, 4.0).into();
        assert_eq!(v.x, 1.0);
        assert_eq!(v.y, 2.0);
        assert_eq!(v.z, 3.0);
        assert_eq!(v.w, 4.0);
    }

    #[test]
    fn test_i8vec4_from_tuple() {
        let v: I8Vec4 = (1_i8, 2, 3, 4).into();
        assert_eq!(v.x, 1);
        assert_eq!(v.y, 2);
        assert_eq!(v.z, 3);
        assert_eq!(v.w, 4);
    }

    #[test]
    fn test_u8vec4_from_tuple() {
        let v: U8Vec4 = (1_u8, 2, 3, 4).into();
        assert_eq!(v.x, 1);
        assert_eq!(v.y, 2);
        assert_eq!(v.z, 3);
        assert_eq!(v.w, 4);
    }

    #[test]
    fn test_color32_from_tuple() {
        let c: Color32 = (10_u8, 20, 30, 40).into();
        assert_eq!(c.r(), 10);
        assert_eq!(c.g(), 20);
        assert_eq!(c.b(), 30);
        assert_eq!(c.a(), 40);
        assert_eq!(c.r, 10);
    }

    #[test]
    fn test_rect_size() {
        let min: Vec2 = (0.0, 0.0).into();
        let max: Vec2 = (10.0, 20.0).into();
        let r = Rect::new(min, max);
        let size = r.size();
        assert_eq!(size.x, 10.0);
        assert_eq!(size.y, 20.0);
    }

    #[test]
    fn test_rect_center() {
        let min: Vec2 = (0.0, 0.0).into();
        let max: Vec2 = (10.0, 20.0).into();
        let r = Rect::new(min, max);
        let center = r.center();
        assert_eq!(center.x, 5.0);
        assert_eq!(center.y, 10.0);
    }

    #[test]
    fn test_rect_size_negative_coords() {
        let min: Vec2 = (-5.0, -10.0).into();
        let max: Vec2 = (5.0, 10.0).into();
        let r = Rect::new(min, max);
        let size = r.size();
        assert_eq!(size.x, 10.0);
        assert_eq!(size.y, 20.0);
    }

    /// 坐标换算往返：screen_to_world(to_screen(p)) == p（误差 < 1e-5）。
    #[test]
    fn test_camera2d_screen_world_roundtrip() {
        let camera = Camera2D::new(
            (-3.5, 7.25).into(),
            12.0,
            Rect::new((0.0, 0.0).into(), (800.0, 600.0).into()),
        );

        for wp in [
            (0.0, 0.0).into(),
            (-100.0, 42.0).into(),
            (3.7, -9.9).into(),
        ] {
            let sp = camera.world_to_screen(wp);
            let back = camera.screen_to_world(sp);
            assert!(
                (back.x - wp.x).abs() < 1e-5 && (back.y - wp.y).abs() < 1e-5,
                "world roundtrip failed: {wp:?} -> {sp:?} -> {back:?}"
            );
        }

        for sp in [
            (10.0, 590.0).into(),
            (400.0, 300.0).into(),
            (795.0, 3.0).into(),
        ] {
            let wp = camera.screen_to_world(sp);
            let back = camera.world_to_screen(wp);
            assert!(
                (back.x - sp.x).abs() < 1e-5 && (back.y - sp.y).abs() < 1e-5,
                "screen roundtrip failed: {sp:?} -> {wp:?} -> {back:?}"
            );
        }
    }
}
