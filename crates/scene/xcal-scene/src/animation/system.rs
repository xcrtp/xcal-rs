// system.rs — BindingApplySystem + 等价性测试
//
// 把时钟树 + 曲线 应用到 hecs 组件。SceneSync 阶段。
// 纯 Resource 读、组件（ctx.world）写。
// 等价性单测：新链路 vs 旧 AnimationApplySystem 公式，误差 < 1e-5。
//
// P0：目标由 `BindingTarget` 枚举改为 `Arc<dyn Target>` —— 本 System 只做
// "遍历 + 求值 + 派发"，**不认识任何具体目标**（新增目标零 System 改动）。

use xc_time::{AnimValue, ClockForest};
use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;
use xcal_scene::TimeState;

use crate::animation::binding::BindingTable;
use crate::contract::WorldScene;

/// 遍历 BindingTable，按 `Binding.target` 把 `Curve::sample(forest.eval_local(clock, t))` 写入组件字段。
pub struct BindingApplySystem;

impl System for BindingApplySystem {
    type Reads = (TimeState, ClockForest, BindingTable);
    type Writes = ();

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 先从 ctx.get 取出所有数据（&mut ctx 的借用到此释放），
        // 再独立用 ctx.world 做 &mut query。
        let t_global = ctx.get::<TimeState>().map(|s| s.elapsed as f64).unwrap_or(0.0);
        let forest = match ctx.get::<ClockForest>().cloned() {
            Some(f) => f,
            None => return,
        };
        let table = match ctx.get::<BindingTable>().cloned() {
            Some(t) => t,
            None => return,
        };

        // 目标层**不认识 hecs**：把世界包成契约层适配器再派发，
        // `apply` 内部只走契约方法（`Scene::get_mut`）。
        let mut scene = WorldScene::new(&mut *ctx.world);
        for b in &table.bindings {
            let lt = forest.eval_local(b.clock, t_global);
            let value: AnimValue = b.curve.sample(lt);
            // 目标自己负责"类型匹配 + 写组件"（不符则跳过本帧，不 panic）。
            b.target.apply(&mut scene, &value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BindingApplySystem;
    use crate::animation::binding::{Binding, BindingTable, SeekKind};
    use crate::animation::target::CircleRadius;
    use xcal_schedule::resource::Resources;
    use xc_object::Circle;
    use xcal_schedule::scheduler::{Phase, Scheduler};
    use crate::TimeState;
    use hecs::World;
    use xc_notion::Vec3;
    use xc_time::{ClockForest as XcClockForest, ClockNode, Curve};

    /// 场景向量（M2.5 后 `xcal_scene::types::Vec2` 就是 `xc_notion::Vec2`（= glam）的转发，
    /// 图元对象用的也是同一类型 —— 原来的 `vec2` / `obj_vec2` 两个包装已被消灭）。
    fn vec2(x: f32, y: f32) -> xc_notion::Vec2 {
        xc_notion::Vec2::new(x, y)
    }

    /// 等价性测试：旧公式 vs 新链路，1000 个采样点最大误差 < 1e-5。
    #[test]
    fn equivalence_sin_radius() {
        // 旧：radius = base + amp * sin(t * omega)
        let base = 1.0f32;
        let amp = 0.4f32;
        let omega = 2.0f32;

        let mut forest = XcClockForest::new();
        let root = forest.insert(ClockNode::new(None)).unwrap();

        let curve = Curve::Sin { base, amp, omega };

        for i in 0..1000 {
            let t_global = (i as f64) * 0.01;
            let lt = forest.eval_local(root, t_global);
            let new_v = curve.sample(lt).scalar();
            let old_v = base + amp * (t_global as f32 * omega).sin();
            assert!(
                (new_v - old_v).abs() < 1e-5,
                "t={t_global}: new={new_v}, old={old_v}"
            );
        }
    }

    #[test]
    fn equivalence_phase_linear() {
        let omega = 1.5f32;
        let mut forest = XcClockForest::new();
        let root = forest.insert(ClockNode::new(None)).unwrap();
        let curve = Curve::Phase { omega };

        for t_global in [0.0f64, 1.0, 2.5, -3.0, 500.0] {
            let lt = forest.eval_local(root, t_global);
            let new_v = curve.sample(lt).scalar();
            let old_v = omega * t_global as f32;
            assert!((new_v - old_v).abs() < 1e-5);
        }
    }

    /// 真等价性测试：新 Curve::Orbit vs 旧 OrbitAnim 公式（参考实现内联保留）。
    /// t ∈ [0, 10] 采 1000 点逐点比对（不只是"轨迹是圆"，起点也必须一致）。
    #[test]
    fn equivalence_orbit_matches_legacy_orbit_anim() {
        // 参考（旧 OrbitAnim）实现：
        // eye_offset = Quat::from_axis_angle(axis, angle) * Vec3::new(0, 0, radius)
        fn legacy_orbit_offset(
            radius: f32,
            axis: Vec3,
            speed: f32,
            initial_angle: f32,
            t: f64,
        ) -> Vec3 {
            let angle = initial_angle + (t as f32) * speed;
            let axis = if axis.length_squared() > f32::EPSILON {
                axis.normalize()
            } else {
                Vec3::Y
            };
            xc_notion::Quat::from_axis_angle(axis, angle) * Vec3::new(0.0, 0.0, radius)
        }

        let mut forest = XcClockForest::new();
        let root = forest.insert(ClockNode::new(None)).unwrap();

        let curve = Curve::Orbit {
            radius: 5.0,
            axis: Vec3::Y,
            speed: 0.5,
            initial_angle: 0.0,
        };

        for i in 0..1000 {
            let t_global = (i as f64) * 0.01; // t ∈ [0, 10)
            let new_v = curve.sample(forest.eval_local(root, t_global)).vec3();
            let old_v = legacy_orbit_offset(5.0, Vec3::Y, 0.5, 0.0, t_global);

            assert!(
                (new_v - old_v).length() < 1e-5,
                "t={t_global}: new={new_v}, old={old_v}"
            );
        }
    }

    #[test]
    fn binding_apply_writes_circle_radius_via_scheduler() {
        let mut world = World::new();
        let entity = world.spawn((Circle::new(vec2(0.0, 0.0), 1.0),));

        let mut forest = XcClockForest::new();
        let root = forest.insert(ClockNode::new(None)).unwrap();

        let mut table = BindingTable::new();
        table.add(Binding {
            clock: root,
            curve: Curve::Sin {
                base: 1.0,
                amp: 0.5,
                omega: 1.0,
            },
            target: std::sync::Arc::new(CircleRadius {
                id: crate::contract::object_id_of(entity),
            }),
            seek_kind: SeekKind::O1,
        });

        let mut resources = Resources::new();
        resources.insert(TimeState { elapsed: 0.0 });
        resources.insert(forest);
        resources.insert(table);

        let mut scheduler = Scheduler::new(world, resources);
        scheduler.add_system(Phase::SceneSync, BindingApplySystem);
        scheduler.run_frame(0.1);

        let radius = scheduler.world().get::<&Circle>(entity).unwrap().radius;
        // t=0, sin(0)=0 → radius = 1.0 + 0.5 * 0 = 1.0
        assert!((radius - 1.0).abs() < 1e-5);
    }

    /// **T1 验收 5**：`Timeline` 驱动 `TransformValue` —— 段内插值、段外冻结（`Hold`）。
    #[test]
    fn timeline_drives_transform_value_with_hold() {
        use crate::animation::target::TransformValue;
        use crate::animation::timeline::Timeline;
        use xc_object::Transform;
        use xc_time::{AnimValue, Curve, Interp};

        let mut world = World::new();
        let entity = world.spawn((Transform::default(),));
        let id = crate::contract::object_id_of(entity);

        let curve = Curve::Keyframes {
            keys: vec![
                (0.0, AnimValue::Vec2(vec2(0.0, 0.0))),
                (2.0, AnimValue::Vec2(vec2(2.0, 0.0))),
            ],
            interpolation: Interp::Linear,
        };
        let mut tl = Timeline::new();
        tl.play(curve, TransformValue { id }, 2.0);

        let mut forest = XcClockForest::new();
        let mut table = BindingTable::new();
        tl.compile(&mut forest, &mut table);

        let mut resources = Resources::new();
        resources.insert(TimeState { elapsed: 0.0 });
        resources.insert(forest);
        resources.insert(table);

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::SceneSync, BindingApplySystem);
        sched.init_resources();

        let x_at = |sched: &mut Scheduler, t: f32| -> f32 {
            sched.resources_mut().get_mut::<TimeState>().unwrap().elapsed = t;
            sched.run_frame(0.0);
            sched.world().get::<&Transform>(entity).unwrap().position.x
        };
        assert_eq!(x_at(&mut sched, 0.0), 0.0);
        assert!((x_at(&mut sched, 1.0) - 1.0).abs() < 1e-4, "段内线性插值");
        assert!((x_at(&mut sched, 2.0) - 2.0).abs() < 1e-4, "段末到终值");
        assert!((x_at(&mut sched, 5.0) - 2.0).abs() < 1e-6, "段外冻结（Hold）");
    }

    /// **改动 1 的行为等价证据**：同一场景里八个 target 各一条绑定，`BindingApplySystem`
    /// 跑一帧后各组件值 = 曲线在 `t = 0.5` 的精确取值（`Keyframes` 线性中点）。
    ///
    /// 这钉住的是"apply 改成契约适配器后，写入落点与字段逐位不变"。
    #[test]
    fn all_eight_targets_write_expected_values_in_one_frame() {
        use crate::animation::target::{
            Camera2DFraming, CameraEye, MorphT, OpacityValue, Phase as PhaseTarget, RevealRange,
            TransformValue,
        };
        use crate::scene::components::{MorphShape, PhaseAnim, Reveal};
        use std::sync::Arc;
        use xc_object::{Camera3D, Framing2D, Projection, Transform};
        use xc_render_ir::Opacity;
        use xc_time::{AnimValue, Interp};

        fn kf(from: AnimValue, to: AnimValue) -> Curve {
            Curve::Keyframes {
                keys: vec![(0.0, from), (1.0, to)],
                interpolation: Interp::Linear,
            }
        }

        let mut world = World::new();
        let circle = world.spawn((Circle::new((0.0, 0.0), 1.0),));
        let phase = world.spawn((PhaseAnim::new(0.0, 1.0),));
        let camera3d = world.spawn((
            Camera3D::new(
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::Y,
                Projection::perspective(1.0, 1.0, 0.1, 100.0),
            ),
        ));
        let morph = world.spawn((MorphShape::new(
            xc_notion::Path2D::new(),
            xc_notion::Path2D::new(),
        ),));
        let camera2d = world.spawn((Framing2D::default(),));
        let reveal = world.spawn((Reveal::new(xc_notion::Path2D::new()),));
        let opacity = world.spawn((Opacity::new(1.0),));
        let transform = world.spawn((Transform::default(),));

        let mut forest = XcClockForest::new();
        let root = forest.insert(ClockNode::new(None)).unwrap();
        let mut table = BindingTable::new();
        let mut bind = |curve: Curve, target: Arc<dyn crate::animation::Target>| {
            table.add(Binding {
                clock: root,
                curve,
                target,
                seek_kind: SeekKind::O1,
            });
        };
        bind(
            kf(AnimValue::Scalar(0.0), AnimValue::Scalar(1.0)),
            Arc::new(CircleRadius {
                id: crate::contract::object_id_of(circle),
            }),
        );
        bind(
            kf(AnimValue::Scalar(0.0), AnimValue::Scalar(2.0)),
            Arc::new(PhaseTarget {
                id: crate::contract::object_id_of(phase),
            }),
        );
        bind(
            kf(
                AnimValue::Vec3(Vec3::ZERO),
                AnimValue::Vec3(Vec3::new(1.0, 2.0, 3.0)),
            ),
            Arc::new(CameraEye {
                id: crate::contract::object_id_of(camera3d),
            }),
        );
        bind(
            kf(AnimValue::Scalar(0.0), AnimValue::Scalar(1.0)),
            Arc::new(MorphT {
                id: crate::contract::object_id_of(morph),
            }),
        );
        // 相机取景：scale（Scalar）与 center（Vec2）各一条绑定。
        bind(
            kf(AnimValue::Scalar(40.0), AnimValue::Scalar(80.0)),
            Arc::new(Camera2DFraming {
                id: crate::contract::object_id_of(camera2d),
            }),
        );
        bind(
            kf(
                AnimValue::Vec2(vec2(0.0, 0.0)),
                AnimValue::Vec2(vec2(2.0, 4.0)),
            ),
            Arc::new(Camera2DFraming {
                id: crate::contract::object_id_of(camera2d),
            }),
        );
        // 书写区间：end（Scalar）与 (start, end)（Vec2）各一条。
        bind(
            kf(AnimValue::Scalar(0.0), AnimValue::Scalar(1.0)),
            Arc::new(RevealRange {
                id: crate::contract::object_id_of(reveal),
            }),
        );
        bind(
            kf(
                AnimValue::Vec2(vec2(0.0, 0.0)),
                AnimValue::Vec2(vec2(0.5, 1.0)),
            ),
            Arc::new(RevealRange {
                id: crate::contract::object_id_of(reveal),
            }),
        );
        bind(
            kf(AnimValue::Scalar(1.0), AnimValue::Scalar(0.0)),
            Arc::new(OpacityValue {
                id: crate::contract::object_id_of(opacity),
            }),
        );
        bind(
            kf(
                AnimValue::Vec2(vec2(0.0, 0.0)),
                AnimValue::Vec2(vec2(2.0, 4.0)),
            ),
            Arc::new(TransformValue {
                id: crate::contract::object_id_of(transform),
            }),
        );

        let mut resources = Resources::new();
        resources.insert(TimeState { elapsed: 0.5 });
        resources.insert(forest);
        resources.insert(table);

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::SceneSync, BindingApplySystem);
        sched.init_resources();
        sched.run_frame(0.0);

        let w = sched.world();
        assert_eq!(w.get::<&Circle>(circle).unwrap().radius, 0.5);
        assert_eq!(w.get::<&PhaseAnim>(phase).unwrap().phase, 1.0);
        assert_eq!(w.get::<&Camera3D>(camera3d).unwrap().eye, Vec3::new(0.5, 1.0, 1.5));
        assert_eq!(w.get::<&MorphShape>(morph).unwrap().t, 0.5);
        {
            let f = w.get::<&Framing2D>(camera2d).unwrap();
            assert_eq!(f.scale, 60.0);
            assert_eq!(f.center, vec2(1.0, 2.0));
        }
        {
            let r = w.get::<&Reveal>(reveal).unwrap();
            assert_eq!((r.start, r.end), (0.25, 0.5));
        }
        assert_eq!(w.get::<&Opacity>(opacity).unwrap().0, 0.5);
        {
            let t = w.get::<&Transform>(transform).unwrap();
            assert_eq!(t.position, Vec3::new(1.0, 2.0, 0.0));
            assert_eq!(t.scale, Vec3::ONE);
        }
    }
}
