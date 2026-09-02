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
}

type Tuple4<T> = (T, T, T, T);

export_vec!(Vec2);

impl From<(f32, f32)> for Vec2 {
    fn from((x, y): (f32, f32)) -> Self {
        glam::Vec2::new(x, y).into()
    }
}

#[cfg(feature = "egui")]
impl Into<egui::Pos2> for Vec2 {
    fn into(self) -> egui::Pos2 {
        egui::Pos2::new(self.x, self.y)
    }
}

export_vec!(Vec3);

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
#[cfg(feature = "egui")]
export_type!(
    #[derive(Clone,Copy,Debug)]
    pub Color32 (U8Vec4)
);

impl Color32 {
    fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
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
        egui::Color32::from_rgba_premultiplied(self.x, self.y, self.z, self.w).into()
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

#[cfg(test)]
mod tests {

    #[allow(unused_imports)]
    use super::*;

    #[test]
    fn test_vec() {
        let vec: Vec2 = glam::Vec2::ONE.into();
        let _: egui::Pos2 = vec.into();
    }
}
#[derive(Debug, Clone, Copy, derive_new::new)]
pub struct Rect {
    min: Vec2,
    max: Vec2,
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
