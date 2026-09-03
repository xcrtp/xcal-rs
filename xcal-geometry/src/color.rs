// color.rs
//
// 32 位 RGBA 颜色（纯数据，sRGB 分量，u8）。

use crate::vec::Vec4;

/// 32 位颜色：r/g/b/a 各 8 位。
///
/// 与后端无关的纯数据类型；后端（egui/wgpu）的转换由各后端 crate 负责。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Color32 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color32 {
    pub const TRANSPARENT: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 0,
    };
    pub const BLACK: Self = Self {
        r: 0,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const WHITE: Self = Self {
        r: 255,
        g: 255,
        b: 255,
        a: 255,
    };
    pub const RED: Self = Self {
        r: 255,
        g: 0,
        b: 0,
        a: 255,
    };
    pub const GREEN: Self = Self {
        r: 0,
        g: 255,
        b: 0,
        a: 255,
    };
    pub const BLUE: Self = Self {
        r: 0,
        g: 0,
        b: 255,
        a: 255,
    };

    pub const fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    /// 等价于 [`Color32::from_rgba`]（与项目历史 API 兼容的构造名）。
    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }

    pub const fn r(&self) -> u8 {
        self.r
    }
    pub const fn g(&self) -> u8 {
        self.g
    }
    pub const fn b(&self) -> u8 {
        self.b
    }
    pub const fn a(&self) -> u8 {
        self.a
    }

    /// 从 0xRRGGBB 不透明颜色构造（alpha = 255）。
    pub const fn from_rgb(hex: u32) -> Self {
        Self {
            r: ((hex >> 16) & 0xFF) as u8,
            g: ((hex >> 8) & 0xFF) as u8,
            b: (hex & 0xFF) as u8,
            a: 255,
        }
    }

    /// 从 0xRRGGBBAA 打包整数构造。
    pub const fn from_hex(hex: u32) -> Self {
        Self {
            r: ((hex >> 24) & 0xFF) as u8,
            g: ((hex >> 16) & 0xFF) as u8,
            b: ((hex >> 8) & 0xFF) as u8,
            a: (hex & 0xFF) as u8,
        }
    }

    /// 转为 [r, g, b, a] 数组。
    pub const fn to_array(self) -> [u8; 4] {
        [self.r, self.g, self.b, self.a]
    }

    /// 转为归一化浮点 RGBA（0.0..=1.0，线性分量值，不做 gamma 处理）。
    pub fn to_vec4(self) -> Vec4 {
        Vec4::new(
            self.r as f32 / 255.0,
            self.g as f32 / 255.0,
            self.b as f32 / 255.0,
            self.a as f32 / 255.0,
        )
    }
}

impl From<(u8, u8, u8, u8)> for Color32 {
    fn from((r, g, b, a): (u8, u8, u8, u8)) -> Self {
        Self::from_rgba(r, g, b, a)
    }
}

impl From<[u8; 4]> for Color32 {
    fn from([r, g, b, a]: [u8; 4]) -> Self {
        Self::from_rgba(r, g, b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_rgba_and_consts() {
        let c = Color32::from_rgba(1, 2, 3, 4);
        assert_eq!(c.to_array(), [1, 2, 3, 4]);
        assert_eq!(Color32::TRANSPARENT.to_array(), [0, 0, 0, 0]);
        assert_eq!(Color32::BLACK.to_array(), [0, 0, 0, 255]);
        assert_eq!(Color32::WHITE.to_array(), [255, 255, 255, 255]);
    }

    #[test]
    fn test_from_rgb_and_hex() {
        assert_eq!(
            Color32::from_rgb(0x112233).to_array(),
            [0x11, 0x22, 0x33, 255]
        );
        assert_eq!(
            Color32::from_hex(0x11223344).to_array(),
            [0x11, 0x22, 0x33, 0x44]
        );
    }

    #[test]
    fn test_from_tuple_and_array() {
        let a: Color32 = (10u8, 20, 30, 40).into();
        let b: Color32 = [10u8, 20, 30, 40].into();
        assert_eq!(a, b);
        assert_eq!(a.to_array(), [10, 20, 30, 40]);
    }

    #[test]
    fn test_to_vec4() {
        let v = Color32::WHITE.to_vec4();
        assert!((v.x - 1.0).abs() < 1e-6);
        assert!((v.w - 1.0).abs() < 1e-6);
        let v = Color32::RED.to_vec4();
        assert!((v.y - 0.0).abs() < 1e-6);
    }
}
