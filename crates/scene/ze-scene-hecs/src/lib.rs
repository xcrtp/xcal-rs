// ze-scene-hecs — 契约层 `SceneRead` 的 **hecs 只读适配器**（B3-1 收尾）
//
// # 为什么需要独立 crate
// 渲染录制（`zela-render::record_systems`）运行在调度器的 `ExecutorContext` 上，
// 拿到的是 `&hecs::World`；它需要把它接成 [`ze_scene_contract::SceneRead`]。
// 但：
// - `ze-scene-contract` 的硬约束**禁止出现 hecs**；
// - 渲染层不应依赖**场景 ECS 实现**（`zela-scene-ecs`：组件登记表 / 系统 / 生命周期）。
//
// ⇒ 把这段唯一的 hecs 接触面（只读 + 身份互转）收在这里，两层都只依赖本 crate。
// 写适配器（`WorldScene`）仍留在 ECS 实现层 —— 它需要组件登记表。

use hecs::{Entity, World};

use ze_scene_contract::{ObjectId, SceneRead};

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

#[cfg(test)]
mod tests {
    use super::*;

    /// 身份互转是**位模式直通**：往返逐位相同；非法位模式显式 `None`。
    #[test]
    fn object_id_and_entity_roundtrip() {
        let mut world = World::new();
        let e = world.spawn((1u32,));
        let id = object_id_of(e);
        assert_eq!(entity_of(id), Some(e));
        assert_eq!(id, object_id_of(e));
    }

    /// 只读适配器能看到组件，且缺席组件 ⇒ `with` 返回 `None`（不 panic）。
    #[test]
    fn world_view_reads_components() {
        let mut world = World::new();
        let e = world.spawn((7u32,));
        let id = object_id_of(e);
        let view = WorldView::new(&world);
        assert!(view.contains(id));
        assert_eq!(view.with::<u32, _>(id, |v| *v), Some(7));
        assert_eq!(view.with::<f32, _>(id, |v| *v), None);
        assert_eq!(view.len(), 1);
        let mut seen = 0;
        view.for_each::<u32>(|_, _| seen += 1);
        assert_eq!(seen, 1);
    }
}
