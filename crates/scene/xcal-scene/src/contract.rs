// contract.rs — 契约层 `Scene` trait 的 **hecs 实现**（M3-7 / M3-8）。
//
// 这里是"具体 ECS"与"契约"的唯一接触面：
// - 渲染管线（`xcal-render::record_scene<S: Scene>`）与装配对象（`Axes2D` / `FunctionGraph`）
//   只认识 `xc_scene_contract::Scene`；
// - hecs 的细节（archetype、`Ref` 借用守卫、`Entity` 位模式）全部封在本文件。
//
// # 为什么不能直接 `impl Scene for hecs::World`（孤儿规则）
//
// trait 在契约层、`World` 在 hecs —— **两者都是外部类型**，任何 crate 都不能
// 为它写 impl（孤儿规则与 impl 所在 crate 无关）。故本层提供**本地适配器**
// [`WorldScene`]（`&mut hecs::World` 的薄包装），并同时为本地类型
// [`crate::scene::Scene`] 实现 —— 两者都是合法落点。
//
// # `ObjectId` ↔ `hecs::Entity`
//
// hecs 的 `Entity` 正是 `{ index, generation }` 模型，故可无损互转
// （`to_bits()` / `from_bits()`；同样因孤儿规则用自由函数而非 `From` 实现）。
//
// # `spawn_dyn` 的组件登记表
//
// hecs 没有类型擦除的 bundle 入口（`EntityBuilder` 只有 `add::<T>()`），
// 故动态创建必须**按类型登记**。下表即唯一登记点：
//
//   图元与摆放：`Circle` / `Line` / `Path` / `Rect` / `Arrow`（T2） / `Transform` / `ZOrder`
//   组织与可见性（N 阶段）：`Name` / `Group` / `Visibility` / `Opacity`
//   相机：      `Camera2D` / `Camera3D` / `Projection` / `Framing2D`（2D 取景参数，P1-2）/
//               `CameraMode`（相机交互模式，K0）/ `FpsView`（FPS 朝向状态，K1）
//   样式：      `StrokeSolid` / `FillSolid`
//   装配对象：  `Axes2D` / `AxesLine` / `FunctionGraph`（M4：网格与函数图像归对象层）
//   动画与标记：`PhaseAnim` / `MorphShape` / `ContentTextureItem` / `Reveal`
//   关系：      `Parent`
//
// 未登记的组件类型 ⇒ `spawn_dyn` 返回 `None`（**不静默丢组件**——宁可失败也不
// 造出"少了一半组件"的对象）。新增可装配组件时，在 `known_component_table!`
// 里加一行即可 —— **正向 / 类型名 / 克隆三向同时生效**（同一张表，只写一处）。

use hecs::{Entity, EntityBuilder, World};

use xc_object::{
    Arrow, Axes2D, AxesLine, Camera2D, Camera3D, Circle, FpsView, Framing2D, FunctionGraph, Line,
    Path, Projection, Rect, Transform, ZOrder,
};
use xc_render_ir::{FillSolid, Opacity, StrokeSolid};

/// 契约层的**只读 / 读写**能力在此处对外转发：
/// 消费方（UI 层 / 装配层 / App）只需 `xcal_scene::contract::*` 一个入口，
/// 不必额外依赖 `xc-scene-contract`（本 crate 已依赖它）。
///
/// 同时转发身份与**组织**类型（句柄 / 目标 / 可见性判定都要用到它们）：
/// `ObjectId`、`Name`、`Group` / `GroupId`、`Visibility` / `VisibilityTag`、
/// `GroupVisibility`（组级开关）。
///
/// **B0（场景脚本可达性修复）**：此前这里**只**转发 `SceneRead` 一族，写 trait
/// [`Scene`]（`spawn_dyn` 的宿主）与载荷类型 [`Components`] **没有对外路径**
/// ⇒ 装配层无法走"经 `add_known` 登记表"的动态创建入口。二者在此补发。
pub use xc_scene_contract::{
    Component, Components, Group, GroupId, GroupVisibility, Name, ObjectId, Parent, Scene,
    SceneRead, Visibility, VisibilityTag,
};

use crate::domain::CameraMode;
use crate::scene::components::{ContentTextureItem, MorphShape, PhaseAnim, Reveal};

/// `hecs::Entity` → 契约层 [`ObjectId`]（位模式直通：低 32 位 index、高 32 位 generation）。
pub fn object_id_of(e: Entity) -> ObjectId {
    let bits = e.to_bits().get();
    ObjectId::new(bits as u32, (bits >> 32) as u32)
}

/// 契约层 [`ObjectId`] → `hecs::Entity`；位模式非法时返回 `None`。
pub fn entity_of(id: ObjectId) -> Option<Entity> {
    let bits = ((id.generation as u64) << 32) | (id.index as u64);
    Entity::from_bits(bits)
}

/// **按名字查找对象**（N1）：返回**第一个**命中的 [`ObjectId`]。
///
/// 用 `for_each::<Name>` 遍历实现，**不新增契约层 trait 方法**
/// （契约层加方法要动 4 个实现 —— `WorldView` / `WorldScene` / `scene::Scene` /
/// 测试用的非 ECS `TestScene` —— 成本与收益不匹配）。
///
/// - 场景规模为几十个对象 ⇒ O(n) 遍历可接受（无全局注册表）；
/// - **不强制名字唯一**：重名时返回先遍历到的那个（`Name` 的文档已写明）。
///   需要唯一性是**装配层策略**，不是契约的职责。
pub fn find_by_name(scene: &impl SceneRead, name: &str) -> Option<ObjectId> {
    let mut found = None;
    scene.for_each::<Name>(|id, n| {
        if found.is_none() && n.as_str() == name {
            found = Some(id);
        }
    });
    found
}

/// **只读适配器**：借用 `&hecs::World` 并实现 [`SceneRead`]
/// （孤儿规则要求 impl 里出现本地类型）。
///
/// 渲染录制（`record_scene<S: SceneRead>`）只需读到它 —— 故调用点可以
/// 用**不可变**借用接进来（例如 `SceneRecordSystem` 拿到的就是 `&hecs::World`）。
pub struct WorldView<'a> {
    world: &'a World,
}

impl<'a> WorldView<'a> {
    pub fn new(world: &'a World) -> Self {
        Self { world }
    }
}

impl SceneRead for WorldView<'_> {
    fn contains(&self, id: ObjectId) -> bool {
        entity_of(id).is_some_and(|e| self.world.contains(e))
    }

    fn len(&self) -> usize {
        self.world.len() as usize
    }

    fn for_each<T: 'static + Send + Sync>(&self, mut f: impl FnMut(ObjectId, &T)) {
        // hecs 的 `query::<Q>()` 只产出 `Q::Item`（不含实体）；`Entity` 本身
        // 也是可查询项（`impl Query for Entity`），故显式把它放进查询元组。
        // QueryBorrow 的生命周期收在本方法体内 —— 这正是访客形态的收益。
        for (e, t) in self.world.query::<(Entity, &T)>().iter() {
            f(object_id_of(e), t);
        }
    }

    fn for_each_pair<A: 'static + Send + Sync, B: 'static + Send + Sync>(
        &self,
        mut f: impl FnMut(ObjectId, &A, &B),
    ) {
        for (e, a, b) in self.world.query::<(Entity, &A, &B)>().iter() {
            f(object_id_of(e), a, b);
        }
    }

    fn for_each_pair_opt<
        A: 'static + Send + Sync,
        B: 'static + Send + Sync,
        C: 'static + Send + Sync,
    >(
        &self,
        mut f: impl FnMut(ObjectId, &A, &B, Option<&C>),
    ) {
        // hecs 原生支持 `Option<&T>` 查询项：`C` 缺席的对象也进结果（传 `None`）。
        for (e, a, b, c) in self.world.query::<(Entity, &A, &B, Option<&C>)>().iter() {
            f(object_id_of(e), a, b, c);
        }
    }

    fn with<T: 'static + Send + Sync, R>(
        &self,
        id: ObjectId,
        f: impl FnOnce(&T) -> R,
    ) -> Option<R> {
        let e = entity_of(id)?;
        // `World::get::<&T>` 返回借用守卫 `Ref<'_, T>`：守卫在本方法体内存活，
        // `&T` 只在 `f` 调用期间有效 —— 这正是访客形态的原因（见契约层文档）。
        let t = self.world.get::<&T>(e).ok()?;
        Some(f(&t))
    }
}

/// **读写适配器**：借用 `&mut hecs::World` 并实现 [`Scene`]
/// （在 [`SceneRead`] 之上补创建 / 销毁 / 写入）。
///
/// 用法（装配对象 / 需要写的调用点）：
///
/// ```ignore
/// axes.refresh(&mut WorldScene::new(world), handle, &camera);
/// ```
pub struct WorldScene<'a> {
    world: &'a mut World,
}

impl<'a> WorldScene<'a> {
    pub fn new(world: &'a mut World) -> Self {
        Self { world }
    }

    /// 取回底层世界（转交 / 收尾用）。
    pub fn into_inner(self) -> &'a mut World {
        self.world
    }
}

impl SceneRead for WorldScene<'_> {
    fn contains(&self, id: ObjectId) -> bool {
        WorldView::new(self.world).contains(id)
    }

    fn len(&self) -> usize {
        WorldView::new(self.world).len()
    }

    fn for_each<T: 'static + Send + Sync>(&self, f: impl FnMut(ObjectId, &T)) {
        WorldView::new(self.world).for_each::<T>(f);
    }

    fn for_each_pair<A: 'static + Send + Sync, B: 'static + Send + Sync>(
        &self,
        f: impl FnMut(ObjectId, &A, &B),
    ) {
        WorldView::new(self.world).for_each_pair::<A, B>(f);
    }

    fn for_each_pair_opt<
        A: 'static + Send + Sync,
        B: 'static + Send + Sync,
        C: 'static + Send + Sync,
    >(
        &self,
        f: impl FnMut(ObjectId, &A, &B, Option<&C>),
    ) {
        WorldView::new(self.world).for_each_pair_opt::<A, B, C>(f);
    }

    fn with<T: 'static + Send + Sync, R>(
        &self,
        id: ObjectId,
        f: impl FnOnce(&T) -> R,
    ) -> Option<R> {
        WorldView::new(self.world).with::<T, R>(id, f)
    }
}

/// **组件登记表（唯一来源）**：正向 downcast、类型名反查、克隆、serde 共用这一份。
///
/// # 为什么是"一张表 + 多个展开器"
///
/// 核心约束是"新增一个已登记类型只写一处"。故 [`add_known`] / [`known_type_name`] /
/// [`clone_known`] / [`serialize_component`] / [`deserialize_component`]
/// 全部由本表展开，各自只需一个极薄的 `macro_rules!` 臂。
///
/// # `clone:` 段
///
/// 所有登记类型都必须 `Clone`（`Components` 的元素要求）—— 这是
/// `SceneScript: Clone` 的直接前提。`AxesLine` 本轮也补上了 `Clone`。
macro_rules! known_component_table {
    ($mac:ident) => {
        $mac! {
            clone: Circle, Line, Path, Rect, Arrow, Transform, ZOrder,
                   Name, Group, Visibility, Opacity,
                   Camera2D, Camera3D, Projection, Framing2D, CameraMode, FpsView,
                   StrokeSolid, FillSolid,
                   Axes2D, AxesLine, FunctionGraph,
                   PhaseAnim, MorphShape, ContentTextureItem, Reveal, Parent;
            manual:
        }
    };
}

/// 反向能力 ①：已登记类型的**稳定名字**（即 Rust 类型名）。
///
/// 供"脚本 / dylib / 序列化"侧把类型记录成可持久化的名字。未登记 ⇒ `None`
/// （与 `spawn_dyn` 的"未登记即失败"同口径）。名字来自 `stringify!`
/// ⇒ **不会与类型定义漂移**。
pub fn known_type_name<T: 'static>() -> Option<&'static str> {
    // 宏定义在函数体内：宏体里的 `T` 才能解析到本函数的泛型参数
    // （与 `add_known` 的 `arm!` 同法 —— 本地变量 / 泛型参数是**定义点卫生**的）。
    macro_rules! name_arms {
        (clone: $($c:ty),+ $(,)?; manual: $($m:ty => $ctor:expr),* $(,)?) => {{
            $(
                if std::any::TypeId::of::<T>() == std::any::TypeId::of::<$c>() {
                    return Some(stringify!($c));
                }
            )+
            $(
                if std::any::TypeId::of::<T>() == std::any::TypeId::of::<$m>() {
                    return Some(stringify!($m));
                }
            )*
        }};
    }
    known_component_table!(name_arms);
    None
}

/// 反向能力 ②：克隆一个**类型擦除**的已登记组件。
///
/// 用途：外部来源（Pod / dylib / 反序列化）拿到一个组件后要把它**复制**进
/// [`Components`] 交给 `spawn_dyn`，而 `Box<dyn Any>` 本身不可 `Clone`。
///
/// 未登记类型 ⇒ `None`（与 `spawn_dyn` 同口径，不 panic）。
pub fn clone_known(
    c: &(dyn std::any::Any + Send + Sync),
) -> Option<Box<dyn std::any::Any + Send + Sync>> {
    macro_rules! clone_arms {
        (clone: $($t:ty),+ $(,)?; manual: $($m:ty => $ctor:expr),* $(,)?) => {{
            $(
                if let Some(v) = c.downcast_ref::<$t>() {
                    return Some(Box::new(v.clone()));
                }
            )+
            $(
                if let Some(v) = c.downcast_ref::<$m>() {
                    let make = $ctor;
                    return Some(Box::new(make(v)));
                }
            )*
        }};
    }
    known_component_table!(clone_arms);
    None
}

/// 按登记表把一个类型擦除的组件加进 builder；未登记返回 `false`。
fn add_known(builder: &mut EntityBuilder, c: Box<dyn Component>) -> bool {
    let mut c: Box<dyn std::any::Any + Send + Sync> = c.into_any();
    // 与 `known_type_name` / `clone_known` / serde **同一张表**（`known_component_table!`）。
    macro_rules! add_arms {
        (clone: $($c:ty),+ $(,)?; manual: $($m:ty => $ctor:expr),* $(,)?) => {{
            $(
                match c.downcast::<$c>() {
                    Ok(v) => {
                        builder.add(*v);
                        return true;
                    }
                    Err(b) => c = b,
                }
            )+
            $(
                match c.downcast::<$m>() {
                    Ok(v) => {
                        builder.add(*v);
                        return true;
                    }
                    Err(b) => c = b,
                }
            )*
        }};
    }
    known_component_table!(add_arms);

    // 未命中时 `c` 是最后一次 `Err(b) => c = b` 的结果；显式消费以表明
    // "丢弃的是未登记组件"（返回值已不需要它，见上方 `bool` 语义）。
    let _unregistered = c;
    false
}

/// **组件持久化（编码）**：按登记表把组件写成 `(稳定类型名, JSON 值)`。
///
/// 未登记类型 / 序列化失败（如匿名函数）⇒ **明确错误**，不静默丢组件。
pub fn serialize_component(
    c: &dyn std::any::Any,
) -> Result<(&'static str, serde_json::Value), crate::persist::PersistError> {
    macro_rules! ser_arms {
        (clone: $($t:ty),+ $(,)?; manual: $($m:ty => $ctor:expr),* $(,)?) => {{
            $(
                if let Some(v) = c.downcast_ref::<$t>() {
                    return serde_json::to_value(v)
                        .map(|data| (stringify!($t), data))
                        .map_err(|e| crate::persist::PersistError::SerializeComponent {
                            type_name: stringify!($t),
                            message: e.to_string(),
                        });
                }
            )+
            $(
                if let Some(v) = c.downcast_ref::<$m>() {
                    return serde_json::to_value(v)
                        .map(|data| (stringify!($m), data))
                        .map_err(|e| crate::persist::PersistError::SerializeComponent {
                            type_name: stringify!($m),
                            message: e.to_string(),
                        });
                }
            )*
        }};
    }
    known_component_table!(ser_arms);

    Err(crate::persist::PersistError::UnregisteredComponent(
        std::any::type_name_of_val(c).to_string(),
    ))
}

/// **组件持久化（解码）**：按稳定类型名重建组件。
///
/// 类型名未知 / 数据不合法 ⇒ **明确错误**（不降级成 `None` 组件）。
pub fn deserialize_component(
    type_name: &str,
    data: serde_json::Value,
) -> Result<Box<dyn Component>, crate::persist::PersistError> {
    macro_rules! de_arms {
        (clone: $($t:ty),+ $(,)?; manual: $($m:ty => $ctor:expr),* $(,)?) => {{
            $(
                if type_name == stringify!($t) {
                    return serde_json::from_value::<$t>(data)
                        .map(|v| Box::new(v) as Box<dyn Component>)
                        .map_err(|e| crate::persist::PersistError::DeserializeComponent {
                            type_name: type_name.to_string(),
                            message: e.to_string(),
                        });
                }
            )+
            $(
                if type_name == stringify!($m) {
                    return serde_json::from_value::<$m>(data)
                        .map(|v| Box::new(v) as Box<dyn Component>)
                        .map_err(|e| crate::persist::PersistError::DeserializeComponent {
                            type_name: type_name.to_string(),
                            message: e.to_string(),
                        });
                }
            )*
        }};
    }
    known_component_table!(de_arms);

    Err(crate::persist::PersistError::UnknownComponentType(
        type_name.to_string(),
    ))
}

impl Scene for WorldScene<'_> {
    fn spawn_dyn(&mut self, components: Components) -> Option<ObjectId> {
        let mut builder = EntityBuilder::new();
        for c in components {
            if !add_known(&mut builder, c) {
                // 未登记的类型：整个创建失败（不造半成品对象）。
                return None;
            }
        }
        Some(object_id_of(self.world.spawn(builder.build())))
    }

    fn despawn(&mut self, id: ObjectId) -> bool {
        match entity_of(id) {
            Some(e) => self.world.despawn(e).is_ok(),
            None => false,
        }
    }

    fn attach(&mut self, child: ObjectId, parent: Parent) -> bool {
        self.set(child, parent)
    }

    fn get_mut<T: 'static + Send + Sync>(&mut self, id: ObjectId) -> Option<&mut T> {
        let e = entity_of(id)?;
        self.world.query_one_mut::<&mut T>(e).ok()
    }

    fn set<T: Component>(&mut self, id: ObjectId, component: T) -> bool {
        let Some(e) = entity_of(id) else {
            return false;
        };
        // `insert_one` = 已有则覆盖、没有则新增（契约层的 `set` 语义）。
        self.world.insert_one(e, component).is_ok()
    }
}

impl SceneRead for crate::scene::Scene {
    fn contains(&self, id: ObjectId) -> bool {
        WorldView::new(self.world()).contains(id)
    }

    fn len(&self) -> usize {
        WorldView::new(self.world()).len()
    }

    fn for_each<T: 'static + Send + Sync>(&self, f: impl FnMut(ObjectId, &T)) {
        WorldView::new(self.world()).for_each::<T>(f);
    }

    fn for_each_pair<A: 'static + Send + Sync, B: 'static + Send + Sync>(
        &self,
        f: impl FnMut(ObjectId, &A, &B),
    ) {
        WorldView::new(self.world()).for_each_pair::<A, B>(f);
    }

    fn for_each_pair_opt<
        A: 'static + Send + Sync,
        B: 'static + Send + Sync,
        C: 'static + Send + Sync,
    >(
        &self,
        f: impl FnMut(ObjectId, &A, &B, Option<&C>),
    ) {
        WorldView::new(self.world()).for_each_pair_opt::<A, B, C>(f);
    }

    fn with<T: 'static + Send + Sync, R>(
        &self,
        id: ObjectId,
        f: impl FnOnce(&T) -> R,
    ) -> Option<R> {
        WorldView::new(self.world()).with::<T, R>(id, f)
    }
}

impl Scene for crate::scene::Scene {
    fn spawn_dyn(&mut self, components: Components) -> Option<ObjectId> {
        WorldScene::new(self.world_mut()).spawn_dyn(components)
    }

    fn despawn(&mut self, id: ObjectId) -> bool {
        WorldScene::new(self.world_mut()).despawn(id)
    }

    fn attach(&mut self, child: ObjectId, parent: Parent) -> bool {
        WorldScene::new(self.world_mut()).attach(child, parent)
    }

    fn get_mut<T: 'static + Send + Sync>(&mut self, id: ObjectId) -> Option<&mut T> {
        // 直接在 `&mut World` 上取（不能经临时 `WorldScene`：引用会逃出临时值）。
        let e = entity_of(id)?;
        self.world_mut().query_one_mut::<&mut T>(e).ok()
    }

    fn set<T: Component>(&mut self, id: ObjectId, component: T) -> bool {
        let Some(e) = entity_of(id) else {
            return false;
        };
        self.world_mut().insert_one(e, component).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene_with_circle() -> World {
        let mut world = World::new();
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            StrokeSolid::new(1.0, (0, 0, 0, 255)),
        ));
        world
    }

    /// 契约层只读路径：`for_each_pair` 能拿到图元 + 样式对（`record_scene` 依赖它）。
    #[test]
    fn for_each_pair_yields_primitive_and_style() {
        let mut world = scene_with_circle();
        let view = WorldView::new(&mut world);
        let mut hit = 0usize;
        let mut width = 0.0f32;
        view.for_each_pair::<Circle, StrokeSolid>(|_, c, s| {
            hit += 1;
            width = s.width;
            assert_eq!(c.radius, 1.0);
        });
        assert_eq!(hit, 1, "应命中 (Circle, StrokeSolid) 一对");
        assert_eq!(width, 1.0);
    }

    /// 写路径（M4-3b）：`set` 覆盖既有组件、`get_mut` 读回。
    #[test]
    fn set_and_get_mut_roundtrip() {
        let mut world = World::new();
        let id = {
            let mut ws = WorldScene::new(&mut world);
            ws.spawn_dyn(vec![Box::new(Circle::new((0.0, 0.0), 1.0))])
                .expect("Circle 已登记")
        };
        {
            let mut ws = WorldScene::new(&mut world);
            assert!(ws.set(id, Circle::new((5.0, 6.0), 2.0)));
            let c = ws.get_mut::<Circle>(id).unwrap();
            assert_eq!((c.center.x, c.center.y, c.radius), (5.0, 6.0, 2.0));
            // 不存在的句柄：写入失败而非 panic。
            assert!(!ws.set(ObjectId::DANGLING, Circle::new((0.0, 0.0), 1.0)));
        }
    }

    /// 登记表：未登记类型 ⇒ `spawn_dyn` 返回 `None`（不造半成品对象）。
    #[test]
    fn spawn_dyn_rejects_unregistered_component() {
        #[derive(Clone)]
        struct NotRegistered;
        let mut world = World::new();
        let mut ws = WorldScene::new(&mut world);
        assert!(ws.spawn_dyn(vec![Box::new(NotRegistered)]).is_none());
        assert_eq!(SceneRead::len(&ws), 0, "拒绝时必须不留下半成品实体");
    }

    // ─── 登记表双向化（本轮）────────────────────────────────────────────

    /// 反向 ①：27 个登记类型**逐个**有稳定名字；未登记 ⇒ `None`。
    ///
    /// ⚠️ 本用例是**覆盖度绊线**（新增类型时提醒补样例），不是注册表 ——
    /// 生产侧的登记表只有 `known_component_table!` 一处。
    #[test]
    fn known_type_name_reports_every_registered_type() {
        assert_eq!(known_type_name::<Circle>(), Some("Circle"));
        assert_eq!(known_type_name::<Line>(), Some("Line"));
        assert_eq!(known_type_name::<Path>(), Some("Path"));
        assert_eq!(known_type_name::<Rect>(), Some("Rect"));
        assert_eq!(known_type_name::<Arrow>(), Some("Arrow"));
        assert_eq!(known_type_name::<Transform>(), Some("Transform"));
        assert_eq!(known_type_name::<ZOrder>(), Some("ZOrder"));
        assert_eq!(known_type_name::<Name>(), Some("Name"));
        assert_eq!(known_type_name::<Group>(), Some("Group"));
        assert_eq!(known_type_name::<Visibility>(), Some("Visibility"));
        assert_eq!(known_type_name::<Opacity>(), Some("Opacity"));
        assert_eq!(known_type_name::<Camera2D>(), Some("Camera2D"));
        assert_eq!(known_type_name::<Camera3D>(), Some("Camera3D"));
        assert_eq!(known_type_name::<Projection>(), Some("Projection"));
        assert_eq!(known_type_name::<Framing2D>(), Some("Framing2D"));
        assert_eq!(known_type_name::<CameraMode>(), Some("CameraMode"));
        assert_eq!(known_type_name::<FpsView>(), Some("FpsView"));
        assert_eq!(known_type_name::<StrokeSolid>(), Some("StrokeSolid"));
        assert_eq!(known_type_name::<FillSolid>(), Some("FillSolid"));
        assert_eq!(known_type_name::<Axes2D>(), Some("Axes2D"));
        assert_eq!(known_type_name::<AxesLine>(), Some("AxesLine"));
        assert_eq!(known_type_name::<FunctionGraph>(), Some("FunctionGraph"));
        assert_eq!(known_type_name::<PhaseAnim>(), Some("PhaseAnim"));
        assert_eq!(known_type_name::<MorphShape>(), Some("MorphShape"));
        assert_eq!(
            known_type_name::<ContentTextureItem>(),
            Some("ContentTextureItem")
        );
        assert_eq!(known_type_name::<Reveal>(), Some("Reveal"));
        assert_eq!(known_type_name::<Parent>(), Some("Parent"));

        #[derive(Clone)]
        struct NotRegistered;
        assert_eq!(
            known_type_name::<NotRegistered>(),
            None,
            "未登记类型必须返回 None（与 spawn_dyn 同口径）"
        );
    }

    /// 反向 ②：27 个登记类型**逐个**可克隆（不能只测一两个）。
    ///
    /// `AxesLine` 未 derive `Clone` —— 由登记表的 `manual:` 段显式重构
    /// （**没有改它的定义**）。
    #[test]
    fn clone_known_covers_every_registered_type() {
        let samples: Vec<Box<dyn std::any::Any + Send + Sync>> = vec![
            Box::new(Circle::new((0.0, 0.0), 1.0)),
            Box::new(Line::new((0.0, 0.0), (1.0, 0.0))),
            Box::new(Path::new(xc_notion::Path2D::new())),
            Box::new(Rect::new((0.0, 0.0), (1.0, 1.0))),
            Box::new(Arrow::new((0.0, 0.0), (1.0, 0.0), 0.1, 0.1)),
            Box::new(Transform::default()),
            Box::new(ZOrder::new(1)),
            Box::new(Name::new("x")),
            Box::new(Group::new(GroupId::new(1))),
            Box::new(Visibility::default()),
            Box::new(Opacity::new(1.0)),
            Box::new(Camera2D::new(
                xc_notion::Vec2::ZERO,
                40.0,
                xc_notion::Rect2D::new(xc_notion::Vec2::ZERO, xc_notion::Vec2::new(800.0, 600.0)),
            )),
            Box::new(Camera3D::new(
                xc_notion::Vec3::ZERO,
                xc_notion::Vec3::ZERO,
                xc_notion::Vec3::Y,
                Projection::perspective(1.0, 1.0, 0.1, 100.0),
            )),
            Box::new(Projection::perspective(1.0, 1.0, 0.1, 100.0)),
            Box::new(Framing2D::default()),
            Box::new(CameraMode::default()),
            Box::new(FpsView::new(0.0, 0.0)),
            Box::new(StrokeSolid::new(1.0, (0, 0, 0, 255))),
            Box::new(FillSolid::new((0, 0, 0, 255))),
            Box::new(Axes2D::new((-1.0, 1.0), (-1.0, 1.0))),
            Box::new(AxesLine {
                owner: ObjectId::DANGLING,
            }),
            Box::new(FunctionGraph::new(
                xc_notion::FunctionX2Y::new(|x: f32| x),
                xc_notion::Sampler1D::new(0.0f32, 1.0, 1.0),
            )),
            Box::new(PhaseAnim::new(0.0, 1.0)),
            Box::new(MorphShape::new(
                xc_notion::Path2D::new(),
                xc_notion::Path2D::new(),
            )),
            Box::new(ContentTextureItem),
            Box::new(Reveal::new(xc_notion::Path2D::new())),
            Box::new(Parent(ObjectId::DANGLING)),
        ];
        assert_eq!(samples.len(), 27, "登记表当前应为 27 个类型");

        for (i, c) in samples.iter().enumerate() {
            let cloned =
                clone_known(c.as_ref()).unwrap_or_else(|| panic!("第 {i} 个已登记类型必须可克隆"));
            assert_eq!(
                cloned.as_ref().type_id(),
                c.as_ref().type_id(),
                "第 {i} 个类型克隆后 TypeId 必须一致"
            );
        }
    }

    /// 未登记类型 ⇒ `clone_known` 返回 `None`（不 panic）。
    #[test]
    fn clone_known_rejects_unregistered_type() {
        #[derive(Clone)]
        struct NotRegistered;
        assert!(clone_known(&NotRegistered).is_none());
    }

    /// 本地类型 `xcal_scene::Scene` 也实现了契约（孤儿规则合法落点之一）。
    #[test]
    fn scene_struct_implements_contract() {
        let mut scene = crate::scene::Scene::new();
        let id = scene
            .spawn_dyn(vec![Box::new(Circle::new((0.0, 0.0), 1.0))])
            .expect("Circle 已登记");
        assert!(scene.contains(id));
        assert_eq!(scene.len(), 1);
        assert!(scene.set(id, Circle::new((1.0, 1.0), 3.0)));
        assert_eq!(scene.get_mut::<Circle>(id).unwrap().radius, 3.0);
        let mut seen = 0;
        scene.for_each::<Circle>(|_, c| {
            seen += 1;
            assert_eq!(c.radius, 3.0);
        });
        assert_eq!(seen, 1);
        assert!(scene.despawn(id));
        assert_eq!(scene.len(), 0);
    }

    fn test_camera() -> Camera2D {
        Camera2D::new(
            xc_notion::Vec2::ZERO,
            40.0,
            xc_notion::Rect2D::new(xc_notion::Vec2::ZERO, xc_notion::Vec2::new(800.0, 600.0)),
        )
    }

    /// `for_each_pair_opt`：`C` 缺席传 `None`（普通对象），存在时给出（分层对象）。
    #[test]
    fn for_each_pair_opt_reports_missing_component() {
        let mut world = World::new();
        world.spawn((
            Circle::new((0.0, 0.0), 1.0),
            StrokeSolid::new(1.0, (0, 0, 0, 255)),
        ));
        world.spawn((
            Circle::new((1.0, 0.0), 1.0),
            StrokeSolid::new(1.0, (0, 0, 0, 255)),
            ZOrder::new(-100),
        ));
        let view = WorldView::new(&world);
        let mut zs = Vec::new();
        view.for_each_pair_opt::<Circle, StrokeSolid, ZOrder>(|_, _, _, z| zs.push(z.map(|z| z.0)));
        zs.sort();
        assert_eq!(zs, vec![None, Some(-100)]);
    }

    /// M4：`Axes2D::build` / `refresh` 走契约层写路径 —— 每帧清旧建新，
    /// 线段数量不累积，且网格线带 `ZOrder`（压在内容之下）。
    #[test]
    fn axes_refresh_rebuilds_without_accumulating() {
        let mut scene = crate::scene::Scene::new();
        let axes = Axes2D::new((-10.0, 10.0), (-10.0, 10.0));
        let handle = axes.build(&mut scene).expect("Axes2D 已登记");
        let camera = test_camera();

        let count_lines = |scene: &crate::scene::Scene| {
            let mut n = 0;
            scene.for_each::<AxesLine>(|_, _| n += 1);
            n
        };

        axes.refresh(&mut scene, handle, &camera);
        // 视口 800x600、scale 40 ⇒ 可见 x ∈ [-10,10]（21 条）、y ∈ [-7.5,7.5]（16 条）
        // ⇒ 37 条网格线 + 2 条坐标轴 = 39。
        assert_eq!(count_lines(&scene), 39);

        // 第二次刷新：旧的全部清掉（不累积）。
        axes.refresh(&mut scene, handle, &camera);
        assert_eq!(count_lines(&scene), 39);

        // 关闭网格 ⇒ 线段清空，容器对象仍在。
        axes.clear(&mut scene, handle);
        assert_eq!(count_lines(&scene), 0);
        assert!(scene.contains(handle));
    }

    /// M4-3b 消费方：`FunctionGraph::refresh` 用 `Scene::set` 覆盖既有 `Path`。
    #[test]
    fn function_graph_refresh_rewrites_path() {
        let mut scene = crate::scene::Scene::new();
        let graph = FunctionGraph::new(
            xc_notion::FunctionX2Y::new(|x: f32| x),
            xc_notion::Sampler1D::new(0.0f32, 1.0, 1.0),
        );
        let handle = graph
            .build(&mut scene, StrokeSolid::new(1.0, (0, 0, 0, 255)))
            .expect("FunctionGraph 已登记");

        assert!(graph.refresh(&mut scene, handle, 1.0));
        let path = scene.get_mut::<Path>(handle).expect("Path 组件在");
        let mut pts = Vec::new();
        for cmd in &path.0.commands {
            if let xc_notion::PathCommand::MoveTo(p) | xc_notion::PathCommand::LineTo(p) = cmd {
                pts.push(*p);
            }
        }
        // 相位 1.0 加在自变量上：x = {0,1} ⇒ y = {1,2}。
        assert_eq!(
            pts,
            vec![
                xc_notion::Vec2::new(0.0, 1.0),
                xc_notion::Vec2::new(1.0, 2.0)
            ]
        );

        // 句柄失效 ⇒ 写入失败（不 panic）。
        assert!(!graph.refresh(&mut scene, ObjectId::DANGLING, 0.0));
    }
}
