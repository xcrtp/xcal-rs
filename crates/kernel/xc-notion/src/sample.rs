// sample.rs — 采样规格与插值器元语。
//
// 从 `xcal-scene::scene::components` 归位（M1 T4）。此前它们与 ECS 组件同处一文件，
// 但内容是**纯数学的采样参数**（区间 + 步长）与**插值函数包装**。
//
// 采样规格 vs 采样动作：规格（区间 / 步长）是理念层数据；真正的采样动作由
// 场景对象层 / 装配层执行（同 `curve.rs` 的判据）。

use std::sync::Arc;

use crate::func::DynFn;

/// 一维采样规格：区间 `[start, end]` + 步长 `step`。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct Sampler1D<T = f32> {
    pub start: T,
    pub end: T,
    pub step: T,
}

impl<T> Sampler1D<T> {
    pub fn new(start: T, end: T, step: T) -> Self {
        Self { start, end, step }
    }
}

/// 二维采样规格：u / v 两个方向的区间与步长。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct Sampler2D<T = f32> {
    pub u_start: T,
    pub u_end: T,
    pub u_step: T,
    pub v_start: T,
    pub v_end: T,
    pub v_step: T,
}

impl<T> Sampler2D<T> {
    pub fn new(u_start: T, u_end: T, u_step: T, v_start: T, v_end: T, v_step: T) -> Self {
        Self {
            u_start,
            u_end,
            u_step,
            v_start,
            v_end,
            v_step,
        }
    }
}

/// 插值函数包装 `T ↦ T`（如重映射 / 缓动）。
pub struct Interpolator<T = f32> {
    pub f: Arc<dyn DynFn<T, T>>,
    /// `Some` = 具名（可序列化）；`None` = 匿名（序列化时明确报错）。
    pub name: Option<String>,
}

impl<T: 'static> Interpolator<T> {
    /// 匿名插值器。
    pub fn new(f: impl Fn(T) -> T + Send + Sync + Clone + 'static) -> Self {
        Self {
            f: Arc::new(f),
            name: None,
        }
    }

    /// 具名插值器（可持久化）。
    pub fn named(
        name: impl Into<String>,
        f: impl Fn(T) -> T + Send + Sync + Clone + 'static,
    ) -> Self {
        Self {
            f: Arc::new(f),
            name: Some(name.into()),
        }
    }
}

impl<T: 'static> Clone for Interpolator<T> {
    fn clone(&self) -> Self {
        Self {
            f: self.f.clone_dyn(),
            name: self.name.clone(),
        }
    }
}

#[cfg(feature = "serde")]
impl<T> serde::Serialize for Interpolator<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.name {
            Some(name) => serializer.serialize_str(name),
            None => Err(serde::ser::Error::custom(
                "匿名 Interpolator 不可序列化：请用 named(...)",
            )),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de, T> serde::Deserialize<'de> for Interpolator<T>
where
    T: 'static,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let name = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self {
            f: crate::func::unbound(),
            name: Some(name),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sampler1d_fields() {
        let s = Sampler1D::new(-10.0f32, 10.0, 0.1);
        assert_eq!((s.start, s.end, s.step), (-10.0, 10.0, 0.1));
    }

    #[test]
    fn test_sampler2d_fields() {
        let s = Sampler2D::new(0.0f32, 1.0, 0.25, -1.0, 1.0, 0.5);
        assert_eq!((s.u_start, s.u_end, s.u_step), (0.0, 1.0, 0.25));
        assert_eq!((s.v_start, s.v_end, s.v_step), (-1.0, 1.0, 0.5));
    }

    #[test]
    fn test_interpolator_call() {
        let i = Interpolator::new(|t: f32| t * t); // ease-in
        assert_eq!((i.f)(0.5), 0.25);
    }

    #[test]
    fn test_interpolator_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Interpolator<f32>>();
    }
}
