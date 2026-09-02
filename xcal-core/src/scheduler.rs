// scheduler.rs
//
// 按阶段（Phase）分组管理 System 的调度器。
// 每帧执行顺序：Input → LogicUpdate → SceneSync → Prepare → RenderCommit → FrameEnd。
//
// 新架构：
// - 注册时通过 `System::Reads` / `System::Writes` 构造 [`Access`]，
//   debug 模式下检测同 phase 内 read/write 冲突，panic 带系统名。
// - 提供 [`Scheduler::init_resources`]，对每个 System 调用
//   `InitSystem::init`，按 Writes::ensure_default 注入默认 Resource。
// - 帧执行时，debug 下用 [`BorrowTracker`] 包裹每个 System 的执行，
//   记录实际 borrow 的 TypeId，与声明对比，越界 panic 带系统名。
// - Release 编译下 BorrowTracker 与 Access 检测完全消失，零开销。
//
// Scheduler 内部存储 `Box<dyn ErasedSystem>`（通过 blanket impl 适配任何
// `System + InitSystem`），把 run/init/reads_ids/writes_ids/name 动态派发。

use std::any::TypeId;

#[cfg(debug_assertions)]
use crate::access::Access;
#[cfg(debug_assertions)]
use crate::context::BorrowTracker;
use crate::context::ExecutorContext;
use crate::resource::Resources;
use crate::system::{ErasedSystem, InitSystem, System};

// ─── Phase 枚举 ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Phase {
    Input,
    LogicUpdate,
    SceneSync,
    /// GPU 资源准备：CPU → GPU 上传、缓存填充（ResourceCache）。
    Prepare,
    RenderCommit,
    FrameEnd,
}

impl Phase {
    pub fn all() -> [Phase; 6] {
        [
            Phase::Input,
            Phase::LogicUpdate,
            Phase::SceneSync,
            Phase::Prepare,
            Phase::RenderCommit,
            Phase::FrameEnd,
        ]
    }
}

// ─── PhaseSlot ────────────────────────────────────────────────────────────

/// 单个 phase 内注册的 system 与其声明的 `Access`（debug 用）。
#[derive(Default)]
struct PhaseSlot {
    systems: Vec<SlotEntry>,
}

struct SlotEntry {
    system: Box<dyn ErasedSystem>,
    #[cfg(debug_assertions)]
    name: &'static str,
    #[cfg(debug_assertions)]
    access: Access,
}

// ─── Scheduler ────────────────────────────────────────────────────────────

pub struct Scheduler {
    world: hecs::World,
    resources: Resources,
    phases: [PhaseSlot; 6],
    #[cfg(debug_assertions)]
    tracker: BorrowTracker,
}

impl Scheduler {
    pub fn new(world: hecs::World, resources: Resources) -> Self {
        Self {
            world,
            resources,
            phases: Default::default(),
            #[cfg(debug_assertions)]
            tracker: BorrowTracker::new(),
        }
    }

    pub fn resources(&self) -> &Resources {
        &self.resources
    }

    pub fn resources_mut(&mut self) -> &mut Resources {
        &mut self.resources
    }

    pub fn world(&self) -> &hecs::World {
        &self.world
    }

    pub fn world_mut(&mut self) -> &mut hecs::World {
        &mut self.world
    }

    // ─── 注册 ─────────────────────────────────────────────────

    /// 注册一个 System 到指定 phase。
    ///
    /// 要求 `S: System + InitSystem`：满足 `Writes: ResourceTupleDefault`
    /// 的 System 自动通过 blanket impl 获得 InitSystem；其余需手动 impl。
    ///
    /// Debug 模式下检测与同 phase 已有 system 的 read/write 冲突；
    /// 冲突时 panic 并打印系统名与冲突 TypeId。
    pub fn add_system<S>(&mut self, phase: Phase, system: S)
    where
        S: System + InitSystem,
    {
        let idx = phase_index(phase);
        let slot = &mut self.phases[idx];

        #[cfg(debug_assertions)]
        {
            let name = std::any::type_name::<S>();
            let new_access = Access::from_tuples::<S::Reads, S::Writes>();
            for (existing_idx, existing) in slot.systems.iter().enumerate() {
                if new_access.conflicts_with(&existing.access) {
                    let conflicts: Vec<TypeId> = new_access
                        .writes
                        .iter()
                        .filter(|w| {
                            existing.access.reads.contains(w) || existing.access.writes.contains(w)
                        })
                        .chain(existing.access.writes.iter().filter(|w| {
                            new_access.reads.contains(w) || new_access.writes.contains(w)
                        }))
                        .copied()
                        .collect();
                    panic!(
                        "Scheduler conflict in phase {:?}: system '{}' (index {}) conflicts with \
                         new system '{}' on resources {:?}",
                        phase, existing.name, existing_idx, name, conflicts
                    );
                }
            }
            slot.systems.push(SlotEntry {
                system: Box::new(system),
                name,
                access: new_access,
            });
        }

        #[cfg(not(debug_assertions))]
        {
            let _ = (TypeId::of::<S::Reads>(), TypeId::of::<S::Writes>()); // 约束泛型
            slot.systems.push(SlotEntry {
                system: Box::new(system),
            });
        }
    }

    // ─── 初始化 ───────────────────────────────────────────────

    /// 对所有已注册的 System 调用 `InitSystem::init`，
    /// 为每个 System 的 `Writes` 注入默认 Resource。
    ///
    /// 应在 `run_frame` 之前调用一次。
    pub fn init_resources(&mut self) {
        // 借用分离：phases 与 resources 是不同字段，可同时借用。
        let Scheduler {
            phases, resources, ..
        } = self;
        for phase in phases.iter_mut() {
            for entry in phase.systems.iter_mut() {
                entry.system.init_erased(resources);
            }
        }
    }

    // ─── 执行 ─────────────────────────────────────────────────

    /// 每帧执行一次。返回执行的 System 总数。
    pub fn run_frame(&mut self, dt: f32) -> usize {
        self.run_frame_with_time(dt, 0.0)
    }

    pub fn run_frame_with_time(&mut self, dt: f32, time: f32) -> usize {
        let mut count = 0;

        for phase in Phase::all() {
            let phase_idx = phase_index(phase);
            // 每个阶段独立的 CommandBuffer，阶段末 flush 到 World
            let mut cmd = hecs::CommandBuffer::new();

            let systems = &mut self.phases[phase_idx].systems;
            for entry in systems.iter_mut() {
                #[cfg(debug_assertions)]
                {
                    self.tracker.begin_system(entry.name, entry.access.clone());
                }

                {
                    let mut ctx = ExecutorContext::new(
                        &mut self.world,
                        &mut cmd,
                        &mut self.resources,
                        dt,
                        time,
                        #[cfg(debug_assertions)]
                        &mut self.tracker,
                    );
                    entry.system.run_erased(&mut ctx);
                }

                #[cfg(debug_assertions)]
                self.tracker.end_system();
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
        Phase::Prepare => 3,
        Phase::RenderCommit => 4,
        Phase::FrameEnd => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource::Resources;
    use crate::system::sys;

    #[test]
    fn test_phase_all_order() {
        let phases = Phase::all();
        assert_eq!(phases[0], Phase::Input);
        assert_eq!(phases[1], Phase::LogicUpdate);
        assert_eq!(phases[2], Phase::SceneSync);
        assert_eq!(phases[3], Phase::Prepare);
        assert_eq!(phases[4], Phase::RenderCommit);
        assert_eq!(phases[5], Phase::FrameEnd);
    }

    #[test]
    fn test_scheduler_empty_runs() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());
        let count = sched.run_frame(0.016);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_scheduler_ordered_execution() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());
        sched.add_system(
            Phase::Input,
            sys::<(), (Vec<u32>,)>(|ctx| {
                if let Some(v) = ctx.get_mut::<Vec<u32>>() {
                    v.push(1);
                }
            }),
        );
        sched.add_system(
            Phase::LogicUpdate,
            sys::<(), (Vec<u32>,)>(|ctx| {
                if let Some(v) = ctx.get_mut::<Vec<u32>>() {
                    v.push(2);
                }
            }),
        );
        sched.add_system(
            Phase::SceneSync,
            sys::<(), (Vec<u32>,)>(|ctx| {
                if let Some(v) = ctx.get_mut::<Vec<u32>>() {
                    v.push(3);
                }
            }),
        );
        sched.add_system(
            Phase::FrameEnd,
            sys::<(), (Vec<u32>,)>(|ctx| {
                if let Some(v) = ctx.get_mut::<Vec<u32>>() {
                    v.push(4);
                }
            }),
        );

        sched.init_resources();
        sched.run_frame(0.016);
        let result = sched.resources_mut().get::<Vec<u32>>().unwrap();
        assert_eq!(*result, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_command_buffer_flushed_per_phase() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());

        sched.add_system(
            Phase::Input,
            sys::<(), ()>(|ctx| {
                ctx.spawn((42_i32,));
            }),
        );
        sched.add_system(
            Phase::LogicUpdate,
            sys::<(), ()>(|ctx| {
                assert_eq!(ctx.world.query::<&i32>().iter().count(), 1);
            }),
        );

        sched.run_frame(0.016);
    }

    #[test]
    fn test_init_resources_injects_defaults() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());

        #[derive(Debug, Default, PartialEq)]
        struct Foo(i32);
        sched.add_system(
            Phase::Input,
            sys::<(), (Foo,)>(|ctx| {
                if let Some(f) = ctx.get_mut::<Foo>() {
                    f.0 += 1;
                }
            }),
        );

        sched.init_resources();
        assert_eq!(sched.resources().get::<Foo>(), Some(&Foo(0)));
        sched.run_frame(0.016);
        assert_eq!(sched.resources().get::<Foo>(), Some(&Foo(1)));
    }

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "Scheduler conflict")]
    fn test_conflict_detection_panics_on_write_vs_read() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());

        sched.add_system(
            Phase::Input,
            sys::<(i32,), ()>(|ctx| {
                let _ = ctx.get::<i32>();
            }),
        );
        // 同 phase 内，新 system 写 i32 与已有 system 读 i32 冲突
        sched.add_system(Phase::Input, sys::<(), (i32,)>(|_ctx| {}));
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_no_conflict_across_phases() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());

        sched.add_system(
            Phase::Input,
            sys::<(i32,), ()>(|ctx| {
                let _ = ctx.get::<i32>();
            }),
        );
        sched.add_system(Phase::LogicUpdate, sys::<(), (i32,)>(|_ctx| {}));
        // 跨 phase 不算冲突
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_double_read_no_conflict_in_same_phase() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());
        sched.add_system(
            Phase::Input,
            sys::<(i32,), ()>(|ctx| {
                let _ = ctx.get::<i32>();
            }),
        );
        sched.add_system(
            Phase::Input,
            sys::<(i32,), ()>(|ctx| {
                let _ = ctx.get::<i32>();
            }),
        );
    }

    // ─── BorrowTracker 跨系统隔离 ────────────────────────────

    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "BorrowTracker")]
    fn test_borrow_tracker_panics_on_undeclared_read() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());
        sched.add_system(
            Phase::Input,
            sys::<(), ()>(|ctx| {
                // 声明 Reads=()/Writes=() 但实际读 i32
                let _ = ctx.get::<i32>();
            }),
        );
        sched.run_frame(0.016);
    }
}
