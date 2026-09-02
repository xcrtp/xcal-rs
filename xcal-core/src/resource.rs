// resource.rs
//
// Resource marker trait + Resources 容器 + ResourceTuple 递归 trait。
//
// 设计：
// - `Resource` 是任何 `Send + Sync + 'static` 类型的 marker，
//   通过 blanket impl 自动覆盖；下游无需手写 impl。
// - `Resources` 是按 `TypeId` 索引的单一实例容器。
// - `ResourceTuple` 对原生 tuple `()`、`(A,)`、`(A, B)`、`(A, B, C)` … 递归展开，
//   用于：
//     * `System::Reads` / `System::Writes` 关联类型约束（仅收集 TypeId）
//     * `Access` 构造（收集所有 TypeId 用于冲突检测）
//   - `()` 作为 Nil（空 tuple）
// - `ResourceTupleDefault` 是 `ResourceTuple` 的子 trait，仅当所有元素都实现
//   `Default` 时才被实现，用于 `InitSystem::init` 默认注入资源。

use std::any::{Any, TypeId};
use std::collections::HashMap;

// ─── Resource marker ──────────────────────────────────────────────────────

/// 所有运行时资源类型的 marker。
///
/// 通过 blanket impl 自动覆盖任何 `Send + Sync + 'static` 类型；
/// 下游无需手写 impl 即可放入 [`Resources`]。
pub trait Resource: Send + Sync + 'static {}

impl<T: Send + Sync + 'static> Resource for T {}

// ─── Resources 容器 ───────────────────────────────────────────────────────

/// 全局运行时资源容器。
///
/// 与 `hecs::World` 平级，存放非组件形式的全局状态（如相机、时间、配置等）。
/// 每个资源类型只能有一个实例；访问时用类型作为 key。
///
/// 注意：本容器不提供运行时借用追踪；debug 模式下的借用追踪在
/// [`crate::context::ExecutorContext`] 中通过 `BorrowTracker` 完成。
#[derive(Default)]
pub struct Resources {
    data: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

impl Resources {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<R: Resource>(&mut self, resource: R) {
        self.data.insert(TypeId::of::<R>(), Box::new(resource));
    }

    pub fn get<R: Resource>(&self) -> Option<&R> {
        self.data
            .get(&TypeId::of::<R>())
            .and_then(|boxed| boxed.downcast_ref::<R>())
    }

    pub fn get_mut<R: Resource>(&mut self) -> Option<&mut R> {
        self.data
            .get_mut(&TypeId::of::<R>())
            .and_then(|boxed| boxed.downcast_mut::<R>())
    }

    pub fn remove<R: Resource>(&mut self) -> Option<R> {
        self.data
            .remove(&TypeId::of::<R>())
            .and_then(|boxed| boxed.downcast::<R>().ok())
            .map(|b| *b)
    }

    pub fn contains<R: Resource>(&self) -> bool {
        self.data.contains_key(&TypeId::of::<R>())
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

// ─── ResourceTuple 递归 ───────────────────────────────────────────────────

/// 对原生 tuple 递归展开的 trait。
///
/// 用途：
/// - 关联类型 `System::Reads` / `System::Writes` 的约束
/// - 收集 tuple 中所有 `Resource` 的 `TypeId`，构造 [`crate::access::Access`]
///
/// 任意元 tuple `(A, B, C, ...)` 均可直接实现 `ResourceTuple`（通过宏生成），
/// 无需嵌套写法。`()` 作为 Nil 表示空 tuple。
///
/// 注意：本 trait 不要求元素实现 `Default`；如需 `Default` 注入能力
/// （用于 `InitSystem::init`），请使用 [`ResourceTupleDefault`]。
pub trait ResourceTuple: 'static {
    /// 返回该 tuple 中所有 `Resource` 类型的 `TypeId`。
    fn ids() -> Vec<TypeId>;
}

/// `ResourceTuple` 的子 trait，额外提供 `Default` 注入能力。
///
/// 仅当 tuple 中所有元素都实现 `Default` 时才会被实现；
/// 因此 `Writes = (NonDefaultType,)` 会编译失败，迫使作者为该类型手写
/// `InitSystem::init` 而非走默认实现。
pub trait ResourceTupleDefault: ResourceTuple {
    /// 对 tuple 中每个 `T: Resource + Default`，若 `Resources` 中缺失则插入默认值。
    fn ensure_default(resources: &mut Resources);
}

// 让 ResourceTupleDefault 也要求 'static（通过 supertrait 已传递）。
// 不需要单独写。

// Nil：空 tuple `()` 同时实现 ResourceTuple 与 ResourceTupleDefault。
impl ResourceTuple for () {
    fn ids() -> Vec<TypeId> {
        Vec::new()
    }
}
impl ResourceTupleDefault for () {
    fn ensure_default(_resources: &mut Resources) {}
}

// 递归宏：对元数 1..=N 生成 impl，区分两个 trait：
// - `ResourceTuple` 仅需 `: Resource`
// - `ResourceTupleDefault` 额外需 `: Default`
macro_rules! impl_resource_tuple {
    ($($T:ident),+) => {
        impl<$($T: Resource),+> ResourceTuple for ($($T,)+) {
            fn ids() -> Vec<TypeId> {
                vec![$(TypeId::of::<$T>()),+]
            }
        }

        impl<$($T: Resource + ::std::default::Default),+> ResourceTupleDefault for ($($T,)+) {
            fn ensure_default(resources: &mut Resources) {
                $(
                    if !resources.contains::<$T>() {
                        resources.insert(<$T as ::std::default::Default>::default());
                    }
                )+
            }
        }
    };
}

impl_resource_tuple!(A);
impl_resource_tuple!(A, B);
impl_resource_tuple!(A, B, C);
impl_resource_tuple!(A, B, C, D);
impl_resource_tuple!(A, B, C, D, E, F);
impl_resource_tuple!(A, B, C, D, E, F, G, H);
impl_resource_tuple!(A, B, C, D, E, F, G, H, I, J, K, L);
impl_resource_tuple!(A, B, C, D, E, F, G, H, I, J, K, L, M, N, O, P);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resources_insert_and_get() {
        let mut r = Resources::new();
        r.insert(42_i32);
        r.insert(2.5_f64);
        assert_eq!(r.get::<i32>(), Some(&42));
        assert_eq!(r.get::<f64>(), Some(&2.5));
    }

    #[test]
    fn test_resources_get_mut() {
        let mut r = Resources::new();
        r.insert(42_i32);
        if let Some(val) = r.get_mut::<i32>() {
            *val = 100;
        }
        assert_eq!(r.get::<i32>(), Some(&100));
    }

    #[test]
    fn test_resources_missing_returns_none() {
        let r = Resources::new();
        assert!(r.get::<i32>().is_none());
    }

    #[test]
    fn test_resources_remove() {
        let mut r = Resources::new();
        r.insert(42_i32);
        assert!(r.contains::<i32>());
        let removed = r.remove::<i32>();
        assert_eq!(removed, Some(42));
        assert!(!r.contains::<i32>());
    }

    #[test]
    fn test_nil_resource_tuple() {
        assert!(<()>::ids().is_empty());
        let mut r = Resources::new();
        <()>::ensure_default(&mut r);
        assert!(r.is_empty());
    }

    #[test]
    fn test_singleton_resource_tuple_ids() {
        let ids = <(i32,)>::ids();
        assert_eq!(ids, vec![TypeId::of::<i32>()]);
    }

    #[test]
    fn test_pair_resource_tuple_ids() {
        let ids = <(i32, f64)>::ids();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&TypeId::of::<i32>()));
        assert!(ids.contains(&TypeId::of::<f64>()));
    }

    #[test]
    fn test_triple_resource_tuple_ids() {
        let ids = <(i32, f64, bool)>::ids();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn test_ensure_default_inserts_missing() {
        let mut r = Resources::new();
        <(i32, f64)>::ensure_default(&mut r);
        assert_eq!(r.get::<i32>(), Some(&0_i32));
        assert_eq!(r.get::<f64>(), Some(&0.0_f64));
    }

    #[test]
    fn test_ensure_default_preserves_existing() {
        let mut r = Resources::new();
        r.insert(42_i32);
        <(i32, f64)>::ensure_default(&mut r);
        assert_eq!(r.get::<i32>(), Some(&42));
        assert_eq!(r.get::<f64>(), Some(&0.0));
    }
}
