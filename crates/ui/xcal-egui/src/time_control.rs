// time_control.rs — 时间控制事件消费（LogicUpdate 阶段）
//
// B 阶段（UI 解耦）后本模块不含任何 egui 类型：
// - 面板 chrome（播放/暂停按钮、速度滑块、时间轴 seek 条）在
//   host.rs::draw_time_panel，控件交互以 SceneEvent（WidgetId 标识）写入 InputEvents；
// - 本 System 只消费事件并写 DirectorState——可 headless 测试。

use xcal_scene::director_state::DirectorState;
use xcal_scene::domain::{InputEvents, SceneEvent, ids};
use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;

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

#[cfg(test)]
mod tests {
    use super::*;
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
}
