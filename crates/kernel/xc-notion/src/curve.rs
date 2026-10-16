// curve.rs — 参数曲线元语。
//
// 从 `xcal-scene::scene::components` 归位（M1 T4）。参数曲线是**纯数学**：
// 参数 `t` 到一个点（2D / 3D）的映射，不含任何渲染 / 采样知识。
//
// 采样（把曲线离散成折线）属**动作归属**问题：类型在理念层，采样动作在
// 场景对象层 / 装配层 —— 因此本模块只定义曲线本身，不提供 `sample()`。

use std::sync::Arc;

use crate::func::SyncSendFn;

/// 参数曲线 `t ↦ (x, y)`（默认 `f32 → (f32, f32)`），共享持有。
#[derive(Clone)]
pub struct ParameterCurveT2XY<T = f32, XY = (f32, f32)> {
    pub f: Arc<dyn SyncSendFn<T, XY>>,
}

impl<T, XY> ParameterCurveT2XY<T, XY> {
    pub fn new(f: impl SyncSendFn<T, XY> + 'static) -> Self {
        Self {
            f: Arc::new(f) as Arc<dyn SyncSendFn<T, XY>>,
        }
    }
}

/// 参数曲线 `t ↦ (x, y, z)`（默认 `(f32, f32, f32)`）。
///
/// 参数曲线采样后曾直接吐 `Vec<Vec3>`，没有 3D 曲线类型承载 —— M1 补
/// [`Path3D`](crate::path::Path3D) 作为承载类型。
#[derive(Clone)]
pub struct ParameterCurveT2XYZ<T, XYZ = (f32, f32, f32)> {
    pub f: Arc<dyn SyncSendFn<T, XYZ>>,
}

impl<T, XYZ> ParameterCurveT2XYZ<T, XYZ> {
    pub fn new(f: impl SyncSendFn<T, XYZ> + 'static) -> Self {
        Self {
            f: Arc::new(f) as Arc<dyn SyncSendFn<T, XYZ>>,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parameter_curve_2xy() {
        let c = ParameterCurveT2XY::new(|t: f32| (t.cos(), t.sin()));
        assert_eq!((c.f)(0.0), (1.0, 0.0));
    }

    #[test]
    fn test_parameter_curve_2xyz() {
        let c = ParameterCurveT2XYZ::new(|t: f32| (t, t * t, t * t * t));
        assert_eq!((c.f)(2.0), (2.0, 4.0, 8.0));
    }

    /// 统一到 `SyncSendFn` 后可作 ECS 组件（hecs 要求 `Send + Sync`）。
    #[test]
    fn test_parameter_curves_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ParameterCurveT2XY<f32, (f32, f32)>>();
        assert_send_sync::<ParameterCurveT2XYZ<f32, (f32, f32, f32)>>();
    }
}
