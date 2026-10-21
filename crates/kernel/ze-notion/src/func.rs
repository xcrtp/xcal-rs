// func.rs — 函数元语。
//
// 从 `zela-scene::scene::components` 归位（M1 T4）：`FunctionX2Y` 此前与
// `Parent` / `Transform` / `Circle` 等 ECS 组件挤在同一文件里，但它是**纯数学函数**
// ——函数与参数曲线、采样器、插值器同属理念层。
//
// 纯度约束：本模块只有 `std::sync::Arc` 与函数类型，不含 ECS / GPU / UI 概念。
//
// # 函数持有者统一到 `DynFn`（本轮）
//
// 旧形态 `Arc<dyn SyncSendFn<X,Y>>` 只能"浅克隆 Arc"，且**不能 downcast**。
// 现在统一为：
//
// ```text
// Arc<dyn DynFn<X, Y>>
//      DynFn: Send + Sync + Fn(X)->Y + Any
// ```
//
// - `Arc` 必须保留：`FunctionGraph` 在组件登记表中，`Box<dyn Fn>` 不可 Clone；
// - 深 Clone：`clone_dyn()` 克隆**捕获的环境**（新 Arc，不是引用计数 +1）；
// - `Any`：提供 downcast 能力（未来 Pod / 插件可识别具体函数对象）。
//
// **对象安全性核查**：`Fn(X)->Y` 是单方法 trait，`Any` 是标记型 auto-trait，
// 二者叠加仍 object-safe；`clone_dyn` 返回 `Arc<dyn DynFn<X,Y>>`（不是 `Self`），
// 故不需要 `Sized`。计划里的 `DynClone` 超 trait 无法 object-safe 地返回
// `Arc<dyn DynFn<X,Y>>`（返回 `Box<dyn DynClone>` 后无法再收回类型），
// 因此把该能力以 `clone_dyn` 方法的形式落在 `DynFn` 上 —— 语义相同。

use std::sync::Arc;

/// 可深克隆、可向下转型的函数对象。
///
/// 闭包在捕获变量都 `Clone` 时自动 `Clone`，故本 trait 的 blanket 实现覆盖
/// 普通闭包与裸 fn 指针。
pub trait DynFn<X, Y>: Send + Sync + Fn(X) -> Y + std::any::Any {
    /// 深克隆：新的 `Arc`，且**捕获的环境**也被克隆。
    fn clone_dyn(&self) -> Arc<dyn DynFn<X, Y>>;
}

impl<X, Y, F> DynFn<X, Y> for F
where
    F: Fn(X) -> Y + Send + Sync + Clone + 'static,
{
    fn clone_dyn(&self) -> Arc<dyn DynFn<X, Y>> {
        Arc::new(self.clone())
    }
}

/// 反序列化得到的"未绑定函数"：调用即 panic（明确失败，不静默给错值）。
///
/// 反序列化只能拿到**名字**，函数体必须由调用方按名字重新绑定（见
/// [`FunctionX2Y::name`]）。在绑定之前调用它属于契约违反。
pub(crate) fn unbound<X, Y>() -> Arc<dyn DynFn<X, Y>>
where
    X: 'static,
    Y: 'static,
{
    Arc::new(|_x: X| -> Y {
        panic!("函数未绑定：反序列化后必须按 name 用 registry / Pod 重新绑定才能调用")
    })
}

/// 一元实函数 `y = f(x)`（默认 `f32 → f32`），共享持有。
///
/// `name` 是**持久化替身**：序列化只写名字，运行时求值仍直接用 `Arc`（不查表）。
pub struct FunctionX2Y<X = f32, Y = f32> {
    pub f: Arc<dyn DynFn<X, Y>>,
    /// `Some` = 具名（可序列化）；`None` = 匿名（序列化时**明确报错**）。
    pub name: Option<String>,
    /// 函数体是否已绑定（反序列化得到的"名字壳"为 `false`）。
    bound: bool,
}

impl<X: 'static, Y: 'static> FunctionX2Y<X, Y> {
    /// 匿名函数（行为不变；不能持久化）。
    pub fn new(f: impl Fn(X) -> Y + Send + Sync + Clone + 'static) -> Self {
        Self {
            f: Arc::new(f),
            name: None,
            bound: true,
        }
    }

    /// 具名函数（可持久化）：名字是唯一被写进序列化的东西。
    pub fn named(
        name: impl Into<String>,
        f: impl Fn(X) -> Y + Send + Sync + Clone + 'static,
    ) -> Self {
        Self {
            f: Arc::new(f),
            name: Some(name.into()),
            bound: true,
        }
    }

    /// 是否为具名函数（可持久化）。
    pub fn is_named(&self) -> bool {
        self.name.is_some()
    }

    /// 函数体是否已绑定。
    ///
    /// 反序列化只拿得到名字 ⇒ `false`；由 Pod / 内置表按名反查后变 `true`。
    /// 未绑定时调用会 panic（明确失败）。
    pub fn is_bound(&self) -> bool {
        self.bound
    }

    /// 绑定函数体（把"名字壳"补成可调用对象）。
    ///
    /// 名字保持不变（`name` 仍是持久化替身）；`bound` 置 `true`。
    pub fn bind(&mut self, f: impl Fn(X) -> Y + Send + Sync + Clone + 'static) {
        self.f = Arc::new(f);
        self.bound = true;
    }
}

/// **深 Clone**：克隆捕获环境（`clone_dyn`），不是 `Arc` 引用计数 +1。
impl<X: 'static, Y: 'static> Clone for FunctionX2Y<X, Y> {
    fn clone(&self) -> Self {
        Self {
            f: self.f.clone_dyn(),
            name: self.name.clone(),
            bound: self.bound,
        }
    }
}

#[cfg(feature = "serde")]
impl<X, Y> serde::Serialize for FunctionX2Y<X, Y> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.name {
            Some(name) => serializer.serialize_str(name),
            None => Err(serde::ser::Error::custom(
                "匿名函数不可序列化：请用 FunctionX2Y::named(...)",
            )),
        }
    }
}

#[cfg(feature = "serde")]
impl<'de, X, Y> serde::Deserialize<'de> for FunctionX2Y<X, Y>
where
    X: 'static,
    Y: 'static,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let name = <String as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self {
            f: unbound(),
            name: Some(name),
            bound: false,
        })
    }
}

/// 参数曲面函数 `z = f(x, y)` + 参数域（纯数学，M1 T5）。
///
/// 从 `zela-scene` 的 `SurfaceSpec` 劈出：`SurfaceSpec` 原把
/// `u_steps` / `v_steps`（"这次采样多少格"）混进了数学定义。
/// `z = f(x, y)` 这个概念只需要**函数 + 域**；步数是离散化决策，留在场景层
/// （见 `zela_scene_ecs::scene::primitives::parametric_surface` 的 `u_steps` / `v_steps` 参数）。
///
/// # 本轮变化：不再是裸 fn 指针（也不再 `Copy`）
///
/// 旧形态 `f: fn(f32, f32) -> f32` 若被序列化会写成**地址** ⇒ 加载回来指向错误位置
/// （不报错、结果错乱）。现在与 [`FunctionX2Y`] 统一为 `Arc<dyn DynFn>` + `name`。
pub struct SurfaceFn {
    /// u 方向（x）参数域 `(min, max)`。
    pub u_range: (f32, f32),
    /// v 方向（y）参数域 `(min, max)`。
    pub v_range: (f32, f32),
    /// 高度函数 `z = f(x, y)`；常数函数即平面。
    pub f: Arc<dyn DynFn<(f32, f32), f32>>,
    /// `Some` = 具名（可序列化）；`None` = 匿名（序列化时明确报错）。
    pub name: Option<String>,
}

impl SurfaceFn {
    /// 匿名高度函数。
    pub fn new(
        u_range: (f32, f32),
        v_range: (f32, f32),
        f: impl Fn(f32, f32) -> f32 + Send + Sync + Clone + 'static,
    ) -> Self {
        Self {
            u_range,
            v_range,
            // `DynFn<X,Y>` 是单参 `Fn(X)->Y`：二元函数用元组入参适配。
            f: Arc::new(move |(x, y): (f32, f32)| f(x, y)),
            name: None,
        }
    }

    /// 具名高度函数（可持久化）。
    pub fn named(
        name: impl Into<String>,
        u_range: (f32, f32),
        v_range: (f32, f32),
        f: impl Fn(f32, f32) -> f32 + Send + Sync + Clone + 'static,
    ) -> Self {
        Self {
            u_range,
            v_range,
            f: Arc::new(move |(x, y): (f32, f32)| f(x, y)),
            name: Some(name.into()),
        }
    }

    /// 求值：`z = f(x, y)`。
    pub fn eval(&self, x: f32, y: f32) -> f32 {
        (self.f)((x, y))
    }
}

impl std::fmt::Debug for SurfaceFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SurfaceFn")
            .field("u_range", &self.u_range)
            .field("v_range", &self.v_range)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl Clone for SurfaceFn {
    fn clone(&self) -> Self {
        Self {
            u_range: self.u_range,
            v_range: self.v_range,
            f: self.f.clone_dyn(),
            name: self.name.clone(),
        }
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for SurfaceFn {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let Some(name) = &self.name else {
            return Err(serde::ser::Error::custom(
                "匿名 SurfaceFn 不可序列化：请用 SurfaceFn::named(...)",
            ));
        };
        let mut state = serializer.serialize_struct("SurfaceFn", 3)?;
        state.serialize_field("u_range", &self.u_range)?;
        state.serialize_field("v_range", &self.v_range)?;
        state.serialize_field("name", name)?;
        state.end()
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for SurfaceFn {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Repr {
            u_range: (f32, f32),
            v_range: (f32, f32),
            name: String,
        }
        let repr = <Repr as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self {
            u_range: repr.u_range,
            v_range: repr.v_range,
            f: unbound(),
            name: Some(repr.name),
        })
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

    /// `DynFn` 抽象的性质：函数可作为 ECS 组件的必要条件。
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

    /// **深 Clone 可证伪**：克隆后是**新** `Arc`（不是引用计数 +1），行为一致。
    #[test]
    fn clone_is_deep_not_shared() {
        let a = FunctionX2Y::new(|x: f32| x + 1.0);
        let b = a.clone();
        assert!(!Arc::ptr_eq(&a.f, &b.f), "深克隆必须产生新的 Arc");
        assert_eq!((a.f)(1.0), (b.f)(1.0), "行为一致");
    }
}
