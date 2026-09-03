// curve.rs — 纯函数动画曲线
//
// 全部 Curve::sample 为纯函数，输入 t，输出 AnimValue。

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// 动画值类型（与具体领域解耦，未来可扩展 Quaternion / Mat4）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AnimValue {
    Scalar(f32),
    Vec2(Vec2),
    Vec3(Vec3),
    Color([f32; 4]),
}

impl AnimValue {
    pub fn scalar(&self) -> f32 {
        match self {
            AnimValue::Scalar(s) => *s,
            _ => panic!("AnimValue::scalar called on non-Scalar variant"),
        }
    }
    pub fn vec3(&self) -> Vec3 {
        match self {
            AnimValue::Vec3(v) => *v,
            _ => panic!("AnimValue::vec3 called on non-Vec3 variant"),
        }
    }
}

/// 采样点插值方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interp {
    Linear,
    Step,
}

/// 动画曲线（纯函数）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Curve {
    Const(AnimValue),
    /// 标量 sin 曲线：base + amp * sin(t * omega)
    Sin {
        base: f32,
        amp: f32,
        omega: f32,
    },
    /// 3D 轨道相机偏移：eye = radius * rotate(axis, speed * t) * initial_angle
    Orbit {
        radius: f32,
        axis: Vec3,
        speed: f32,
        initial_angle: f32,
    },
    /// 相位（线性 f(t) = t * omega）—— 供 FunctionX2Y 水平滑动
    Phase {
        omega: f32,
    },
    /// 兜底：无闭式解的采样点数组
    Sampled {
        points: Vec<(f64, AnimValue)>,
        interpolation: Interp,
    },
}

impl Curve {
    /// 纯函数：在时间 t 处采样。
    pub fn sample(&self, t: f64) -> AnimValue {
        match self {
            Curve::Const(v) => *v,
            Curve::Sin { base, amp, omega } => {
                let s = *base + *amp * ((*omega as f64) * t).sin() as f32;
                AnimValue::Scalar(s)
            }
            Curve::Orbit {
                radius,
                axis,
                speed,
                initial_angle,
            } => {
                let angle = (*speed as f64) * t + (*initial_angle as f64);
                let axis = axis.normalize_or_zero();
                // Rodrigues 旋转轴角 → 旋转初始 Vec3::new(0, 0, *radius)
                // （与旧 OrbitAnim 一致：t=0 时偏移在 +Z，eye = target + (0,0,radius)）
                let cos_a = angle.cos() as f32;
                let sin_a = angle.sin() as f32;
                let base = Vec3::new(0.0, 0.0, *radius);
                let rotated = base * cos_a
                    + axis.cross(base) * sin_a
                    + axis * axis.dot(base) * (1.0 - cos_a);
                AnimValue::Vec3(rotated)
            }
            Curve::Phase { omega } => {
                AnimValue::Scalar((*omega as f32) * t as f32)
            }
            Curve::Sampled {
                points,
                interpolation,
            } => Self::sample_sampled(points, t, *interpolation),
        }
    }

    fn sample_sampled(
        points: &[(f64, AnimValue)],
        t: f64,
        interpolation: Interp,
    ) -> AnimValue {
        if points.is_empty() {
            return AnimValue::Scalar(0.0);
        }
        if points.len() == 1 {
            return points[0].1;
        }
        if t <= points[0].0 {
            return points[0].1;
        }
        if t >= points.last().unwrap().0 {
            return points.last().unwrap().1;
        }
        // 线性搜索
        for i in 0..points.len() - 1 {
            let (t0, v0) = points[i];
            let (t1, v1) = points[i + 1];
            if t0 <= t && t <= t1 {
                if interpolation == Interp::Step {
                    return v1;
                }
                let alpha = (t - t0) / (t1 - t0);
                return lerp(v0, v1, alpha as f32);
            }
        }
        points.last().unwrap().1
    }
}

fn lerp(a: AnimValue, b: AnimValue, alpha: f32) -> AnimValue {
    match (a, b) {
        (AnimValue::Scalar(a), AnimValue::Scalar(b)) => AnimValue::Scalar(a + (b - a) * alpha),
        (AnimValue::Vec2(a), AnimValue::Vec2(b)) => AnimValue::Vec2(a + (b - a) * alpha),
        (AnimValue::Vec3(a), AnimValue::Vec3(b)) => AnimValue::Vec3(a + (b - a) * alpha),
        _ => a, // fallback: return first value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sin_curve_formula() {
        let curve = Curve::Sin {
            base: 1.0,
            amp: 0.4,
            omega: 2.0,
        };
        for i in 0..1000 {
            let t = (i as f64) * 0.01;
            let expected = 1.0 + 0.4 * (2.0_f64 * t).sin() as f32;
            let actual = curve.sample(t).scalar();
            assert!((actual - expected).abs() < 1e-5, "t={t}: {actual} vs {expected}");
        }
    }

    #[test]
    fn const_curve_always_same() {
        let curve = Curve::Const(AnimValue::Scalar(42.0));
        for t in [0.0, -100.0, 1e6] {
            assert_eq!(curve.sample(t).scalar(), 42.0);
        }
    }

    #[test]
    fn orbit_curve_samples_vec3() {
        let curve = Curve::Orbit {
            radius: 5.0,
            axis: Vec3::Y,
            speed: 1.0,
            initial_angle: 0.0,
        };
        // t=0 → (0, 0, 5)（与旧 OrbitAnim 起点一致）
        let v0 = curve.sample(0.0).vec3();
        assert!((v0 - Vec3::new(0.0, 0.0, 5.0)).length() < 1e-5);
        // t=π/2 → (5, 0, 0)（右手坐标绕 Y 轴 +π/2 旋转：z→x）
        let vq = curve.sample(std::f64::consts::PI / 2.0).vec3();
        assert!((vq - Vec3::new(5.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn phase_curve_is_linear() {
        let curve = Curve::Phase { omega: 1.5 };
        for t in [0.0, 1.0, 2.5, -3.0] {
            assert!((curve.sample(t).scalar() - (1.5 * t as f32)).abs() < 1e-5);
        }
    }

    #[test]
    fn sampled_curve_lerps() {
        let curve = Curve::Sampled {
            points: vec![
                (0.0, AnimValue::Scalar(0.0)),
                (1.0, AnimValue::Scalar(10.0)),
                (2.0, AnimValue::Scalar(0.0)),
            ],
            interpolation: Interp::Linear,
        };
        assert!((curve.sample(0.5).scalar() - 5.0).abs() < 1e-5);
        assert!((curve.sample(1.5).scalar() - 5.0).abs() < 1e-5);
        assert_eq!(curve.sample(0.0).scalar(), 0.0);
        assert_eq!(curve.sample(2.0).scalar(), 0.0);
    }

    #[test]
    fn serde_roundtrip_curve() {
        let curve = Curve::Sin {
            base: 0.0,
            amp: 1.0,
            omega: 3.0,
        };
        let json = serde_json::to_string(&curve).unwrap();
        let restored: Curve = serde_json::from_str(&json).unwrap();
        for t in [0.0, 1.0, -1.5, 5.2] {
            assert_eq!(curve.sample(t), restored.sample(t));
        }
    }
}
