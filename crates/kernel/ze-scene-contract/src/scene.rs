// scene.rs — 场景抽象（契约层）。
//
// 目的（M3-7 / M3-8）：让渲染管线与宿主只认识 `Scene` trait，**不再认识 ECS**。
// ECS 细节（hecs / archetype / 借用守卫）只出现在实现方（`zela-scene`）。

use std::any::Any;

use crate::id::ObjectId;
use crate::relation::Parent;

/// 可放进 [`Components`] 的组件：`Send + Sync + Clone + 'static`。
///
/// 为什么需要独立 trait（而不是继续用 `dyn Any`）：`SceneScript` 要 **Clone**
/// 才能"克隆后重新 commit"（`commit(self)` 签名不变），而 `Box<dyn Any>` 不可 Clone。
/// 本 trait 把"可克隆"变成类型擦除集合的**编译期要求**。
///
/// `as_any` / `as_any_mut` 供动态创建（downcast）与句柄引用改写使用；
/// `into_any` 供 `spawn_dyn` 的登记表按具体类型装箱。
///
/// ⚠️ **调用约定**：`Box<dyn Component>` 自身也满足本 trait 的 blanket impl。
/// 从 `Box<dyn Component>` 取组件的 `Any` 时必须先解引用到 trait object：
/// `(**b).as_any()`（`b: &Box<dyn Component>`）或 `(*b).as_any()`（按值位置），
/// 否则拿到的是 **Box 本身** 的 `Any`。
pub trait Component: Send + Sync + 'static {
    /// 只读向下转型。
    fn as_any(&self) -> &dyn Any;
    /// 可变向下转型（句柄引用改写用）。
    fn as_any_mut(&mut self) -> &mut dyn Any;
    /// 取回类型擦除的 `Box`（`spawn_dyn` 的登记表消费）。
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send + Sync>;
    /// 深克隆（`Box<dyn Component>` 的 Clone 落点）。
    fn clone_box(&self) -> Box<dyn Component>;
}

impl<T: Clone + Send + Sync + 'static> Component for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any + Send + Sync> {
        self
    }

    fn clone_box(&self) -> Box<dyn Component> {
        Box::new(self.clone())
    }
}

impl Clone for Box<dyn Component> {
    fn clone(&self) -> Self {
        // 必须先解引用到 `dyn Component`（走 vtable 到具体类型的 `clone_box`）；
        // 直接 `self.clone_box()` 会命中 blanket impl（`Box<dyn Component>` 自身
        // 也满足 `Clone + Send + Sync`）⇒ 无限递归。
        (**self).clone_box()
    }
}

/// 动态组件集合：`spawn_dyn` 的载荷（每个 `Box` 是一个组件实例）。
///
/// 与旧版 `Vec<Box<dyn Any + Send + Sync>>` 的唯一区别：元素可 **Clone**
/// （编译器强制组件实现 `Clone`）⇒ `SceneScript` 可 Clone / 可重放。
pub type Components = Vec<Box<dyn Component>>;

/// **只读**场景视图：查询与迭代。
///
/// 为什么与 [`Scene`] 分开：渲染录制（`record_scene`）与大多数查询**只需要读**，
/// 而只读能力可以被 `&World` 满足（`WorldView`）；写能力（spawn / set）必须
/// 独占借用。把只读拆出来 ⇒ 调用点可以用不可变借用接进来
/// （例如 `SceneRecordSystem` 拿到的就是 `&hecs::World`），
/// 且"管线只需要读"这件事在**类型层面**成立。
pub trait SceneRead {
    /// 对象是否存在（句柄有效）。
    fn contains(&self, id: ObjectId) -> bool;

    /// 当前对象数。
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 逐对象访问带组件 `T` 的对象（**访客形态**）。
    ///
    /// 顺序为实现方（ECS）的存储顺序 —— 调用方不得依赖它跨实现一致，
    /// 但同一实现内必须**稳定**（渲染录制依赖"同 z 保持迭代顺序"）。
    ///
    /// `Send + Sync` 约束来自"组件要能放进 ECS 的多线程系统"这一事实
    /// （与 [`Components`] 的 `Box<dyn Any + Send + Sync>` 同一口径）。
    ///
    /// # 为什么是闭包而不是 `impl Iterator`（实测后的形态修正）
    ///
    /// hecs 的安全 API 把迭代项的**生命周期绑在 `QueryBorrow` 的 `&mut` 借用上**
    /// （`QueryBorrow::iter(&mut self) -> QueryIter<'_, Q>`），且没有按值
    /// `IntoIterator`：把 QueryBorrow 放在方法体里（临时值）就无法把迭代器返回出去。
    /// 访客形态让 QueryBorrow 的生命周期收在方法体内 —— 这是"借用守卫"这一事实的
    /// 直接后果，不是为了迁就实现而降低表达力（`for_each` 同样表达全部查询）。
    fn for_each<T: 'static + Send + Sync>(&self, f: impl FnMut(ObjectId, &T));

    /// 逐对象访问**同时**带组件 `A` 与 `B` 的对象（顺序语义同上）。
    fn for_each_pair<A: 'static + Send + Sync, B: 'static + Send + Sync>(
        &self,
        f: impl FnMut(ObjectId, &A, &B),
    );

    /// 逐对象访问 `(A, B)`，并**可选**地取 `C`：`C` 缺席时传 `None`。
    ///
    /// 为什么需要"可选"：对象层新增了**可缺席的摆放信息**（如 `ZOrder` 绘制层级）——
    /// 绝大多数对象没有它（用默认值），只有需要显式分层的对象才有；
    /// 若要求 `C` 必须存在，就没法用一次查询同时覆盖两类对象。
    ///
    /// hecs 侧即 `query::<(Entity, &A, &B, Option<&C>)>()`：ECS 的 `Option<&T>`
    /// 查询项是原生支持的，故这不是额外负担。
    fn for_each_pair_opt<
        A: 'static + Send + Sync,
        B: 'static + Send + Sync,
        C: 'static + Send + Sync,
    >(
        &self,
        f: impl FnMut(ObjectId, &A, &B, Option<&C>),
    );

    /// **只读点查询（访客形态）**：`id` 存在且带组件 `T` 时，以 `&T` 调用 `f`
    /// 并返回其结果；对象不存在 / 未挂 `T` ⇒ `None`。
    ///
    /// 用途（P1-3）：相机**句柄**（`Camera2DHandle`）要按 `ObjectId` 取一个场景对象上的
    /// 取景参数，而渲染录制只拿得到 `&S`（[`SceneRead`]），没有 `&mut`。
    ///
    /// # 为什么是访客形态，而不是 `Option<&T>`（实测修正）
    ///
    /// hecs 0.11 的 `World::get::<&T>(entity)` 返回的是**借用守卫**
    /// `Ref<'_, T>`（生命周期绑在 `&World` 的借用上，守卫本身必须存活），
    /// 因此无法把 `&T` 直接交出去 —— 与 [`SceneRead::for_each`] 是**同一约束**，
    /// 不是为迁就实现而降低表达力（`with` 同样表达"按 id 读一个组件"）。
    fn with<T: 'static + Send + Sync, R>(&self, id: ObjectId, f: impl FnOnce(&T) -> R)
    -> Option<R>;
}

/// **读写**场景：在 [`SceneRead`] 之上补创建 / 销毁 / 写入。
///
/// 装配对象（`Axes2D::build` / `FunctionGraph::refresh`）需要它；
/// 纯消费者（渲染管线 / 查询）只需 [`SceneRead`]。
pub trait Scene: SceneRead {
    /// 以动态组件集合创建对象。
    ///
    /// **任一组件类型未被实现方登记时返回 `None`**（实现方须在文档中列出登记表）。
    fn spawn_dyn(&mut self, components: Components) -> Option<ObjectId>;

    /// 销毁对象；返回是否确实存在（`false` = 句柄无效）。
    fn despawn(&mut self, id: ObjectId) -> bool;

    /// 把 `child` 挂到 `parent` 之下（写入子对象的 [`Parent`] 组件）。
    ///
    /// 返回是否成功（对象不存在即 `false`）。父对象的 [`Children`] 维护
    /// 由装配层决定（本 trait 不强制双向同步）。
    ///
    /// [`Children`]: crate::relation::Children
    fn attach(&mut self, child: ObjectId, parent: Parent) -> bool;

    /// 取某对象上组件的可变引用。
    fn get_mut<T: 'static + Send + Sync>(&mut self, id: ObjectId) -> Option<&mut T>;

    /// **写入路径（M4-3b）**：插入 / 覆盖某对象上的组件。
    ///
    /// 为什么必须补：契约层最初按 `record_scene` 的**只读**需求设计，
    /// 而"装配"（坐标轴 / 采样网格每帧刷新）是**写**路径 —— 没有它，
    /// `Axes2D::refresh` 之类的装配对象无法走 `Scene` trait。
    ///
    /// 返回 `false` = 对象不存在（句柄无效）。
    fn set<T: Component>(&mut self, id: ObjectId, component: T) -> bool;
}
