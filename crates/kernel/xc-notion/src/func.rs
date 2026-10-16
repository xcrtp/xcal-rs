// func.rs — 函数元语。
//
// 从 `xcal-scene::scene::components` 归位（M1 T4）：`FunctionX2Y` 此前与
// `Parent` / `Transform` / `Circle` 等 ECS 组件挤在同一文件里，但它是**纯数学函数**
// ——函数与参数曲线、采样器、插值器同属理念层。
//
// 纯度约束：本模块只有 `std::sync::Arc` 与函数类型，不含 ECS / GPU / UI 概念。

use std::sync::Arc;

/// 函数抽象：要求 `Send + Sync`，使承载它的类型可作 ECS 组件（hecs 的硬性要求）。
///
/// M1 T4 修正：`ParameterCurveT2XY` / `ParameterCurveT2XYZ` 原为
/// `Arc<dyn Fn(..)>`（无 `Send + Sync`），与 `FunctionX2Y` 不一致 —— 后果是
/// 参数曲线**无法作为 ECS 组件**。现统一到本 trait。
pub trait SyncSendFn<X, Y>: Sync + Send + Fn(X) -> Y {}
impl<X, Y, T: Send + Sync + Fn(X) -> Y> SyncSendFn<X, Y> for T {}

/// 一元实函数 `y = f(x)`（默认 `f32 → f32`），共享持有。
#[derive(Clone)]
pub struct FunctionX2Y<X = f32, Y = f32> {
    pub f: Arc<dyn SyncSendFn<X, Y>>,
}

impl<X, Y> FunctionX2Y<X, Y> {
    pub fn new(f: impl SyncSendFn<X, Y> + 'static) -> Self {
        Self {
            f: Arc::new(f) as Arc<dyn SyncSendFn<X, Y>>,
        }
    }
}

/// 参数曲面函数 `z = f(x, y)` + 参数域（纯数学，M1 T5）。
///
/// 从 `xcal-scene` 的 `SurfaceSpec` 劈出：`SurfaceSpec` 原把
/// `u_steps` / `v_steps`（"这次采样多少格"）混进了数学定义。
/// `z = f(x, y)` 这个概念只需要**函数 + 域**；步数是离散化决策，留在场景层
/// （见 `xcal_scene::scene::primitives::parametric_surface` 的 `u_steps` / `v_steps` 参数）。
#[derive(Debug, Clone, Copy)]
pub struct SurfaceFn {
    /// u 方向（x）参数域 `(min, max)`。
    pub u_range: (f32, f32),
    /// v 方向（y）参数域 `(min, max)`。
    pub v_range: (f32, f32),
    /// 高度函数 `z = f(x, y)`；常数函数即平面。
    pub f: fn(f32, f32) -> f32,
}

impl SurfaceFn {
    pub fn new(u_range: (f32, f32), v_range: (f32, f32), f: fn(f32, f32) -> f32) -> Self {
        Self {
            u_range,
            v_range,
            f,
        }
    }

    /// 求值：`z = f(x, y)`。
    pub fn eval(&self, x: f32, y: f32) -> f32 {
        (self.f)(x, y)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_x2y_call() {
        let g = FunctionX2Y::new(|x: f32| x * 2.0);
        assert_eq!((g.f)(3.0), 6.0);
    }

    /// `SyncSendFn` 抽象的性质：函数可作为 ECS 组件的必要条件。
    #[test]
    fn test_function_x2y_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FunctionX2Y<f32, f32>>();
    }

    #[test]
    fn test_surface_fn_eval_and_domain() {
        let s = SurfaceFn::new((-1.0, 2.0), (0.0, 3.0), |x, y| x * y);
        assert_eq!(s.u_range, (-1.0, 2.0));
        assert_eq!(s.v_range, (0.0, 3.0));
        assert_eq!(s.eval(2.0, 3.0), 6.0);
        // 常数函数即平面。
        let flat = SurfaceFn::new((0.0, 1.0), (0.0, 1.0), |_, _| 0.5);
        assert_eq!(flat.eval(0.7, 0.2), 0.5);
    }
}
