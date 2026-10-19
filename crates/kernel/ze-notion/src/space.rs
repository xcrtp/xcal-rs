// space.rs — 呈现层度量空间。
//
// 判断标准：这个度量是**被观测对象的属性**，还是**观测工具的属性**？
//   - 被观测对象的属性（如"有数学含义的粗线段"）→ `World`：应随视图缩放。
//   - 观测工具的属性（如网格线粗细）→ `Screen`：不随缩放变化。
//
// 同一个对象的不同属性可属不同空间：向量箭头的**长度**是世界，**粗细**是屏幕。
//
// 本模块只做"声明 → 屏幕像素"的解析；**唯一换算点**在 zela-render 的
// `record_scene`（拿到 `camera.scale` 的那一点），其它地方不得乘除 scale。

/// 度量所属的空间（呈现层概念）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MeasureSpace {
    /// 世界单位：随视图缩放变化。用于有数学含义的度量。
    World,
    /// 屏幕像素：不随缩放变化。用于纯视觉呈现（**默认**）。
    Screen,
    /// 世界单位，但不小于 `min` 像素——防止缩小时彻底消失。
    WorldWithMinPx(f32),
}

impl Default for MeasureSpace {
    fn default() -> Self {
        Self::Screen
    }
}

/// 把声明式宽度解析为**屏幕像素**（唯一换算入口）。
///
/// `scale` = 每世界单位对应的屏幕像素数（即 `Camera2D.scale`）。
/// `scale = 0` 时 `World` 解析为 0（有限，不产生 NaN）。
pub fn resolve_width(width: f32, space: MeasureSpace, scale: f32) -> f32 {
    match space {
        MeasureSpace::World => width * scale,
        MeasureSpace::Screen => width,
        MeasureSpace::WorldWithMinPx(min) => (width * scale).max(min),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Screen` 的缩放不变性——本任务的核心承诺。
    #[test]
    fn screen_is_scale_invariant() {
        for scale in [0.0, 0.5, 1.0, 40.0, 1e6] {
            assert_eq!(resolve_width(2.5, MeasureSpace::Screen, scale), 2.5);
        }
    }

    /// `World` 随 scale 线性变化。
    #[test]
    fn world_scales_linearly() {
        for scale in [0.0, 1.0, 10.0, 40.0] {
            let expected = 2.0 * scale;
            assert!((resolve_width(2.0, MeasureSpace::World, scale) - expected).abs() < 1e-4);
        }
        // 线性：scale 翻倍 → 结果翻倍
        assert_eq!(
            resolve_width(3.0, MeasureSpace::World, 20.0),
            2.0 * resolve_width(3.0, MeasureSpace::World, 10.0)
        );
    }

    /// `WorldWithMinPx`：大 scale 等于 World，小 scale 钳到下限。
    #[test]
    fn world_with_min_px_clamps_low() {
        let space = MeasureSpace::WorldWithMinPx(3.0);
        // 大 scale：世界值已超下限 → 等于 World
        assert!((resolve_width(2.0, space, 10.0) - 20.0).abs() < 1e-4);
        // 小 scale：世界值低于下限 → 钳到 3.0
        assert!((resolve_width(2.0, space, 1.0) - 3.0).abs() < 1e-4);
        // 边界：世界值恰等于下限
        assert!((resolve_width(1.5, space, 2.0) - 3.0).abs() < 1e-4);
    }

    /// scale = 0 不产生 NaN（含退化宽度）。
    #[test]
    fn zero_scale_does_not_produce_nan() {
        for space in [
            MeasureSpace::World,
            MeasureSpace::Screen,
            MeasureSpace::WorldWithMinPx(1.0),
        ] {
            assert!(resolve_width(5.0, space, 0.0).is_finite(), "{space:?}");
        }
        assert_eq!(resolve_width(5.0, MeasureSpace::World, 0.0), 0.0);
        assert_eq!(resolve_width(0.0, MeasureSpace::Screen, 0.0), 0.0);
    }

    /// 默认空间是 `Screen`（保证既有行为不变的前提）。
    #[test]
    fn default_is_screen() {
        assert_eq!(MeasureSpace::default(), MeasureSpace::Screen);
    }
}
