// types.rs
//
// 数学包装类型：Vec2, Vec3, Vec4, I8Vec4, U8Vec4, Color32, Rect, Camera2D。
// egui 相关 impl 由 feature "egui" 门控，使 xcal-core 可在无 egui 时编译。

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

type Tuple4<T> = (T, T, T, T);

export_vec!(Vec2, xcal_macro::Transform2DInto);

impl From<(f32, f32)> for Vec2 {
    fn from((x, y): (f32, f32)) -> Self {
        glam::Vec2::new(x, y).into()
    }
}

#[cfg(feature = "egui")]
impl From<Vec2> for egui::Pos2 {
    fn from(val: Vec2) -> Self {
        egui::Pos2::new(val.x, val.y)
    }
}

export_vec!(Vec3, xcal_macro::Transform2DInto);

impl From<(f32, f32, f32)> for Vec3 {
    fn from((x, y, z): (f32, f32, f32)) -> Self {
        glam::Vec3::new(x, y, z).into()
    }
}
#[cfg(feature = "egui")]
impl From<egui::Pos2> for Vec2 {
    fn from(value: egui::Pos2) -> Self {
        glam::Vec2::new(value.x, value.y).into()
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

// Color32 总是存在（即使无 egui），以便 xcal-render 中的 StrokeSolid/FillSolid
// 能在不启用 egui feature 的情况下使用。
// 仅 egui 相关转换由 feature "egui" 门控。
export_type!(
    #[derive(Clone,Copy,Debug)]
    pub Color32 (U8Vec4)
);

impl Color32 {
    pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self(glam::U8Vec4::new(r, g, b, a).into())
    }

    pub fn r(&self) -> u8 {
        self.x
    }

    pub fn g(&self) -> u8 {
        self.y
    }

    pub fn b(&self) -> u8 {
        self.z
    }

    pub fn a(&self) -> u8 {
        self.w
    }

    #[cfg(feature = "egui")]
    pub fn into_egui_color32(self) -> egui::Color32 {
        egui::Color32::from_rgba_premultiplied(self.x, self.y, self.z, self.w)
    }
}

#[cfg(feature = "egui")]
impl From<egui::Color32> for Color32 {
    fn from(value: egui::Color32) -> Self {
        Self::new(value.r(), value.g(), value.b(), value.a())
    }
}
impl From<Tuple4<u8>> for Color32 {
    fn from((r, g, b, a): Tuple4<u8>) -> Self {
        Self::new(r, g, b, a)
    }
}

#[derive(Debug, Clone, Copy, derive_new::new, xcal_macro::Transform2DInto)]
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

#[cfg(feature = "egui")]
impl From<egui::Rect> for Rect {
    fn from(value: egui::Rect) -> Self {
        Self {
            min: value.min.into(),
            max: value.max.into(),
        }
    }
}
#[cfg(feature = "egui")]
impl Rect {
    pub fn into_egui_rect(self) -> egui::Rect {
        egui::Rect {
            min: self.min.into(),
            max: self.max.into(),
        }
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
/// - 屏幕坐标系：以视口左上角为原点，y 向下，单位为像素。
/// - 逻辑坐标系：以 `center` 为相机中心，y 向上，单位为世界单位。
#[derive(Debug, Clone, Copy, derive_new::new)]
pub struct Camera2D {
    /// 逻辑坐标系中相机中心位置
    pub center: Vec2,
    /// 缩放系数：1 单位逻辑 = `scale` 像素
    pub scale: f32,
    /// 屏幕视口矩形（像素）
    pub rect: Rect,
}

#[cfg(test)]
mod tests {

    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_vec() {
        let vec: Vec2 = glam::Vec2::ONE.into();
        let _: egui::Pos2 = vec.into();
    }

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
    }

    #[test]
    fn test_color32_from_egui() {
        let ec = egui::Color32::from_rgba_premultiplied(10, 20, 30, 40);
        let c: Color32 = ec.into();
        assert_eq!(c.r(), 10);
        assert_eq!(c.g(), 20);
        assert_eq!(c.b(), 30);
        assert_eq!(c.a(), 40);
    }

    #[test]
    fn test_color32_round_trip() {
        let original = egui::Color32::from_rgba_premultiplied(100, 150, 200, 255);
        let xcal_color: Color32 = original.into();
        let back: egui::Color32 = xcal_color.into_egui_color32();
        assert_eq!(back, original);
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

    #[test]
    fn test_rect_from_egui() {
        let er = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 20.0));
        let r: Rect = er.into();
        let size = r.size();
        assert_eq!(size.x, 10.0);
        assert_eq!(size.y, 20.0);
    }

    #[test]
    fn test_rect_into_egui() {
        let min: Vec2 = (0.0, 0.0).into();
        let max: Vec2 = (10.0, 20.0).into();
        let r = Rect::new(min, max);
        let er: egui::Rect = r.into_egui_rect();
        assert_eq!(er.min, egui::pos2(0.0, 0.0));
        assert_eq!(er.max, egui::pos2(10.0, 20.0));
    }
}
