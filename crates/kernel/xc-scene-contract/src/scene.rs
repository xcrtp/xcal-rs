// scene.rs — 场景抽象（契约层）。
//
// 目的（M3-7 / M3-8）：让渲染管线与宿主只认识 `Scene` trait，**不再认识 ECS**。
// ECS 细节（hecs / archetype / 借用守卫）只出现在实现方（`xcal-scene`）。

use crate::id::ObjectId;
use crate::relation::Parent;

/// 动态组件集合：`spawn_dyn` 的载荷（每个 `Box` 是一个组件实例）。
pub type Components = Vec<Box<dyn std::any::Any + Send + Sync>>;

/// 场景：按 [`ObjectId`] 增删 / 访问组件的最小抽象。
///
/// # 为什么不是 object-safe
///
/// `spawn_dyn` / `iter` 等按设计就是泛型的（组件类型在编译期确定），
/// 本 trait **不打算**运行时切换实现 —— 泛型静态分发零开销即可，
/// 因此接受不能 `dyn Scene`（决策 M2-1 ①）。
///
/// # 为什么没有 `get(&self, id) -> Option<&T>`
///
/// hecs 的共享访问返回 `Ref<'_, T>` **守卫**（带运行时借用检查，Drop 时释放），
/// 无法安全降级为裸 `&T`（守卫必须存活；泄漏守卫会让该 archetype 永久"已借出"）。
/// 故本 trait 的**共享访问统一走迭代**：
///
/// - [`Scene::iter`]：单组件迭代（`(ObjectId, &T)`）；
/// - [`Scene::iter_with`]：双组件迭代（`(ObjectId, &A, &B)`）—— `record_scene`
///   的 7 组 `(图元, 样式)` 组合查询全部用它，从而**不需要** `get`。
///
/// 可变访问 [`Scene::get_mut`] 不受此限：hecs 的 `query_one_mut` 直接给 `&mut T`。
///
/// # 为什么 `spawn` 叫 `spawn_dyn` 且吃 `Components`
///
/// 计划草稿写的是 `spawn<B: Bundle>`，但契约层不含 hecs，`hecs::DynamicBundle`
/// 无法作为约束出现在这里（本地 marker trait 也无法反推"某类型实现了 hecs 的
/// bundle"）。故取最省 churn 的形式：**冷路径**（创建对象不是每帧操作）用
/// `Vec<Box<dyn Any + Send + Sync>>`，热路径（迭代 / 查询）全部保留零开销泛型。
/// 实现方另提供类型化的便利 API（如 `xcal_scene::Scene::spawn(bundle)` 接受元组）。
pub trait Scene {
    /// 以动态组件集合创建对象，返回其句柄。
    fn spawn_dyn(&mut self, components: Components) -> ObjectId;

    /// 销毁对象；返回是否确实存在（`false` = 句柄无效）。
    fn despawn(&mut self, id: ObjectId) -> bool;

    /// 对象是否存在（句柄有效）。
    fn contains(&self, id: ObjectId) -> bool;

    /// 当前对象数。
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 把 `child` 挂到 `parent` 之下（写入子对象的 [`Parent`] 组件）。
    ///
    /// 返回是否成功（任一对象不存在即 `false`）。父对象的 [`Children`] 维护
    /// 由装配层决定（本 trait 不强制双向同步 —— 它与 ECS 的组件写入语义无关）。
    ///
    /// [`Children`]: crate::relation::Children
    fn attach(&mut self, child: ObjectId, parent: Parent) -> bool;

    /// 取某对象上组件的可变引用。
    fn get_mut<T: 'static>(&mut self, id: ObjectId) -> Option<&mut T>;

    /// 迭代**所有**带组件 `T` 的对象。
    ///
    /// 顺序为实现方（ECS）的存储顺序 —— 调用方不得依赖它跨实现一致，
    /// 但同一实现内必须**稳定**（渲染录制依赖"同 z 保持迭代顺序"）。
    fn iter<T: 'static>(&self) -> impl Iterator<Item = (ObjectId, &T)>;

    /// 迭代**同时**带组件 `A` 与 `B` 的对象（顺序语义同上）。
    fn iter_with<A: 'static, B: 'static>(&self) -> impl Iterator<Item = (ObjectId, &A, &B)>;
}
