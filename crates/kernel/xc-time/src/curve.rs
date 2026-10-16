// curve.rs — 纯函数动画曲线
//
// 全部 Curve::sample 为纯函数，输入 t，输出 AnimValue。

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};
use xc_notion::{DEFAULT_MORPH_SAMPLES, Path2D};

use crate::easing::Easing;

/// 动画值类型（与具体领域解耦）。
///
/// 注意：自 [`AnimValue::Path`] 引入后本类型**不再是 `Copy`**（路径含 Vec），
/// 取值处需 `.clone()`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AnimValue {
    Scalar(f32),
    Vec2(Vec2),
    Vec3(Vec3),
    Color([f32; 4]),
    /// 变长几何：路径（形变结果 / 关键帧路径值）。
    Path(Path2D),
}

impl AnimValue {
    /// 取 Scalar 值。仅用于已确认类型的热路径：
    /// debug 构建下类型不匹配会 panic（尽早暴露 bug），
    /// release 构建下返回 0.0。类型不确定时请用 [`AnimValue::try_scalar`]。
    pub fn scalar(&self) -> f32 {
        match self {
            AnimValue::Scalar(s) => *s,
            other => {
                debug_assert!(
                    false,
                    "AnimValue::scalar called on non-Scalar variant: {other:?}"
                );
                0.0
            }
        }
    }
    /// 取 Vec3 值。仅用于已确认类型的热路径：
    /// debug 构建下类型不匹配会 panic（尽早暴露 bug），
    /// release 构建下返回 Vec3::ZERO。类型不确定时请用 [`AnimValue::try_vec3`]。
    pub fn vec3(&self) -> Vec3 {
        match self {
            AnimValue::Vec3(v) => *v,
            other => {
                debug_assert!(
                    false,
                    "AnimValue::vec3 called on non-Vec3 variant: {other:?}"
                );
                Vec3::ZERO
            }
        }
    }
    /// 类型安全取 Scalar：不匹配返回 `None`，不 panic。
    pub fn try_scalar(&self) -> Option<f32> {
        match self {
            AnimValue::Scalar(s) => Some(*s),
            _ => None,
        }
    }
    /// 类型安全取 Vec3：不匹配返回 `None`，不 panic。
    pub fn try_vec3(&self) -> Option<Vec3> {
        match self {
            AnimValue::Vec3(v) => Some(*v),
            _ => None,
        }
    }
    /// 类型安全取 Path 引用：不匹配返回 `None`，不 panic。
    /// （返回引用——`Path2D` 非 `Copy`，避免无谓克隆。）
    pub fn try_path(&self) -> Option<&Path2D> {
        match self {
            AnimValue::Path(p) => Some(p),
            _ => None,
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
    /// 兜底：无闭式解的**外部采样数据**（如导入的运动捕捉 / 数值积分结果）。
    /// 与 [`Curve::Keyframes`] 的分工见后者文档。
    Sampled {
        points: Vec<(f64, AnimValue)>,
        interpolation: Interp,
    },
    /// 任意两条曲线之间插值：`span` 内按归一化时间经 `easing` 混合两条子曲线。
    /// `span` 外取端点（clamp），不会外插。
    Morph {
        from: Box<Curve>,
        to: Box<Curve>,
        span: (f64, f64),
        easing: Easing,
    },
    /// Path → Path 形变（按弧长对齐）；输出顶点数恒为 `samples`。
    ///
    /// **状态：已实现且有单测覆盖，但当前无生产接线（就绪未接线）。**
    /// 场景侧 demo 走 `BindingTarget::MorphT` + `MorphShape`（持有 from/to、只写 `t`，
    /// 为 S3 上 GPU 预留：from/to 上传一次、每帧一个 float）；本变体是另一条用法——
    /// 「路径对**直接**插值、一次性 CPU 产出路径」，供将来按需接线。
    PathMorph {
        from: Path2D,
        to: Path2D,
        easing: Easing,
        samples: u32,
    },
    /// 关键帧：用户编排的时间→值表（帧间按 `Interp` 插值）。
    ///
    /// 与 [`Curve::Sampled`] 数据结构一致（都是采样数组），分工是**语义**：
    /// `Sampled` 承载"无闭式解的外部数据"，`Keyframes` 承载"用户手工编排"。
    /// 二者共享同一套采样逻辑（[`Curve::sample_sampled`]）；若后续确认无差别，
    /// 优先合并而非并存（S1 计划 §3.2 / §6）。
    Keyframes {
        keys: Vec<(f64, AnimValue)>,
        interpolation: Interp,
    },
}

impl Curve {
    /// 纯函数：在时间 t 处采样。
    pub fn sample(&self, t: f64) -> AnimValue {
        match self {
            Curve::Const(v) => v.clone(),
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
            Curve::Morph {
                from,
                to,
                span,
                easing,
            } => {
                let (lo, hi) = *span;
                let u = if (hi - lo).abs() < 1e-12 {
                    if t < lo { 0.0 } else { 1.0 }
                } else {
                    (((t - lo) / (hi - lo)).clamp(0.0, 1.0)) as f32
                };
                let e = easing.apply(u);
                let va = from.sample(t);
                let vb = to.sample(t);
                lerp(va.clone(), vb, e).unwrap_or(va)
            }
            Curve::PathMorph {
                from,
                to,
                easing,
                samples,
            } => {
                // 归一化到 [0,1] 后夹紧：形变不做外插（避免形状爆裂）。
                let p = easing.apply(t as f32).clamp(0.0, 1.0);
                AnimValue::Path(Path2D::morph(from, to, p, (*samples).max(1) as usize))
            }
            Curve::Keyframes {
                keys,
                interpolation,
            } => Self::sample_sampled(keys, t, *interpolation),
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
            return points[0].1.clone();
        }
        if t <= points[0].0 {
            return points[0].1.clone();
        }
        if t >= points.last().unwrap().0 {
            return points.last().unwrap().1.clone();
        }
        // 线性搜索
        for i in 0..points.len() - 1 {
            let (t0, v0) = &points[i];
            let (t1, v1) = &points[i + 1];
            if *t0 <= t && t <= *t1 {
                if interpolation == Interp::Step {
                    return v1.clone();
                }
                let alpha = (t - t0) / (t1 - t0);
                // 类型不匹配（非法数据）退化为取区间起点，行为等同 Step；
                // lerp 本身不静默 fallback。
                return lerp(v0.clone(), v1.clone(), alpha as f32).unwrap_or_else(|| v0.clone());
            }
        }
        points.last().unwrap().1.clone()
    }
}

/// 同类型 AnimValue 线性插值；类型不匹配返回 `None`（不静默 fallback）。
fn lerp(a: AnimValue, b: AnimValue, alpha: f32) -> Option<AnimValue> {
    match (a, b) {
        (AnimValue::Scalar(a), AnimValue::Scalar(b)) => {
            Some(AnimValue::Scalar(a + (b - a) * alpha))
        }
        (AnimValue::Vec2(a), AnimValue::Vec2(b)) => Some(AnimValue::Vec2(a + (b - a) * alpha)),
        (AnimValue::Vec3(a), AnimValue::Vec3(b)) => Some(AnimValue::Vec3(a + (b - a) * alpha)),
        (AnimValue::Color(a), AnimValue::Color(b)) => Some(AnimValue::Color([
            a[0] + (b[0] - a[0]) * alpha,
            a[1] + (b[1] - a[1]) * alpha,
            a[2] + (b[2] - a[2]) * alpha,
            a[3] + (b[3] - a[3]) * alpha,
        ])),
        (AnimValue::Path(a), AnimValue::Path(b)) => Some(AnimValue::Path(Path2D::morph(
            &a,
            &b,
            alpha,
            DEFAULT_MORPH_SAMPLES,
        ))),
        _ => None,
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

    #[test]
    fn try_scalar_returns_none_on_mismatch() {
        // 类型不匹配返回 None，不 panic
        assert_eq!(AnimValue::Scalar(1.5).try_scalar(), Some(1.5));
        assert_eq!(AnimValue::Vec3(Vec3::ONE).try_scalar(), None);
        assert_eq!(AnimValue::Color([0.0; 4]).try_scalar(), None);
    }

    #[test]
    fn try_vec3_returns_none_on_mismatch() {
        assert_eq!(AnimValue::Vec3(Vec3::ONE).try_vec3(), Some(Vec3::ONE));
        assert_eq!(AnimValue::Scalar(1.5).try_vec3(), None);
        assert_eq!(AnimValue::Color([0.0; 4]).try_vec3(), None);
    }

    #[test]
    fn sampled_color_curve_interpolates() {
        let curve = Curve::Sampled {
            points: vec![
                (0.0, AnimValue::Color([0.0, 0.0, 0.0, 1.0])),
                (1.0, AnimValue::Color([1.0, 0.5, 0.25, 0.0])),
            ],
            interpolation: Interp::Linear,
        };
        let mid = curve.sample(0.5);
        let AnimValue::Color(c) = mid else {
            panic!("expected Color, got {mid:?}");
        };
        assert!((c[0] - 0.5).abs() < 1e-5);
        assert!((c[1] - 0.25).abs() < 1e-5);
        assert!((c[2] - 0.125).abs() < 1e-5);
        assert!((c[3] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn sampled_mismatched_types_hold_previous_point() {
        // 相邻采样点类型不一致：lerp 返回 None，退化为取区间起点（不静默编造插值）
        let curve = Curve::Sampled {
            points: vec![
                (0.0, AnimValue::Scalar(7.0)),
                (1.0, AnimValue::Vec3(Vec3::ONE)),
            ],
            interpolation: Interp::Linear,
        };
        let mid = curve.sample(0.5);
        assert_eq!(mid, AnimValue::Scalar(7.0));
    }

    // ── I4: AnimValue::Path / try_path ────────────────────────────────

    fn square(half: f32) -> Path2D {
        Path2D::from_polyline(
            &[
                glam::Vec2::new(-half, -half),
                glam::Vec2::new(half, -half),
                glam::Vec2::new(half, half),
                glam::Vec2::new(-half, half),
            ],
            true,
        )
    }

    /// 闭合折线的"不同顶点数"（flatten 末尾会重复起点）。
    fn distinct_len(p: &Path2D) -> usize {
        let f = p.flatten(0.0);
        if f.len() >= 2 && f.first() == f.last() {
            f.len() - 1
        } else {
            f.len()
        }
    }

    #[test]
    fn try_path_returns_none_on_mismatch() {
        let p = square(1.0);
        let v = AnimValue::Path(p.clone());
        assert!(v.try_path().is_some());
        assert_eq!(AnimValue::Scalar(1.5).try_path(), None);
        assert_eq!(AnimValue::Vec3(Vec3::ONE).try_path(), None);
        // 取到的正是同一条路径
        assert_eq!(v.try_path().unwrap(), &p);
    }

    /// AnimValue::Path 参与 lerp：两端路径形变结果仍是 Path。
    #[test]
    fn sampled_path_values_morph() {
        let curve = Curve::Keyframes {
            keys: vec![
                (0.0, AnimValue::Path(square(1.0))),
                (1.0, AnimValue::Path(square(2.0))),
            ],
            interpolation: Interp::Linear,
        };
        let mid = curve.sample(0.5);
        let p = mid.try_path().expect("Path at midpoint");
        let b = p.compute_bounds().unwrap();
        // 1 与 2 的中点 → 半宽 ≈ 1.5。
        assert!((b.max.x - 1.5).abs() < 1e-3, "max.x={}", b.max.x);
        assert!((b.min.x + 1.5).abs() < 1e-3, "min.x={}", b.min.x);
    }

    // ── I5: PathMorph / Morph / Keyframes ─────────────────────────────

    #[test]
    fn path_morph_endpoints_and_stable_count() {
        let from = square(1.0);
        let to = square(2.0);
        let samples = 64u32;

        let head = Curve::PathMorph {
            from: from.clone(),
            to: to.clone(),
            easing: Easing::Linear,
            samples,
        };
        let p0 = head.sample(0.0);
        let p1 = head.sample(1.0);

        let bnd = |v: &AnimValue| v.try_path().unwrap().compute_bounds().unwrap();
        // t=0 → from（±1），t=1 → to（±2）。
        assert!((bnd(&p0).max.x - 1.0).abs() < 1e-3, "t=0 max.x={}", bnd(&p0).max.x);
        assert!((bnd(&p1).max.x - 2.0).abs() < 1e-3, "t=1 max.x={}", bnd(&p1).max.x);

        // 输出顶点数恒为 samples，与输入复杂度无关：from/to 一个 4 点、一个 512 点。
        let circle_poly: Vec<glam::Vec2> = (0..512)
            .map(|i| {
                let a = std::f32::consts::TAU * (i as f32) / 512.0;
                glam::Vec2::new(a.cos(), a.sin())
            })
            .collect();
        let curve2 = Curve::PathMorph {
            from: square(1.0),
            to: Path2D::from_polyline(&circle_poly, true),
            easing: Easing::Linear,
            samples,
        };
        assert_eq!(distinct_len(curve2.sample(0.0).try_path().unwrap()), samples as usize);
        assert_eq!(distinct_len(curve2.sample(0.37).try_path().unwrap()), samples as usize);
        assert_eq!(distinct_len(curve2.sample(1.0).try_path().unwrap()), samples as usize);
    }

    #[test]
    fn morph_blends_two_curves_within_span_and_holds_outside() {
        let curve = Curve::Morph {
            from: Box::new(Curve::Const(AnimValue::Scalar(0.0))),
            to: Box::new(Curve::Const(AnimValue::Scalar(10.0))),
            span: (2.0, 4.0),
            easing: Easing::Linear,
        };
        // span 内：中点 = 5
        assert!((curve.sample(3.0).scalar() - 5.0).abs() < 1e-5);
        // span 外：clamp 到端点（不外插）
        assert!((curve.sample(0.0).scalar() - 0.0).abs() < 1e-6);
        assert!((curve.sample(9.0).scalar() - 10.0).abs() < 1e-6);
    }

    #[test]
    fn keyframes_exact_at_keys_and_interpolated_between() {
        let curve = Curve::Keyframes {
            keys: vec![
                (0.0, AnimValue::Scalar(0.0)),
                (1.0, AnimValue::Scalar(10.0)),
                (2.0, AnimValue::Scalar(0.0)),
            ],
            interpolation: Interp::Linear,
        };
        assert_eq!(curve.sample(0.0).scalar(), 0.0);
        assert_eq!(curve.sample(1.0).scalar(), 10.0);
        assert_eq!(curve.sample(2.0).scalar(), 0.0);
        assert!((curve.sample(0.5).scalar() - 5.0).abs() < 1e-5);
        assert!((curve.sample(1.5).scalar() - 5.0).abs() < 1e-5);

        let stepped = Curve::Keyframes {
            keys: vec![
                (0.0, AnimValue::Scalar(0.0)),
                (1.0, AnimValue::Scalar(10.0)),
            ],
            interpolation: Interp::Step,
        };
        assert_eq!(stepped.sample(0.5).scalar(), 10.0);
    }

    #[test]
    fn serde_roundtrip_path_curve() {
        let curve = Curve::PathMorph {
            from: square(1.0),
            to: square(3.0),
            easing: Easing::EaseInOut,
            samples: 32,
        };
        let json = serde_json::to_string(&curve).unwrap();
        let back: Curve = serde_json::from_str(&json).unwrap();
        let a = curve.sample(0.4).try_path().unwrap().clone();
        let b = back.sample(0.4).try_path().unwrap().clone();
        assert_eq!(a, b);
    }
}
