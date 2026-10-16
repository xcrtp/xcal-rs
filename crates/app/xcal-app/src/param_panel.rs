// param_panel.rs — 参数面板事件消费（LogicUpdate 阶段）
//
// B 阶段（UI 解耦）后本模块不含任何 egui 类型：
// - 面板 chrome（滑块绘制）在 xcal-egui::host::draw_param_panel，控件交互以
//   SceneEvent::Slider（WidgetId 标识）写入 InputEvents；
// - 本 System 只消费 Slider 事件并按 **(目标类别, 曲线类型)** 匹配写回 BindingTable
//   （与 A5 一致，不依赖插入顺序）——可 headless 测试。
//
// P0：驱动目标 trait 化后 target 无法模式匹配，故用 `Target::kind()`（无载荷判别式）
// 分类；曲线仍按类型匹配。
//
// 这是节点编辑器的简化版，后续替换为完整节点图（xc-node-schema + xcal-node-egui）
// 时上层逻辑不动。

use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;
use xcal_scene::Curve;
use xcal_scene::animation::{BindingTable, TargetKind};
use xcal_scene::domain::{InputEvents, SceneEvent, WidgetId, ids};

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
            let kind = binding.target.kind();
            for (id, value) in &sliders {
                match (*id, kind, &mut binding.curve) {
                    (ids::CIRCLE_BASE, TargetKind::CircleRadius, Curve::Sin { base, .. }) => {
                        *base = *value
                    }
                    (ids::CIRCLE_AMP, TargetKind::CircleRadius, Curve::Sin { amp, .. }) => {
                        *amp = *value
                    }
                    (ids::CIRCLE_OMEGA, TargetKind::CircleRadius, Curve::Sin { omega, .. }) => {
                        *omega = *value
                    }
                    (ids::PHASE_OMEGA, TargetKind::Phase, Curve::Phase { omega }) => *omega = *value,
                    (ids::ORBIT_RADIUS, TargetKind::CameraEye, Curve::Orbit { radius, .. }) => {
                        *radius = *value
                    }
                    (ids::ORBIT_SPEED, TargetKind::CameraEye, Curve::Orbit { speed, .. }) => {
                        *speed = *value
                    }
                    (
                        ids::ORBIT_ANGLE,
                        TargetKind::CameraEye,
                        Curve::Orbit { initial_angle, .. },
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
    use std::sync::Arc;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};
    use xcal_scene::ClockId;
    use xcal_scene::animation::{Binding, CircleRadius, CameraEye, SeekKind};
    use xcal_scene::contract::ObjectId;

    /// headless：Slider 事件按 WidgetId 精确写入对应绑定字段，顺序无关。
    #[test]
    fn slider_events_patch_binding_fields_by_widget_id() {
        let mut table = BindingTable::new();
        table.add(Binding {
            clock: ClockId(0),
            curve: Curve::Sin {
                base: 1.0,
                amp: 0.4,
                omega: 2.0,
            },
            target: Arc::new(CircleRadius {
                id: ObjectId::DANGLING,
            }),
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
            target: Arc::new(CameraEye {
                id: ObjectId::DANGLING,
            }),
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
            match (binding.target.kind(), &binding.curve) {
                (TargetKind::CircleRadius, Curve::Sin { amp, base, omega }) => {
                    assert!((amp - 0.8).abs() < 1e-5, "amp = {amp}");
                    assert!((base - 1.0).abs() < 1e-5, "base 不应被无关事件改动");
                    assert!((omega - 2.0).abs() < 1e-5);
                }
                (TargetKind::CameraEye, Curve::Orbit { radius, .. }) => {
                    assert!((radius - 12.0).abs() < 1e-5, "radius = {radius}");
                }
                _ => {}
            }
        }
    }
}
