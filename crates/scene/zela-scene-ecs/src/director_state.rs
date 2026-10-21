// director_state.rs — Director + 推进 System（zela-scene）

use ze_time::Director;
use zela_scene::TimeState;
use zela_schedule::context::ExecutorContext;
use zela_schedule::system::System;

/// 全局时间控制 Resource：包装 `ze_time::Director`。
/// 由 `DirectorAdvanceSystem`（Input 阶段）推进，`TimeControlSystem`（LogicUpdate）读写。
#[derive(Debug, Default, Clone)]
pub struct DirectorState {
    pub director: Director,
}

impl DirectorState {
    pub fn new() -> Self {
        Self {
            director: Director::new(),
        }
    }
}

/// 时间推进 System（Input 阶段）。
///
/// 唯一能推进时间的 System。每帧 advance dt，然后把 `director.global_t` 同步到
/// `TimeState.elapsed`（供 BindingApplySystem 等消费方读取）。
pub struct DirectorAdvanceSystem;

impl System for DirectorAdvanceSystem {
    type Reads = ();
    type Writes = (DirectorState, TimeState);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let dt = ctx.dt.min(0.1);

        // 先 clone 值再分别 get_mut，避免 &mut self 同时持有多份。
        let dir = ctx
            .get_mut::<DirectorState>()
            .map(|s| {
                s.director.advance(dt as f64);
                s.director.clone()
            })
            .unwrap_or_default();

        if let Some(state) = ctx.get_mut::<TimeState>() {
            state.elapsed = dir.global_t as f32;
        }
    }
}
