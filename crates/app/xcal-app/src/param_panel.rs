// param_panel.rs — 参数面板事件消费（LogicUpdate 阶段）
//
// B 阶段（UI 解耦）后本模块不含任何 egui 类型：
// - 面板 chrome（滑块绘制）在 xcal-egui::host::draw_param_panel，控件交互以
//   SceneEvent::Slider（WidgetId 标识）写入 InputEvents；
// - 本 System 只消费 Slider 事件并按 (curve, target) 类型匹配写回 BindingTable
//   （与 A5 一致，不依赖插入顺序）——可 headless 测试。
//
// 这是节点编辑器的简化版，后续替换为完整节点图（xc-node-schema + xcal-node-egui）
// 时上层逻辑不动。

use xcal_core::animation::{BindingTable, BindingTarget};
use xcal_core::context::ExecutorContext;
use xcal_core::domain::{ids, InputEvents, SceneEvent, WidgetId};
use xcal_core::system::System;
use xcal_core::Curve;

pub struct ParamPanelSystem;
impl System for ParamPanelSystem {
    type Reads = (InputEvents,);
    type Writes = (BindingTable,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let events: Vec<SceneEvent> = match ctx.get::<InputEvents>() {
            Some(e) => e.iter().copied().collect(),
            None => return,
        };
        // 只关心 Slider 事件。
        let sliders: Vec<(WidgetId, f32)> = events
            .into_iter()
            .filter_map(|e| match e {
                SceneEvent::Slider { id, value } => Some((id, value)),
                _ => None,
            })
            .collect();
        if sliders.is_empty() {
            return;
        }

        // 克隆出 BindingTable 修改，避免同时持有 &mut ctx 和 &mut table。
        let mut table = ctx.get::<BindingTable>().cloned().unwrap_or_default();

        for binding in table.bindings.iter_mut() {
            for (id, value) in &sliders {
                match (*id, &mut binding.curve, &binding.target) {
                    (
                        ids::CIRCLE_BASE,
                        Curve::Sin { base, .. },
                        BindingTarget::CircleRadius(_),
                    ) => *base = *value,
                    (
                        ids::CIRCLE_AMP,
                        Curve::Sin { amp, .. },
                        BindingTarget::CircleRadius(_),
                    ) => *amp = *value,
                    (
                        ids::CIRCLE_OMEGA,
                        Curve::Sin { omega, .. },
                        BindingTarget::CircleRadius(_),
                    ) => *omega = *value,
                    (ids::PHASE_OMEGA, Curve::Phase { omega }, BindingTarget::Phase(_)) => {
                        *omega = *value
                    }
                    (
                        ids::ORBIT_RADIUS,
                        Curve::Orbit { radius, .. },
                        BindingTarget::CameraEye(_),
                    ) => *radius = *value,
                    (
                        ids::ORBIT_SPEED,
                        Curve::Orbit { speed, .. },
                        BindingTarget::CameraEye(_),
                    ) => *speed = *value,
                    (
                        ids::ORBIT_ANGLE,
                        Curve::Orbit { initial_angle, .. },
                        BindingTarget::CameraEye(_),
                    ) => *initial_angle = *value,
                    _ => {}
                }
            }
        }

        // 写回
        if let Some(t) = ctx.get_mut::<BindingTable>() {
            *t = table;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_core::animation::{Binding, SeekKind};
    use xcal_core::resource::Resources;
    use xcal_core::scheduler::{Phase, Scheduler};
    use xcal_core::ClockId;

    /// headless：Slider 事件按 WidgetId 精确写入对应绑定字段，顺序无关。
    #[test]
    fn slider_events_patch_binding_fields_by_widget_id() {
        let mut table = BindingTable::new();
        let sin_entity = hecs::Entity::DANGLING;
        let orbit_entity = hecs::Entity::DANGLING;
        table.add(Binding {
            clock: ClockId(0),
            curve: Curve::Sin {
                base: 1.0,
                amp: 0.4,
                omega: 2.0,
            },
            target: BindingTarget::CircleRadius(sin_entity),
            seek_kind: SeekKind::O1,
        });
        table.add(Binding {
            clock: ClockId(1),
            curve: Curve::Orbit {
                radius: 5.0,
                axis: glam::Vec3::Y,
                speed: 0.5,
                initial_angle: 0.0,
            },
            target: BindingTarget::CameraEye(orbit_entity),
            seek_kind: SeekKind::O1,
        });

        let mut resources = Resources::new();
        resources.insert(table);
        resources.insert(InputEvents::from_events(vec![
            SceneEvent::Slider {
                id: ids::CIRCLE_AMP,
                value: 0.8,
            },
            SceneEvent::Slider {
                id: ids::ORBIT_RADIUS,
                value: 12.0,
            },
        ]));

        let mut sched = Scheduler::new(hecs::World::new(), resources);
        sched.add_system(Phase::LogicUpdate, ParamPanelSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let table = sched.resources().get::<BindingTable>().unwrap();
        for binding in &table.bindings {
            match (&binding.curve, &binding.target) {
                (Curve::Sin { amp, base, omega }, BindingTarget::CircleRadius(_)) => {
                    assert!((amp - 0.8).abs() < 1e-5, "amp = {amp}");
                    assert!((base - 1.0).abs() < 1e-5, "base 不应被无关事件改动");
                    assert!((omega - 2.0).abs() < 1e-5);
                }
                (Curve::Orbit { radius, .. }, BindingTarget::CameraEye(_)) => {
                    assert!((radius - 12.0).abs() < 1e-5, "radius = {radius}");
                }
                _ => {}
            }
        }
    }
}
