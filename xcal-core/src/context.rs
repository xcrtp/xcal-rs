// context.rs
//
// ExecutorContext + (debug-only) BorrowTracker。
//
// 设计：
// - `ExecutorContext` 把 `resources` 字段设为私有，只通过 `get` / `get_mut`
//   访问，强制所有 System 走声明的关联类型路径。
// - `world` / `commands` 字段保留 pub（hecs::CommandBuffer 不是 Resource）。
// - `dt` / `time` 平铺字段，System 可直接读取。
// - Debug 模式下，每次 `get` / `get_mut` 调用都会记录到 `BorrowTracker`；
//   `Scheduler` 在每个 System 执行结束后调用 `end_system`，
//   断言「实际 borrow ⊆ 声明 Reads ∪ Writes」。
// - Release 编译下 `BorrowTracker` 完全不存在，零开销。

#[cfg(debug_assertions)]
use std::any::TypeId;

#[cfg(debug_assertions)]
use std::collections::HashSet;

#[cfg(debug_assertions)]
use crate::access::Access;
use crate::resource::{Resource, Resources};

// ─── BorrowTracker（仅 debug）─────────────────────────────────────────────

/// Debug 模式下的借用追踪器。
///
/// 每个 System 执行前，由 `Scheduler` 调用 [`BorrowTracker::begin_system`]
/// 注入声明的 `Access`；`ExecutorContext::get` / `get_mut` 调用时记录
/// 实际访问的 `TypeId`；System 执行结束后调用 [`BorrowTracker::end_system`]
/// 断言「实际 ⊆ 声明」，违例时 panic 并打印系统名。
#[cfg(debug_assertions)]
#[derive(Default)]
pub struct BorrowTracker {
    current: Option<SystemBorrowState>,
}

#[cfg(debug_assertions)]
struct SystemBorrowState {
    name: &'static str,
    declared: Access,
    actual_reads: HashSet<TypeId>,
    actual_writes: HashSet<TypeId>,
}

#[cfg(debug_assertions)]
impl BorrowTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// 进入某个 System 的执行：记录其名称与声明的访问集。
    pub fn begin_system(&mut self, name: &'static str, declared: Access) {
        debug_assert!(
            self.current.is_none(),
            "BorrowTracker::begin_system called without end_system"
        );
        self.current = Some(SystemBorrowState {
            name,
            declared,
            actual_reads: HashSet::new(),
            actual_writes: HashSet::new(),
        });
    }

    pub fn record_read(&mut self, id: TypeId) {
        if let Some(state) = &mut self.current {
            state.actual_reads.insert(id);
        }
    }

    pub fn record_write(&mut self, id: TypeId) {
        if let Some(state) = &mut self.current {
            state.actual_writes.insert(id);
        }
    }

    /// 结束当前 System 的执行；断言实际 ⊆ 声明。
    ///
    /// 规则：
    /// - 实际 read 必须在 `declared.reads ∪ declared.writes` 中
    ///   （写者也允许读自身）
    /// - 实际 write 必须在 `declared.writes` 中
    pub fn end_system(&mut self) {
        let Some(state) = self.current.take() else {
            return;
        };

        for r in &state.actual_reads {
            let declared_read = state.declared.reads.contains(r);
            let declared_write = state.declared.writes.contains(r);
            if !declared_read && !declared_write {
                panic!(
                    "BorrowTracker: System '{}' read resource {:?} not declared in Reads/Writes",
                    state.name, r
                );
            }
        }
        for w in &state.actual_writes {
            if !state.declared.writes.contains(w) {
                panic!(
                    "BorrowTracker: System '{}' wrote resource {:?} not declared in Writes",
                    state.name, w
                );
            }
        }
    }
}

// ─── ExecutorContext ──────────────────────────────────────────────────────

/// System 执行时的上下文。
///
/// 字段布局：
/// - `world` / `commands` / `dt` / `time` 为 pub，System 直接读写。
/// - `resources` 私有，只能通过 [`get`](Self::get) / [`get_mut`](Self::get_mut)
///   访问，强制走声明的关联类型路径。
/// - debug 模式下额外携带 `BorrowTracker` 引用，自动记录每次资源访问。
///
/// 借用规则：同一个 `&mut ExecutorContext` 不能同时持有多个 `get_mut` 返回值，
/// System 需要同时操作多个资源时应按顺序克隆/取出再写入。
pub struct ExecutorContext<'frame> {
    pub world: &'frame mut hecs::World,
    pub commands: &'frame mut hecs::CommandBuffer,
    resources: &'frame mut Resources,
    pub dt: f32,
    pub time: f32,
    #[cfg(debug_assertions)]
    tracker: &'frame mut BorrowTracker,
}

impl<'frame> ExecutorContext<'frame> {
    pub fn new(
        world: &'frame mut hecs::World,
        commands: &'frame mut hecs::CommandBuffer,
        resources: &'frame mut Resources,
        dt: f32,
        time: f32,
        #[cfg(debug_assertions)] tracker: &'frame mut BorrowTracker,
    ) -> Self {
        Self {
            world,
            commands,
            resources,
            dt,
            time,
            #[cfg(debug_assertions)]
            tracker,
        }
    }

    // ─── World 便捷访问 ──────────────────────────────────────

    pub fn borrow_world(&self) -> &hecs::World {
        self.world
    }

    pub fn borrow_world_mut(&mut self) -> &mut hecs::World {
        self.world
    }

    // ─── Resources 访问（强制走 BorrowTracker） ─────────────

    /// 读取资源；debug 模式下记录为 `actual_read`。
    ///
    /// 注意：签名要求 `&mut self` 是为了让 debug 模式下的 `BorrowTracker`
    /// 能写入访问记录；release 编译下编译器会自动消除这层间接开销。
    pub fn get<R: Resource>(&mut self) -> Option<&R> {
        #[cfg(debug_assertions)]
        self.tracker.record_read(TypeId::of::<R>());
        self.resources.get::<R>()
    }

    /// 读取可变资源；debug 模式下记录为 `actual_write`。
    pub fn get_mut<R: Resource>(&mut self) -> Option<&mut R> {
        #[cfg(debug_assertions)]
        self.tracker.record_write(TypeId::of::<R>());
        self.resources.get_mut::<R>()
    }

    pub fn contains_resource<R: Resource>(&self) -> bool {
        self.resources.contains::<R>()
    }

    /// 在同一个调用内同时拿到 `&hecs::World` 与 `&mut R`，
    /// 通过闭包封装规避 Rust 借用检查器对 `&mut self` 的整体占用。
    ///
    /// 内部使用 split borrow：`world` 与 `resources` 是 `ExecutorContext`
    /// 的不同字段，可同时独立借用。
    pub fn with_world_and_resource_mut<R, F>(&mut self, f: F)
    where
        R: Resource,
        F: FnOnce(&hecs::World, &mut R),
    {
        #[cfg(debug_assertions)]
        self.tracker.record_write(TypeId::of::<R>());

        // Split borrow：world 与 resources 是 self 的不同字段，可分别借用。
        let resources = &mut self.resources;
        let world: &hecs::World = self.world;
        if let Some(r) = resources.get_mut::<R>() {
            f(world, r);
        }
    }

    // ─── CommandBuffer 便捷操作 ──────────────────────────────

    pub fn spawn(&mut self, bundle: impl hecs::DynamicBundle) {
        self.commands.spawn(bundle);
    }

    pub fn despawn(&mut self, entity: hecs::Entity) {
        self.commands.despawn(entity);
    }

    pub fn insert(&mut self, entity: hecs::Entity, bundle: impl hecs::DynamicBundle) {
        self.commands.insert(entity, bundle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_context_get_missing_returns_none() {
        let mut world = hecs::World::new();
        let mut cmd = hecs::CommandBuffer::new();
        let mut resources = Resources::new();
        #[cfg(debug_assertions)]
        let mut tracker = BorrowTracker::new();

        let mut ctx = ExecutorContext::new(
            &mut world,
            &mut cmd,
            &mut resources,
            0.016,
            0.0,
            #[cfg(debug_assertions)]
            &mut tracker,
        );

        assert!(ctx.get::<i32>().is_none());
        assert!(ctx.get_mut::<i32>().is_none());
    }

    #[test]
    fn test_executor_context_get_inserted_resource() {
        let mut world = hecs::World::new();
        let mut cmd = hecs::CommandBuffer::new();
        let mut resources = Resources::new();
        resources.insert(42_i32);
        #[cfg(debug_assertions)]
        let mut tracker = BorrowTracker::new();
        // declare access: read i32
        #[cfg(debug_assertions)]
        tracker.begin_system("test", {
            let mut a = Access::new();
            a.reads.insert(TypeId::of::<i32>());
            a
        });

        {
            let mut ctx = ExecutorContext::new(
                &mut world,
                &mut cmd,
                &mut resources,
                0.016,
                0.0,
                #[cfg(debug_assertions)]
                &mut tracker,
            );
            assert_eq!(ctx.get::<i32>(), Some(&42));
        }

        #[cfg(debug_assertions)]
        tracker.end_system();
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_borrow_tracker_detects_undeclared_read() {
        let mut tracker = BorrowTracker::new();
        tracker.begin_system("TestSystem", Access::new());
        tracker.record_read(TypeId::of::<i32>());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tracker.end_system();
        }));
        assert!(result.is_err(), "should panic on undeclared read");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_borrow_tracker_detects_undeclared_write() {
        let mut tracker = BorrowTracker::new();
        let mut declared = Access::new();
        declared.reads.insert(TypeId::of::<i32>());
        tracker.begin_system("TestSystem", declared);
        tracker.record_write(TypeId::of::<i32>());
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tracker.end_system();
        }));
        assert!(result.is_err(), "should panic on undeclared write");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_borrow_tracker_accepts_declared_read() {
        let mut tracker = BorrowTracker::new();
        let mut declared = Access::new();
        declared.reads.insert(TypeId::of::<i32>());
        tracker.begin_system("TestSystem", declared);
        tracker.record_read(TypeId::of::<i32>());
        tracker.end_system(); // should not panic
    }

    #[cfg(debug_assertions)]
    #[test]
    fn test_borrow_tracker_accepts_writer_as_reader() {
        let mut tracker = BorrowTracker::new();
        let mut declared = Access::new();
        declared.writes.insert(TypeId::of::<i32>());
        tracker.begin_system("TestSystem", declared);
        tracker.record_read(TypeId::of::<i32>());
        tracker.record_write(TypeId::of::<i32>());
        tracker.end_system(); // should not panic
    }
}
