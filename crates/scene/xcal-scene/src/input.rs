// input.rs — UI 输入事件的场景侧消费者（S0：从 `xcal-egui` 归位）。
//
// 面板 chrome（按钮 / 滑块）仍在 `xcal-egui` 的 host 里绘制；它们只把交互写成
// `SceneEvent`（按 `WidgetId` 标识）放进 `InputEvents`。本模块的两个 System
// 只消费这些纯数据事件并改场景状态 —— 可 headless 测试，不含任何 egui 类型。
//
// Phase 分配：LogicUpdate（与 `DirectorAdvanceSystem` 的 Input 相位分开）。

use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;

use crate::Curve;
use crate::animation::BindingTable;
use crate::director_state::DirectorState;
use crate::domain::{InputEvents, SceneEvent, WidgetId, ids};

// ─── TimeControlSystem（播放/暂停、速度、seek）────────────────────────────

/// 时间控制事件消费（LogicUpdate 阶段）。
pub struct TimeControlSystem;
impl System for TimeControlSystem {
    type Reads = (InputEvents,);
    type Writes = (DirectorState,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let events: Vec<SceneEvent> = match ctx.get::<InputEvents>() {
            Some(e) => e.iter().copied().collect(),
            None => return,
        };
        if events.is_empty() {
            return;
        }

        // 写者可读自身：clone 出本地副本应用事件，结束后写回。
        let mut dir = ctx.get::<DirectorState>().cloned().unwrap_or_default();
        for event in events {
            match event {
                SceneEvent::Button { id } if id == ids::PLAY_PAUSE => dir.director.toggle(),
                // 重新播放：拨回 0 并恢复播放。全部绑定是 O1（纯函数、无记忆）
                // ⇒ 场景状态由 t 唯一决定，回到 0 即回到初始画面（无需重建场景）。
                SceneEvent::Button { id } if id == ids::REPLAY => {
                    dir.director.seek(0.0);
                    dir.director.resume();
                }
                SceneEvent::Slider { id, value } if id == ids::SPEED => {
                    dir.director.set_speed(value as f64);
                }
                SceneEvent::Slider { id, value } if id == ids::SEEK => {
                    dir.director.seek(value as f64);
                }
                _ => {}
            }
        }
        if let Some(d) = ctx.get_mut::<DirectorState>() {
            *d = dir;
        }
    }
}

// ─── ParamPanelSystem（滑块 → BindingTable）──────────────────────────────

/// 参数面板事件消费（LogicUpdate 阶段）。
///
/// 只消费 `SceneEvent::Slider` 并按 **WidgetId** 写回 `BindingTable`
/// （与 A5 一致，不依赖插入顺序）。
///
/// T0-②：驱动目标 trait 化后 target 无法模式匹配，且**集中分类清单已删除**。
/// 写回只靠 `WidgetId` —— 它本身**唯一确定**一个滑块（面板里六个 `WidgetId` 互不重复），
/// 故 `kind` 是冗余的保险，去掉后仍无歧义。滑块是否显示由**曲线类型**决定（见 host 侧），
/// 与 target 无关。
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

        // 写回：**只按 `WidgetId` + 曲线类型**匹配（不再需要 target 类别）。
        for binding in table.bindings.iter_mut() {
            for (id, value) in &sliders {
                match (*id, &mut binding.curve) {
                    (ids::CIRCLE_BASE, Curve::Sin { base, .. }) => *base = *value,
                    (ids::CIRCLE_AMP, Curve::Sin { amp, .. }) => *amp = *value,
                    (ids::CIRCLE_OMEGA, Curve::Sin { omega, .. }) => *omega = *value,
                    (ids::PHASE_OMEGA, Curve::Phase { omega }) => *omega = *value,
                    (ids::ORBIT_RADIUS, Curve::Orbit { radius, .. }) => *radius = *value,
                    (ids::ORBIT_SPEED, Curve::Orbit { speed, .. }) => *speed = *value,
                    (ids::ORBIT_ANGLE, Curve::Orbit { initial_angle, .. }) => {
                        *initial_angle = *value
                    }
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
    use crate::ClockId;
    use crate::animation::{Binding, CameraEye, CircleRadius, SeekKind};
    use crate::contract::ObjectId;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};

    fn run_with_events(events: Vec<SceneEvent>) -> DirectorState {
        let mut resources = Resources::new();
        resources.insert(DirectorState::new());
        resources.insert(InputEvents::from_events(events));

        let mut sched = Scheduler::new(hecs::World::new(), resources);
        sched.add_system(Phase::LogicUpdate, TimeControlSystem);
        sched.init_resources();
        sched.run_frame(0.016);
        sched.resources().get::<DirectorState>().cloned().unwrap()
    }

    /// headless：Button(PLAY_PAUSE) 切换播放状态。
    #[test]
    fn play_pause_button_toggles_director() {
        let before = DirectorState::new();
        let after = run_with_events(vec![SceneEvent::Button {
            id: ids::PLAY_PAUSE,
        }]);
        assert_ne!(after.director.playing, before.director.playing);
    }

    /// headless：Button(REPLAY) 把时间拨回 0 并恢复播放（无论当前是否暂停）。
    #[test]
    fn replay_button_seeks_to_zero_and_resumes() {
        // 当前状态：已跑到 3.7s 且处于暂停 —— 重播应同时"归零"与"开始播放"。
        let mut resources = Resources::new();
        let mut director = DirectorState::new();
        director.director.seek(3.7);
        director.director.pause();
        resources.insert(director);
        resources.insert(InputEvents::from_events(vec![SceneEvent::Button {
            id: ids::REPLAY,
        }]));

        let mut sched = Scheduler::new(hecs::World::new(), resources);
        sched.add_system(Phase::LogicUpdate, TimeControlSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let d = sched.resources().get::<DirectorState>().unwrap();
        assert!(
            d.director.global_t.abs() < 1e-9,
            "应拨回 0，实际 {}",
            d.director.global_t
        );
        assert!(d.director.playing, "重播后应处于播放态");
    }

    /// headless：Slider(SPEED) / Slider(SEEK) 分别作用于 speed / global_t。
    #[test]
    fn slider_events_drive_speed_and_seek() {
        let after = run_with_events(vec![
            SceneEvent::Slider {
                id: ids::SPEED,
                value: 2.0,
            },
            SceneEvent::Slider {
                id: ids::SEEK,
                value: 5.5,
            },
        ]);
        assert!((after.director.speed - 2.0).abs() < 1e-5);
        assert!((after.director.global_t - 5.5).abs() < 1e-5);
    }

    /// 无关事件（其他控件 ID）不影响 DirectorState。
    #[test]
    fn unrelated_events_are_ignored() {
        let before = DirectorState::new();
        let after = run_with_events(vec![
            SceneEvent::Button { id: ids::SHOW_GRID },
            SceneEvent::Slider {
                id: ids::CIRCLE_BASE,
                value: 9.9,
            },
        ]);
        assert_eq!(after.director.playing, before.director.playing);
        assert_eq!(after.director.speed, before.director.speed);
        assert_eq!(after.director.global_t, before.director.global_t);
    }

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
        let mut saw_sin = false;
        let mut saw_orbit = false;
        for binding in &table.bindings {
            // T0-② 后按**曲线类型**分辨绑定（target 类别清单已删除）。
            match &binding.curve {
                Curve::Sin { amp, base, omega } => {
                    saw_sin = true;
                    assert!((amp - 0.8).abs() < 1e-5, "amp = {amp}");
                    assert!((base - 1.0).abs() < 1e-5, "base 不应被无关事件改动");
                    assert!((omega - 2.0).abs() < 1e-5);
                }
                Curve::Orbit { radius, .. } => {
                    saw_orbit = true;
                    assert!((radius - 12.0).abs() < 1e-5, "radius = {radius}");
                }
                _ => {}
            }
        }
        assert!(saw_sin && saw_orbit, "两条绑定都应被核对过");
    }
}
