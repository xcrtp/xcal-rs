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

        for b in &table.bindings {
            let lt = forest.eval_local(b.clock, t_global);
            let value: AnimValue = b.curve.sample(lt);
            // 目标自己负责"类型匹配 + 写组件"（不符则跳过本帧，不 panic）。
            b.target.apply(&mut ctx.world, &value);
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
}
