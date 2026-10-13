// lerp.rs — 线性插值原语（纯数据层）。
//
// 纯函数、无 trait object、无状态：给定 a / b / t 返回 a + (b - a) * t。
// 单位区间外的 t 会外插（不 clamp）——区间语义由调用方负责。

use crate::color::Color32;
use crate::path::{DEFAULT_MORPH_SAMPLES, Path2D};
use crate::vec::{Vec2, Vec3};

/// 线性插值：`t = 0 → a`，`t = 1 → b`。
///
/// 泛型关联函数（`Lerp::lerp(a, b, t)`）而非方法，允许对 `f32` 等
/// 外部类型（glam / 本 crate）统一实现，且不需要 `&self`。
pub trait Lerp: Sized {
    fn lerp(a: &Self, b: &Self, t: f32) -> Self;
}

impl Lerp for f32 {
    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

impl Lerp for Vec2 {
    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        *a + (*b - *a) * t
    }
}

impl Lerp for Vec3 {
    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        *a + (*b - *a) * t
    }
}

impl Lerp for Color32 {
    /// 各通道在 f32 域插值后四舍五入回 u8（线性 RGBA 空间，非感知均匀；
    /// gamma 校正留待后续，见 S1 计划已知限制）。
    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        Color32::from_rgba(
            lerp_u8(a.r, b.r, t),
            lerp_u8(a.g, b.g, t),
            lerp_u8(a.b, b.b, t),
            lerp_u8(a.a, b.a, t),
        )
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).round().clamp(0.0, 255.0) as u8
}

impl Lerp for Path2D {
    /// 路径形变：按弧长对齐后逐点插值，输出 [`DEFAULT_MORPH_SAMPLES`] 个顶点的闭合折线。
    /// 需要自定义点数时直接调 [`Path2D::morph`]。
    fn lerp(a: &Self, b: &Self, t: f32) -> Self {
        Path2D::morph(a, b, t, DEFAULT_MORPH_SAMPLES)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 注意：glam 的 Vec2/Vec3 自带固有方法 `lerp(self, rhs, s)`，
    // 其优先级高于 trait 方法，故测试统一用 UFCS 显式指定 trait 实现。

    #[test]
    fn scalar_endpoints_and_midpoint() {
        assert_eq!(<f32 as Lerp>::lerp(&2.0, &6.0, 0.0), 2.0);
        assert_eq!(<f32 as Lerp>::lerp(&2.0, &6.0, 1.0), 6.0);
        assert!((<f32 as Lerp>::lerp(&2.0, &6.0, 0.5) - 4.0).abs() < 1e-6);
    }

    #[test]
    fn vec2_endpoints_and_midpoint() {
        let a = Vec2::new(0.0, 0.0);
        let b = Vec2::new(4.0, 8.0);
        assert_eq!(<Vec2 as Lerp>::lerp(&a, &b, 0.0), a);
        assert_eq!(<Vec2 as Lerp>::lerp(&a, &b, 1.0), b);
        assert_eq!(<Vec2 as Lerp>::lerp(&a, &b, 0.5), Vec2::new(2.0, 4.0));
    }

    #[test]
    fn vec3_endpoints_and_midpoint() {
        let a = Vec3::new(-1.0, 2.0, 0.0);
        let b = Vec3::new(3.0, 2.0, 10.0);
        assert_eq!(<Vec3 as Lerp>::lerp(&a, &b, 0.0), a);
        assert_eq!(<Vec3 as Lerp>::lerp(&a, &b, 1.0), b);
        assert_eq!(<Vec3 as Lerp>::lerp(&a, &b, 0.5), Vec3::new(1.0, 2.0, 5.0));
    }

    #[test]
    fn color_endpoints_and_midpoint() {
        let a = Color32::from_rgba(0, 100, 200, 0);
        let b = Color32::from_rgba(200, 0, 0, 255);
        assert_eq!(<Color32 as Lerp>::lerp(&a, &b, 0.0), a);
        assert_eq!(<Color32 as Lerp>::lerp(&a, &b, 1.0), b);
        // 中点：各通道四舍五入。
        assert_eq!(
            <Color32 as Lerp>::lerp(&a, &b, 0.5),
            Color32::from_rgba(100, 50, 100, 128)
        );
    }

    #[test]
    fn extrapolation_is_not_clamped() {
        // 区间外线性外插（不做 clamp）。
        assert!((<f32 as Lerp>::lerp(&0.0, &10.0, 1.5) - 15.0).abs() < 1e-6);
        assert!((<f32 as Lerp>::lerp(&0.0, &10.0, -0.5) - (-5.0)).abs() < 1e-6);
    }
}
