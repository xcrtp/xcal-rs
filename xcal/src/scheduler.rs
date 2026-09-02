// scheduler.rs
//
// 按阶段（Phase）分组管理 System 的调度器。
// 每帧执行顺序：Input → LogicUpdate → SceneSync → RenderCommit → FrameEnd。

use crate::executor::{ExecutorContext, Resources, System};

// ─── Phase 枚举 ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Input,
    LogicUpdate,
    SceneSync,
    RenderCommit,
    FrameEnd,
}

impl Phase {
    pub fn all() -> [Phase; 5] {
        [
            Phase::Input,
            Phase::LogicUpdate,
            Phase::SceneSync,
            Phase::RenderCommit,
            Phase::FrameEnd,
        ]
    }
}

// ─── Scheduler ─────────────────────────────────────────────────────────────

pub struct Scheduler {
    world: hecs::World,
    resources: Resources,
    phases: [Vec<Box<dyn System>>; 5],
}

impl Scheduler {
    pub fn new(world: hecs::World, resources: Resources) -> Self {
        Self {
            world,
            resources,
            phases: Default::default(),
        }
    }

    pub fn resources_mut(&mut self) -> &mut Resources {
        &mut self.resources
    }

    pub fn add_system<S: System>(&mut self, phase: Phase, system: S) {
        let idx = phase_index(phase);
        self.phases[idx].push(Box::new(system));
    }

    /// 每帧执行一次（带 egui UI）。返回执行的 System 总数。
    #[cfg(feature = "egui")]
    pub fn run_frame(&mut self, dt: f32, ui: Option<&mut egui::Ui>) -> usize {
        self.run_frame_inner(dt, 0.0, ui)
    }

    /// 无头执行。
    #[cfg(not(feature = "egui"))]
    pub fn run_frame(&mut self, dt: f32) -> usize {
        self.run_frame_inner(dt, 0.0)
    }

    #[cfg(feature = "egui")]
    pub fn run_frame_with_time(
        &mut self,
        dt: f32,
        time: f32,
        ui: Option<&mut egui::Ui>,
    ) -> usize {
        self.run_frame_inner(dt, time, ui)
    }

    #[cfg(not(feature = "egui"))]
    pub fn run_frame_with_time(&mut self, dt: f32, time: f32) -> usize {
        self.run_frame_inner(dt, time)
    }

    #[cfg(feature = "egui")]
    fn run_frame_inner(
        &mut self,
        dt: f32,
        time: f32,
        mut ui: Option<&mut egui::Ui>,
    ) -> usize {
        let mut count = 0;

        for phase_idx in 0..5 {
            // 每个阶段独立的 CommandBuffer，阶段末 flush 到 World
            let mut cmd = hecs::CommandBuffer::new();

            let systems = &mut self.phases[phase_idx];
            for sys in systems.iter_mut() {
                // 每次循环 reborrow &mut Ui。
                // 上一次循环结束后 ExecutorContext drop，&mut Ui 释放，
                // 所以这里可以再次取出（Rust 借用检查器按作用域追踪）。
                let ui_reborrow: Option<&mut egui::Ui> = ui.as_deref_mut();

                let mut ctx = ExecutorContext::new(
                    &mut self.world,
                    &mut cmd,
                    &mut self.resources,
                    dt,
                    time,
                    ui_reborrow,
                );
                sys.run(&mut ctx);
                count += 1;
            }

            cmd.run_on(&mut self.world);
        }

        count
    }

    #[cfg(not(feature = "egui"))]
    fn run_frame_inner(&mut self, dt: f32, time: f32) -> usize {
        let mut count = 0;

        for phase_idx in 0..5 {
            let mut cmd = hecs::CommandBuffer::new();

            let systems = &mut self.phases[phase_idx];
            for sys in systems.iter_mut() {
                let mut ctx = ExecutorContext::new(
                    &mut self.world,
                    &mut cmd,
                    &mut self.resources,
                    dt,
                    time,
                );
                sys.run(&mut ctx);
                count += 1;
            }

            cmd.run_on(&mut self.world);
        }

        count
    }
}

fn phase_index(phase: Phase) -> usize {
    match phase {
        Phase::Input => 0,
        Phase::LogicUpdate => 1,
        Phase::SceneSync => 2,
        Phase::RenderCommit => 3,
        Phase::FrameEnd => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phase_all_order() {
        let phases = Phase::all();
        assert_eq!(phases[0], Phase::Input);
        assert_eq!(phases[1], Phase::LogicUpdate);
        assert_eq!(phases[2], Phase::SceneSync);
        assert_eq!(phases[3], Phase::RenderCommit);
        assert_eq!(phases[4], Phase::FrameEnd);
    }

    #[test]
    fn test_scheduler_empty_runs() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());
        #[cfg(feature = "egui")]
        let count = sched.run_frame(0.016, None);
        #[cfg(not(feature = "egui"))]
        let count = sched.run_frame(0.016);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_scheduler_ordered_execution() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());
        sched.resources_mut().insert(Vec::<u32>::new());

        sched.add_system(Phase::Input, |ctx: &mut ExecutorContext<'_>| {
            if let Some(v) = ctx.resources.borrow_mut::<Vec<u32>>() {
                v.push(1);
            }
        });
        sched.add_system(Phase::LogicUpdate, |ctx: &mut ExecutorContext<'_>| {
            if let Some(v) = ctx.resources.borrow_mut::<Vec<u32>>() {
                v.push(2);
            }
        });
        sched.add_system(Phase::Input, |ctx: &mut ExecutorContext<'_>| {
            if let Some(v) = ctx.resources.borrow_mut::<Vec<u32>>() {
                v.push(3);
            }
        });
        sched.add_system(Phase::FrameEnd, |ctx: &mut ExecutorContext<'_>| {
            if let Some(v) = ctx.resources.borrow_mut::<Vec<u32>>() {
                v.push(4);
            }
        });

        #[cfg(feature = "egui")]
        sched.run_frame(0.016, None);
        #[cfg(not(feature = "egui"))]
        sched.run_frame(0.016);
        let result = sched.resources_mut().borrow::<Vec<u32>>().unwrap();
        assert_eq!(*result, vec![1, 3, 2, 4]);
    }

    #[test]
    fn test_command_buffer_flushed_per_phase() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());

        sched.add_system(Phase::Input, |ctx: &mut ExecutorContext<'_>| {
            ctx.spawn((42_i32,));
        });
        sched.add_system(Phase::LogicUpdate, |ctx: &mut ExecutorContext<'_>| {
            assert_eq!((&*ctx.world).query::<&i32>().iter().count(), 1);
        });

        #[cfg(feature = "egui")]
        sched.run_frame(0.016, None);
        #[cfg(not(feature = "egui"))]
        sched.run_frame(0.016);
    }
}
