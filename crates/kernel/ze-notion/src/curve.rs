// curve.rs — 参数曲线元语。
//
// 从 `zela-scene::scene::components` 归位（M1 T4）。参数曲线是**纯数学**：
// 参数 `t` 到一个点（2D / 3D）的映射，不含任何渲染 / 采样知识。
//
// 采样（把曲线离散成折线）属**动作归属**问题：类型在理念层，采样动作在
// 场景对象层 / 装配层 —— 因此本模块只定义曲线本身，不提供 `sample()`。
//
// 本轮：函数持有者统一为 `Arc<dyn DynFn>`（深克隆 + Any），并带可选 `name`
// （持久化只写名字）。

use std::sync::Arc;

use crate::func::DynFn;

/// 参数曲线 `t ↦ (x, y)`（默认 `f32 → (f32, f32)`），共享持有。
pub struct ParameterCurveT2XY<T = f32, XY = (f32, f32)> {
    pub f: Arc<dyn DynFn<T, XY>>,
    /// `Some` = 具名（可序列化）；`None` = 匿名（序列化时明确报错）。
    pub name: Option<String>,
}

impl<T: 'static, XY: 'static> ParameterCurveT2XY<T, XY> {
    /// 匿名参数曲线。
    pub fn new(f: impl Fn(T) -> XY + Send + Sync + Clone + 'static) -> Self {
        Self {
            f: Arc::new(f),
            name: None,
        }
    }

    /// 具名参数曲线（可持久化）。
    pub fn named(
        name: impl Into<String>,
        f: impl Fn(T) -> XY + Send + Sync + Clone + 'static,
    ) -> Self {
        Self {
            f: Arc::new(f),
            name: Some(name.into()),
        }
    }
}

impl<T: 'static, XY: 'static> Clone for ParameterCurveT2XY<T, XY> {
    fn clone(&self) -> Self {
        Self {
            f: self.f.clone_dyn(),
            name: self.name.clone(),
        }
    }
}

#[cfg(feature = "serde")]
impl<T, XY> serde::Serialize for ParameterCurveT2XY<T, XY> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.name {
            Some(name) => serializer.serialize_str(name),
            None => Err(serde::ser::Error::custom(
                "匿名 ParameterCurveT2XY 不可序列化：请用 named(...)",
            )),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de, T, XY> serde::Deserialize<'de> for ParameterCurveT2XY<T, XY>
where
    T: 'static,
    XY: 'static,
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

/// 参数曲线 `t ↦ (x, y, z)`（默认 `(f32, f32, f32)`）。
///
/// 参数曲线采样后曾直接吐 `Vec<Vec3>`，没有 3D 曲线类型承载 —— M1 补
/// [`Path3D`](crate::path::Path3D) 作为承载类型。
pub struct ParameterCurveT2XYZ<T, XYZ = (f32, f32, f32)> {
    pub f: Arc<dyn DynFn<T, XYZ>>,
    /// `Some` = 具名（可序列化）；`None` = 匿名（序列化时明确报错）。
    pub name: Option<String>,
}

impl<T: 'static, XYZ: 'static> ParameterCurveT2XYZ<T, XYZ> {
    /// 匿名参数曲线。
    pub fn new(f: impl Fn(T) -> XYZ + Send + Sync + Clone + 'static) -> Self {
        Self {
            f: Arc::new(f),
            name: None,
        }
    }

    /// 具名参数曲线（可持久化）。
    pub fn named(
        name: impl Into<String>,
        f: impl Fn(T) -> XYZ + Send + Sync + Clone + 'static,
    ) -> Self {
        Self {
            f: Arc::new(f),
            name: Some(name.into()),
        }
    }
}

impl<T: 'static, XYZ: 'static> Clone for ParameterCurveT2XYZ<T, XYZ> {
    fn clone(&self) -> Self {
        Self {
            f: self.f.clone_dyn(),
            name: self.name.clone(),
        }
    }
}

#[cfg(feature = "serde")]
impl<T, XYZ> serde::Serialize for ParameterCurveT2XYZ<T, XYZ> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.name {
            Some(name) => serializer.serialize_str(name),
            None => Err(serde::ser::Error::custom(
                "匿名 ParameterCurveT2XYZ 不可序列化：请用 named(...)",
            )),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de, T, XYZ> serde::Deserialize<'de> for ParameterCurveT2XYZ<T, XYZ>
where
    T: 'static,
    XYZ: 'static,
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
    fn test_parameter_curve_2xy() {
        let c = ParameterCurveT2XY::new(|t: f32| (t.cos(), t.sin()));
        assert_eq!((c.f)(0.0), (1.0, 0.0));
    }

    #[test]
    fn test_parameter_curve_2xyz() {
        let c = ParameterCurveT2XYZ::new(|t: f32| (t, t * t, t * t * t));
        assert_eq!((c.f)(2.0), (2.0, 4.0, 8.0));
    }

    /// 统一到 `DynFn` 后可作 ECS 组件（hecs 要求 `Send + Sync`）。
    #[test]
    fn test_parameter_curves_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ParameterCurveT2XY<f32, (f32, f32)>>();
        assert_send_sync::<ParameterCurveT2XYZ<f32, (f32, f32, f32)>>();
    }
}
