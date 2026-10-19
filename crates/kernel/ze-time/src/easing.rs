// easing.rs — 纯函数缓动曲线 f(t) → t'（时间重映射）。
//
// 与 Curve 同属"纯函数"家族：无状态，同一 t 永远得到同一结果。
// 约定输入/输出均在 [0, 1]；区间外的行为由调用方决定（本模块不做 clamp，
// 唯一例外是 Cubic 的求解区间）。

use serde::{Deserialize, Serialize};

/// 缓动（时间重映射）曲线。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Easing {
    /// 恒等：`t`。
    Linear,
    /// 加速：`t²`。
    EaseIn,
    /// 减速：`1 - (1-t)²`。
    EaseOut,
    /// 两端平滑（smoothstep）：`t²(3 - 2t)`。
    EaseInOut,
    /// CSS 风格 `cubic-bezier(x1, y1, x2, y2)`；P0=(0,0)、P3=(1,1) 固定。
    Cubic([f32; 4]),
    /// 阶梯量化：把 [0,1] 均分为 `n` 档，取所在档的下界；`n = 0` 时退化为恒等。
    Step(usize),
}

impl Easing {
    /// 应用缓动：`f(t) → t'`。
    pub fn apply(&self, t: f32) -> f32 {
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Easing::EaseInOut => t * t * (3.0 - 2.0 * t),
            Easing::Cubic([x1, y1, x2, y2]) => cubic_bezier(*x1, *y1, *x2, *y2, t),
            Easing::Step(n) => {
                if *n == 0 {
                    return t;
                }
                let n = *n as f32;
                (t * n).floor() / n
            }
        }
    }
}

/// 求解 CSS cubic-bezier：给定横坐标 `x = t`，二分求参数 `s` 使 `bezier_x(s) = t`，
/// 返回 `bezier_y(s)`。固定 32 次迭代 → 完全确定性（纯函数要求）。
fn cubic_bezier(x1: f32, y1: f32, x2: f32, y2: f32, t: f32) -> f32 {
    if t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    let bezier = |p1: f32, p2: f32, s: f32| {
        let u = 1.0 - s;
        3.0 * u * u * s * p1 + 3.0 * u * s * s * p2 + s * s * s
    };
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    let mut s = t;
    for _ in 0..32 {
        s = 0.5 * (lo + hi);
        if bezier(x1, x2, s) < t {
            lo = s;
        } else {
            hi = s;
        }
    }
    bezier(y1, y2, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_easings() -> Vec<Easing> {
        vec![
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
            Easing::Cubic([0.25, 0.1, 0.25, 1.0]),
            Easing::Step(4),
        ]
    }

    #[test]
    fn endpoints_are_exact() {
        for e in all_easings() {
            assert!((e.apply(0.0) - 0.0).abs() < 1e-5, "{e:?} @0");
            assert!((e.apply(1.0) - 1.0).abs() < 1e-5, "{e:?} @1");
        }
    }

    #[test]
    fn midpoint_values() {
        assert!((Easing::Linear.apply(0.5) - 0.5).abs() < 1e-6);
        assert!((Easing::EaseIn.apply(0.5) - 0.25).abs() < 1e-6);
        assert!((Easing::EaseOut.apply(0.5) - 0.75).abs() < 1e-6);
        assert!((Easing::EaseInOut.apply(0.5) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn monotonic_non_decreasing() {
        for e in all_easings() {
            let mut prev = e.apply(0.0);
            for i in 1..=100 {
                let v = e.apply(i as f32 / 100.0);
                assert!(v + 1e-6 >= prev, "{e:?} not monotonic at i={i}: {prev} -> {v}");
                prev = v;
            }
        }
    }

    #[test]
    fn step_quantizes() {
        let e = Easing::Step(4);
        assert_eq!(e.apply(0.0), 0.0);
        assert_eq!(e.apply(0.24), 0.0);
        assert_eq!(e.apply(0.25), 0.25);
        assert_eq!(e.apply(0.99), 0.75);
        // n = 0 → 恒等（不除零）
        assert!((Easing::Step(0).apply(0.37) - 0.37).abs() < 1e-6);
    }

    #[test]
    fn cubic_linear_like_control_points_are_near_identity() {
        // cubic-bezier(1/3, 1/3, 2/3, 2/3) 在数学上等价于直线 y=x。
        let e = Easing::Cubic([1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0]);
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            assert!((e.apply(t) - t).abs() < 1e-4, "t={t}: {}", e.apply(t));
        }
    }

    #[test]
    fn serde_roundtrip() {
        let e = Easing::Cubic([0.1, 0.2, 0.3, 0.4]);
        let json = serde_json::to_string(&e).unwrap();
        let back: Easing = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
}
