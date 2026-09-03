// system.rs — BindingApplySystem + 等价性测试
//
// 把时钟树 + 曲线 应用到 hecs 组件。SceneSync 阶段。
// 纯 Resource 读、组件（ctx.world）写。
// 等价性单测：新链路 vs 旧 AnimationApplySystem 公式，误差 < 1e-5。

use xc_time::ClockForest;
use xcal_core::context::ExecutorContext;
use xcal_core::scene::components::{Camera3D, Circle, PhaseAnim};
use xcal_core::system::System;
use xcal_core::TimeState;

use crate::animation::binding::{BindingTarget, BindingTable};

/// 遍历 BindingTable，按 `BindingTarget` 把 `Curve::sample(forest.eval_local(clock, t))` 写入组件字段。
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
            let value = b.curve.sample(lt);
            self.apply_target(&b.target, value, &mut ctx.world);
        }
    }
}

impl BindingApplySystem {
    fn apply_target(&self, target: &BindingTarget, value: xc_time::AnimValue, world: &mut hecs::World) {
        match target {
            BindingTarget::CircleRadius(entity) => {
                if let Ok(mut circle) = world.get::<&mut Circle>(*entity) {
                    circle.radius = value.scalar();
                }
            }
            BindingTarget::Phase(entity) => {
                if let Ok(mut phase) = world.get::<&mut PhaseAnim>(*entity) {
                    phase.phase = value.scalar();
                }
            }
            BindingTarget::CameraEye(entity) => {
                if let Ok(mut camera) = world.get::<&mut Camera3D>(*entity) {
                    camera.eye = value.vec3();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BindingApplySystem;
    use crate::animation::binding::{Binding, BindingTarget, BindingTable, SeekKind};
    use crate::resource::Resources;
    use crate::scene::components::Circle;
    use crate::scheduler::{Phase, Scheduler};
    use crate::TimeState;
    use glam::Vec3;
    use hecs::World;
    use xc_time::{ClockForest as XcClockForest, ClockNode, Curve};

    fn vec2(x: f32, y: f32) -> crate::types::Vec2 {
        crate::types::Vec2::new(glam::Vec2::new(x, y))
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

    /// 真等价性测试：新 Curve::Orbit vs 旧 OrbitAnim 公式。
    /// t ∈ [0, 10] 采 1000 点逐点比对（不只是"轨迹是圆"，起点也必须一致）。
    #[test]
    fn equivalence_orbit_matches_legacy_orbit_anim() {
        use xcal_core::scene::components::{Camera3D, OrbitAnim, Projection};

        let mut forest = XcClockForest::new();
        let root = forest.insert(ClockNode::new(None)).unwrap();

        let curve = Curve::Orbit {
            radius: 5.0,
            axis: Vec3::Y,
            speed: 0.5,
            initial_angle: 0.0,
        };
        let orbit = OrbitAnim::new(0.5, 5.0, Vec3::Y, 0.0);

        for i in 0..1000 {
            let t_global = (i as f64) * 0.01; // t ∈ [0, 10)
            let new_v = curve.sample(forest.eval_local(root, t_global)).vec3();

            // 旧链路：OrbitAnim.apply 写入 eye；target=0 → eye 即轨道偏移。
            let mut cam = Camera3D::new(
                Vec3::ZERO,
                Vec3::ZERO,
                Vec3::Y,
                Projection::perspective(1.0, 1.0, 0.1, 100.0),
            );
            orbit.apply(t_global as f32, &mut cam);
            let old_v = cam.eye;

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
            target: BindingTarget::CircleRadius(entity),
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
