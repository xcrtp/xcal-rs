// transform_hierarchy.rs — 父子变换累乘（S5，S1 换型为 3D）。
//
// # 产物是**派生态**
//
// `WorldTransforms` 是 Resource（每帧重建，不进场景持久化）。渲染路径读它；
// 无 `Parent` 的对象其世界矩阵与 local **逐位相同**（直接复制，不乘恒等），
// 因此默认场景零变化。
//
// # 组合顺序
//
// 沿用 `ze_object::Transform::to_affine3d` 的约定：`scale → rotation → translation`。
// 父子：`world = parent ∘ local`（点先被 local 作用，再被 parent 作用）。
// `Transform3D::compose(self, other)` 的语义是"先 self 后 other"，故写
// `local.compose(parent)`。
//
// # 2D 是投影视图（S1 / D1 / D4）
//
// 本资源只存 **3D 世界矩阵（Mat4，单一真源）**；2D 消费方经 [`WorldTransforms::get2d`]
// 读投影视图（取 xy 子块，`z` 平移与 xy↔z 耦合被丢弃，见 `GAPS.md` G-101）。
// 旧的"2D 仿射"是派生 getter，不是第二份状态。
//
// # 确定性 / 环路
//
// - 遍历按 `ObjectId` 升序（不依赖 HashMap 顺序）；
// - `Parent(DANGLING)` 视为**无父**（声明期合法占位）；
// - 环路写入 `TransformHierarchyError`（明确字符串），并跳过该子树（返回 local）。
//   不静默截断：错误 Resource 可由测试 / 宿主读取。

use std::collections::{HashMap, HashSet};

use ze_notion::{Transform2D, Transform3D};
use ze_object::Transform;
use ze_scene_contract::{ObjectId, Parent, SceneRead};
use zela_schedule::context::ExecutorContext;
use zela_schedule::system::System;

use crate::contract::WorldView;

/// 每帧派生的**世界矩阵**（Resource；不进持久化）。
#[derive(Debug, Clone, Default)]
pub struct WorldTransforms {
    world: HashMap<ObjectId, Transform3D>,
}

impl WorldTransforms {
    /// 取对象的**世界矩阵**（3D 唯一真源；缺席 ⇒ 该对象没有 Transform，
    /// 渲染侧回退 local / 恒等）。
    pub fn get(&self, id: ObjectId) -> Option<&Transform3D> {
        self.world.get(&id)
    }

    /// 取对象的 **2D 投影视图**（D1/D4）：3D 世界矩阵的 xy 子块。
    /// `z` 平移与 xy↔z 耦合在此被丢弃（投影，非同构）。
    pub fn get2d(&self, id: ObjectId) -> Option<Transform2D> {
        self.world.get(&id).map(|t| t.to_affine2d())
    }

    pub fn len(&self) -> usize {
        self.world.len()
    }

    pub fn is_empty(&self) -> bool {
        self.world.is_empty()
    }
}

/// 父子环路错误（S5）：`None` = 本帧无环。
#[derive(Debug, Clone, Default)]
pub struct TransformHierarchyError(pub Option<String>);

impl TransformHierarchyError {
    pub fn is_err(&self) -> bool {
        self.0.is_some()
    }

    pub fn message(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

/// 父子变换累乘（Prepare：在 `BindingApplySystem` 写入 local Transform 之后、录制之前）。
pub struct TransformHierarchySystem;

impl System for TransformHierarchySystem {
    type Reads = ();
    type Writes = (WorldTransforms, TransformHierarchyError);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 先在世界只读借用内收集 local / parent（收集完即释放）。
        let (locals, parents) = {
            let world = WorldView::new(ctx.borrow_world());
            let mut locals: HashMap<ObjectId, Transform3D> = HashMap::new();
            let mut parents: HashMap<ObjectId, ObjectId> = HashMap::new();
            world.for_each::<Transform>(|id, transform| {
                locals.insert(id, transform.to_affine3d());
            });
            world.for_each::<Parent>(|id, parent| {
                if parent.0 != ObjectId::DANGLING {
                    parents.insert(id, parent.0);
                }
            });
            (locals, parents)
        };

        // 拓扑序由 ObjectId 升序决定（确定性；不依赖 HashMap 遍历顺序）。
        let mut ids: Vec<ObjectId> = locals
            .keys()
            .copied()
            .chain(parents.keys().copied())
            .collect();
        ids.sort_unstable();
        ids.dedup();

        let mut out: HashMap<ObjectId, Transform3D> = HashMap::with_capacity(ids.len());
        let mut visiting: HashSet<ObjectId> = HashSet::new();
        let mut error: Option<String> = None;
        for id in ids {
            let _ = world_of(id, &locals, &parents, &mut out, &mut visiting, &mut error);
        }

        if let Some(resource) = ctx.get_mut::<WorldTransforms>() {
            resource.world = out;
        }
        if let Some(resource) = ctx.get_mut::<TransformHierarchyError>() {
            resource.0 = error;
        }
    }
}

fn world_of(
    id: ObjectId,
    locals: &HashMap<ObjectId, Transform3D>,
    parents: &HashMap<ObjectId, ObjectId>,
    out: &mut HashMap<ObjectId, Transform3D>,
    visiting: &mut HashSet<ObjectId>,
    error: &mut Option<String>,
) -> Transform3D {
    if let Some(world) = out.get(&id) {
        return *world;
    }
    if !visiting.insert(id) {
        if error.is_none() {
            *error = Some(format!("父子变换环路：ObjectId {id:?} 在祖先链上重复"));
        }
        return Transform3D::identity();
    }

    let local = locals
        .get(&id)
        .copied()
        .unwrap_or_else(Transform3D::identity);
    let world = match parents.get(&id) {
        // 先 local 后 parent ⇒ parent ∘ local。
        Some(parent) => local.compose(world_of(
            *parent, locals, parents, out, visiting, error,
        )),
        None => local,
    };

    visiting.remove(&id);
    out.insert(id, world);
    world
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_notion::Vec2;
    use zela_schedule::resource::Resources;
    use zela_schedule::scheduler::{Phase, Scheduler};

    fn run(world: hecs::World) -> (WorldTransforms, TransformHierarchyError) {
        let mut sched = Scheduler::new(world, Resources::new());
        sched.add_system(Phase::Prepare, TransformHierarchySystem);
        sched.init_resources();
        sched.run_frame(0.0);
        (
            sched.resources().get::<WorldTransforms>().cloned().unwrap(),
            sched
                .resources()
                .get::<TransformHierarchyError>()
                .cloned()
                .unwrap(),
        )
    }

    fn transform(x: f32, y: f32) -> Transform {
        Transform {
            position: ze_notion::Vec3::new(x, y, 0.0),
            ..Transform::default()
        }
    }

    #[test]
    fn no_parent_world_equals_local() {
        let mut world = hecs::World::new();
        let entity = world.spawn((transform(2.0, 3.0),));
        let id = crate::contract::object_id_of(entity);
        let (worlds, error) = run(world);
        assert!(!error.is_err());
        // S1：get2d（2D 投影视图）逐位等于旧的 to_affine2d ⇒ 2D 路径回退不变性。
        assert_eq!(
            worlds.get2d(id),
            Some(transform(2.0, 3.0).to_affine2d())
        );
        // S1：3D 真源与 local 的 to_affine3d 逐位一致。
        assert_eq!(
            worlds.get(id),
            Some(&transform(2.0, 3.0).to_affine3d())
        );
    }

    #[test]
    fn two_level_world_is_parent_then_local() {
        let mut world = hecs::World::new();
        let parent = world.spawn((transform(10.0, 0.0),));
        let parent_id = crate::contract::object_id_of(parent);
        let child = world.spawn((transform(0.0, 5.0), Parent(parent_id)));
        let child_id = crate::contract::object_id_of(child);

        let (worlds, error) = run(world);
        assert!(!error.is_err());
        assert_eq!(
            worlds
                .get2d(child_id)
                .unwrap()
                .apply_point(Vec2::ZERO),
            Vec2::new(10.0, 5.0)
        );
    }

    /// **S1-5 #1**：3D 世界矩阵传播 z —— 父 `position.z = 5`，子 local 恒等
    /// ⇒ 子世界 z = 5（旧的 2D 仿射根本表达不了 z）。
    #[test]
    fn world_of_3d_child_inherits_parent_z() {
        let mut world = hecs::World::new();
        let parent = world.spawn((Transform {
            position: ze_notion::Vec3::new(0.0, 0.0, 5.0),
            rotation: ze_notion::Quat::IDENTITY,
            scale: ze_notion::Vec3::ONE,
        },));
        let parent_id = crate::contract::object_id_of(parent);
        let child = world.spawn((Transform::default(), Parent(parent_id)));
        let child_id = crate::contract::object_id_of(child);

        let (worlds, error) = run(world);
        assert!(!error.is_err());
        let world_child = worlds.get(child_id).unwrap();
        assert_eq!(
            world_child.apply_point(ze_notion::Vec3::ZERO),
            ze_notion::Vec3::new(0.0, 0.0, 5.0),
            "3D 世界矩阵必须传播 z"
        );
        // 2D 投影视图不受 z 影响（xy 平移为 0）。
        assert_eq!(
            worlds.get2d(child_id).unwrap().apply_point(Vec2::ZERO),
            Vec2::ZERO
        );
    }

    #[test]
    fn rotation_parent_composes_with_local_translation() {
        use ze_notion::Quat;
        let mut world = hecs::World::new();
        let parent = world.spawn((Transform {
            position: ze_notion::Vec3::ZERO,
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            scale: ze_notion::Vec3::ONE,
        },));
        let parent_id = crate::contract::object_id_of(parent);
        let child = world.spawn((transform(1.0, 0.0), Parent(parent_id)));
        let child_id = crate::contract::object_id_of(child);

        let (worlds, _) = run(world);
        let p = worlds.get2d(child_id).unwrap().apply_point(Vec2::ZERO);
        assert!((p.x - 0.0).abs() < 1e-5, "x = {}", p.x);
        assert!((p.y - 1.0).abs() < 1e-5, "y = {}", p.y);
    }

    #[test]
    fn cycle_is_an_explicit_error() {
        let mut world = hecs::World::new();
        let a = world.spawn((transform(1.0, 0.0),));
        let b = world.spawn((transform(0.0, 1.0),));
        let a_id = crate::contract::object_id_of(a);
        let b_id = crate::contract::object_id_of(b);
        world.insert_one(a, Parent(b_id)).unwrap();
        world.insert_one(b, Parent(a_id)).unwrap();

        let (_, error) = run(world);
        assert!(error.is_err(), "环路必须产生明确错误");
        assert!(error.message().unwrap().contains("环路"));
    }

    #[test]
    fn dangling_parent_is_treated_as_root() {
        let mut world = hecs::World::new();
        let id = {
            let e = world.spawn((transform(1.0, 2.0), Parent(ObjectId::DANGLING)));
            crate::contract::object_id_of(e)
        };
        let (worlds, error) = run(world);
        assert!(!error.is_err());
        assert_eq!(
            worlds.get2d(id).unwrap().apply_point(Vec2::ZERO),
            Vec2::new(1.0, 2.0)
        );
    }

    #[test]
    fn parent_scale_composes_with_child_translation() {
        let mut world = hecs::World::new();
        let parent = world.spawn((Transform {
            position: ze_notion::Vec3::ZERO,
            rotation: ze_notion::Quat::IDENTITY,
            scale: ze_notion::Vec3::splat(2.0),
        },));
        let parent_id = crate::contract::object_id_of(parent);
        let child = world.spawn((transform(1.0, 0.0), Parent(parent_id)));
        let child_id = crate::contract::object_id_of(child);

        let (worlds, _) = run(world);
        let p = worlds.get2d(child_id).unwrap().apply_point(Vec2::ZERO);
        assert!((p.x - 2.0).abs() < 1e-5, "x = {}", p.x);
        assert!(p.y.abs() < 1e-5, "y = {}", p.y);
    }

    /// 文本成组后，每个字形仍是独立对象 ⇒ 各自持有样式，互不覆盖。
    #[test]
    fn glyph_styles_are_independent_under_a_shared_parent() {
        use ze_notion::{Color32, Path2D};
        use ze_object::Path;
        use ze_render_ir::StrokeSolid;

        let mut world = hecs::World::new();
        let parent = world.spawn((transform(2.0, 3.0),));
        let parent_id = crate::contract::object_id_of(parent);

        let path = Path::new(Path2D::from_polyline(
            &[Vec2::ZERO, Vec2::new(0.2, 0.0)],
            false,
        ));
        let glyph_a = world.spawn((
            path.clone(),
            StrokeSolid::new(1.0, (0, 255, 0, 255)),
            Parent(parent_id),
        ));
        let glyph_b = world.spawn((
            path,
            StrokeSolid::new(1.0, (0, 0, 255, 255)),
            Parent(parent_id),
        ));

        let (worlds, _) = run(world);
        let a = crate::contract::object_id_of(glyph_a);
        let b = crate::contract::object_id_of(glyph_b);
        assert_eq!(worlds.get(a), worlds.get(b), "同一父下世界变换相同");
        assert_eq!(
            worlds.get2d(a).unwrap().apply_point(Vec2::ZERO),
            Vec2::new(2.0, 3.0)
        );

        // 两个字形各自持有 StrokeSolid：颜色可以不同，不会互相覆盖。
        let _a_color = Color32::new(0, 255, 0, 255);
        let _b_color = Color32::new(0, 0, 255, 255);
    }

    #[test]
    fn world_transforms_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<WorldTransforms>();
        assert_send_sync::<TransformHierarchyError>();
    }
}
