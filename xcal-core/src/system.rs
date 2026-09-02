// system.rs
//
// System trait + Sys 闭包适配器 + InitSystem + ErasedSystem。
//
// 设计：
// - `System` trait 暴露两个关联类型 `Reads` / `Writes`，约束到
//   `ResourceTuple`。下游 System 通过声明这两个 tuple 显式标注
//   自己访问了哪些 Resource，供 Scheduler 在 debug 模式下做冲突检测、
//   在 release 模式下做零开销跳过。
//   注：`associated type defaults` 在稳定 Rust 上未稳定，因此不使用
//   `type Reads = ()` 默认值；下游必须显式声明 `type Reads = ();`
//   `type Writes = ();`。
// - `Sys<F, R, W>` 闭包适配器：通过 `sys::<R, W>(f)` 构造，内部用
//   `Box<dyn FnMut>` 类型擦除闭包具体类型，使 `Sys<Box<dyn FnMut>, R, W>`
//   在同一 `(R, W)` 下是同构的，便于 trait object 化。
// - `InitSystem: System` 提供 `init(&self, &mut Resources)` 默认注入；
//   blanket auto-impl 仅当 `System::Writes: ResourceTupleDefault` 时生效，
//   否则需手动 impl。
// - `ErasedSystem` 是 object-safe 的运行时 trait，Scheduler 内部用它存储
//   `Box<dyn ErasedSystem>`，把 System + InitSystem 的方法动态派发。
//   关联类型 `Reads` / `Writes` 通过 `reads_ids()` / `writes_ids()` 方法
//   在运行时查询，不破坏 object safety。

use std::any::TypeId;
use std::marker::PhantomData;

use crate::context::ExecutorContext;
use crate::resource::{ResourceTuple, ResourceTupleDefault, Resources};

// ─── System trait ─────────────────────────────────────────────────────────

/// 调度器中的单个 System。
///
/// 约束：`Send + Sync + 'static`，为将来并行调度预留接口。
/// 每帧调度器在对应阶段依次调用 [`run`](Self::run)。
/// System 之间禁止直接互调，只能通过 World 组件或 Resources 通信。
///
/// 关联类型：
/// - `Reads`: 声明只读访问的 Resource tuple（如 `(TimeState, AppCamera)`）
/// - `Writes`: 声明可写访问的 Resource tuple
///
/// 两者必须显式声明；如不访问任何 Resource，写 `type Reads = ();`
/// `type Writes = ();`。通过 [`crate::resource::ResourceTuple`] 递归展开为
/// `TypeId` 列表，用于 [`crate::access::Access`] 冲突检测。
pub trait System: Send + Sync + 'static {
    type Reads: ResourceTuple;
    type Writes: ResourceTuple;

    fn run(&mut self, ctx: &mut ExecutorContext<'_>);
}

// ─── Sys 闭包适配器 ───────────────────────────────────────────────────────

/// 闭包类型别名：erased `FnMut(&mut ExecutorContext)` + Send + Sync。
pub type SysClosure = Box<dyn FnMut(&mut ExecutorContext<'_>) + Send + Sync>;

/// 把闭包包装成 `System`，强制作者显式声明 `R`（Reads）与 `W`（Writes）。
///
/// `F` 通常是 [`SysClosure`]（由 [`sys`] 构造时自动 Box），但保留为
/// 泛型参数以便用户自定义非闭包实现。
pub struct Sys<F = SysClosure, R = (), W = ()> {
    f: F,
    _r: PhantomData<fn() -> R>,
    _w: PhantomData<fn() -> W>,
}

impl<F, R, W> System for Sys<F, R, W>
where
    F: FnMut(&mut ExecutorContext<'_>) + Send + Sync + 'static,
    R: ResourceTuple,
    W: ResourceTuple,
{
    type Reads = R;
    type Writes = W;

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        (self.f)(ctx);
    }
}

/// 构造闭包 System，显式声明 Reads/Writes tuple。
///
/// 示例：
/// ```ignore
/// scheduler.add_system(
///     Phase::Input,
///     sys::<(), (TimeState,)>(|ctx| {
///         if let Some(t) = ctx.get_mut::<TimeState>() {
///             t.elapsed += ctx.dt;
///         }
///     }),
/// );
/// ```
pub fn sys<R, W>(
    f: impl FnMut(&mut ExecutorContext<'_>) + Send + Sync + 'static,
) -> Sys<SysClosure, R, W>
where
    R: ResourceTuple,
    W: ResourceTuple,
{
    Sys {
        f: Box::new(f),
        _r: PhantomData,
        _w: PhantomData,
    }
}

// ─── InitSystem ───────────────────────────────────────────────────────────

/// System 初始化钩子：在调度器启动前注入默认资源。
///
/// - 默认实现（通过 blanket auto-impl）：当 `System::Writes:
///   ResourceTupleDefault` 时，`init` 会调用 `Writes::ensure_default`
///   为每个缺失的 `Resource` 注入 `Default` 实例。
/// - 特殊类型（Writes 含非 `Default` 元素）不满足 blanket 条件，
///   作者需手动 `impl InitSystem for XxxSystem` 提供自定义 init 逻辑。
///
/// `init` 取 `&self` 而非关联函数，是为了 object-safe 与 ErasedSystem 桥接。
pub trait InitSystem: System {
    /// 在 `Resources` 中为本 System 声明的所有 `Writes` 注入默认值。
    fn init(&self, resources: &mut Resources);
}

// Blanket auto-impl：满足 `Writes: ResourceTupleDefault` 的 System 自动获得
// `InitSystem`；不满足的类型不会被覆盖，作者可手动 impl。
impl<S> InitSystem for S
where
    S: System,
    S::Writes: ResourceTupleDefault,
{
    fn init(&self, resources: &mut Resources) {
        S::Writes::ensure_default(resources);
    }
}

// ─── ErasedSystem ─────────────────────────────────────────────────────────

/// 运行时 object-safe 的 System 接口，供 Scheduler 存储
/// `Box<dyn ErasedSystem>`。
///
/// 通过 blanket impl 把 `System + InitSystem` 的方法转成动态派发版本：
/// - `run_erased` → `<Self as System>::run`
/// - `init_erased` → `<Self as InitSystem>::init`
/// - `reads_ids` / `writes_ids` → 通过 `ResourceTuple::ids()` 查询
/// - `name` → `std::any::type_name::<Self>()`
pub trait ErasedSystem: Send + Sync + 'static {
    fn run_erased(&mut self, ctx: &mut ExecutorContext<'_>);
    fn init_erased(&self, resources: &mut Resources);
    fn reads_ids(&self) -> Vec<TypeId>;
    fn writes_ids(&self) -> Vec<TypeId>;
    fn name(&self) -> &'static str;
}

impl<S> ErasedSystem for S
where
    S: System + InitSystem,
{
    fn run_erased(&mut self, ctx: &mut ExecutorContext<'_>) {
        <Self as System>::run(self, ctx);
    }

    fn init_erased(&self, resources: &mut Resources) {
        <Self as InitSystem>::init(self, resources);
    }

    fn reads_ids(&self) -> Vec<TypeId> {
        <<Self as System>::Reads as ResourceTuple>::ids()
    }

    fn writes_ids(&self) -> Vec<TypeId> {
        <<Self as System>::Writes as ResourceTuple>::ids()
    }

    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::access::Access;
    use crate::context::BorrowTracker;
    use crate::scheduler::{Phase, Scheduler};

    // 测试用资源类型
    #[derive(Debug, Default, PartialEq, Clone)]
    struct Counter(i32);

    #[derive(Debug, Default, PartialEq)]
    struct Flag(bool);

    // ─── 测试 System 关联类型必须显式声明 ───────────────────

    #[test]
    fn test_system_explicit_associated_types() {
        struct DummySystem;
        impl System for DummySystem {
            type Reads = ();
            type Writes = ();
            fn run(&mut self, _ctx: &mut ExecutorContext<'_>) {}
        }
        let reads_ids = <<DummySystem as System>::Reads as ResourceTuple>::ids();
        let writes_ids = <<DummySystem as System>::Writes as ResourceTuple>::ids();
        assert!(reads_ids.is_empty());
        assert!(writes_ids.is_empty());
    }

    // ─── 测试 Sys 闭包适配器 ──────────────────────────────────

    #[test]
    fn test_sys_closure_runs() {
        let mut world = hecs::World::new();
        let mut cmd = hecs::CommandBuffer::new();
        let mut resources = Resources::new();
        resources.insert(Counter(0));
        #[cfg(debug_assertions)]
        let mut tracker = BorrowTracker::new();
        #[cfg(debug_assertions)]
        tracker.begin_system(
            "test_sys_closure_runs",
            Access::from_tuples::<(), (Counter,)>(),
        );

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
            let mut s = sys::<(), (Counter,)>(|ctx| {
                if let Some(c) = ctx.get_mut::<Counter>() {
                    c.0 += 1;
                }
            });
            s.run(&mut ctx);
        }

        #[cfg(debug_assertions)]
        tracker.end_system();

        assert_eq!(resources.get::<Counter>(), Some(&Counter(1)));
    }

    // ─── 测试 InitSystem blanket auto-impl ──────────────────────

    #[test]
    fn test_init_system_default_injects_writes() {
        struct CounterSystem;
        impl System for CounterSystem {
            type Reads = ();
            type Writes = (Counter, Flag);
            fn run(&mut self, _ctx: &mut ExecutorContext<'_>) {}
        }

        let mut resources = Resources::new();
        <CounterSystem as InitSystem>::init(&CounterSystem, &mut resources);
        assert_eq!(resources.get::<Counter>(), Some(&Counter(0)));
        assert_eq!(resources.get::<Flag>(), Some(&Flag(false)));
    }

    #[test]
    fn test_init_system_default_preserves_existing() {
        struct CounterSystem;
        impl System for CounterSystem {
            type Reads = ();
            type Writes = (Counter,);
            fn run(&mut self, _ctx: &mut ExecutorContext<'_>) {}
        }

        let mut resources = Resources::new();
        resources.insert(Counter(42));
        <CounterSystem as InitSystem>::init(&CounterSystem, &mut resources);
        // 不应覆盖已有值
        assert_eq!(resources.get::<Counter>(), Some(&Counter(42)));
    }

    // ─── 集成：通过 Scheduler 跑闭包 System + InitSystem ───────

    #[test]
    fn test_scheduler_with_sys_closure() {
        let mut sched = Scheduler::new(hecs::World::new(), Resources::new());
        sched.add_system(
            Phase::Input,
            sys::<(), (Counter,)>(|ctx| {
                if let Some(c) = ctx.get_mut::<Counter>() {
                    c.0 += 1;
                }
            }),
        );
        sched.init_resources();
        sched.run_frame(0.016);
        assert_eq!(sched.resources_mut().get::<Counter>(), Some(&Counter(1)));
    }

    // ─── 测试特殊类型手动 impl InitSystem ──────────────────────

    // 一个无 Default 的 Resource
    struct NoDefault {
        value: i32,
    }
    // Resource 通过 blanket impl 自动覆盖（Send+Sync+'static）
    // 但 NoDefault 不实现 Default，所以 (NoDefault,) 不 impl ResourceTupleDefault

    struct SpecialSystem;
    impl System for SpecialSystem {
        type Reads = ();
        type Writes = (NoDefault,);
        fn run(&mut self, _ctx: &mut ExecutorContext<'_>) {}
    }

    // 由于 (NoDefault,) 不 impl ResourceTupleDefault，blanket InitSystem 不生效，
    // 必须手动 impl：
    impl InitSystem for SpecialSystem {
        fn init(&self, resources: &mut Resources) {
            if !resources.contains::<NoDefault>() {
                resources.insert(NoDefault { value: 100 });
            }
        }
    }

    #[test]
    fn test_special_system_manual_init_system_impl() {
        let mut resources = Resources::new();
        let s = SpecialSystem;
        <SpecialSystem as InitSystem>::init(&s, &mut resources);
        let nd = resources.get::<NoDefault>();
        assert!(nd.is_some());
        assert_eq!(nd.unwrap().value, 100);
    }

    // ─── ErasedSystem 接口 ─────────────────────────────────────

    #[test]
    fn test_erased_system_dispatches_run_and_init() {
        struct CountingSystem;
        impl System for CountingSystem {
            type Reads = ();
            type Writes = (Counter,);
            fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
                if let Some(c) = ctx.get_mut::<Counter>() {
                    c.0 += 5;
                }
            }
        }

        let mut resources = Resources::new();
        let mut s = CountingSystem;
        // init via ErasedSystem
        <CountingSystem as ErasedSystem>::init_erased(&s, &mut resources);
        assert_eq!(resources.get::<Counter>(), Some(&Counter(0)));

        // run via ErasedSystem
        let mut world = hecs::World::new();
        let mut cmd = hecs::CommandBuffer::new();
        #[cfg(debug_assertions)]
        let mut tracker = BorrowTracker::new();
        #[cfg(debug_assertions)]
        tracker.begin_system("CountingSystem", Access::from_tuples::<(), (Counter,)>());
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
            <CountingSystem as ErasedSystem>::run_erased(&mut s, &mut ctx);
        }
        #[cfg(debug_assertions)]
        tracker.end_system();
        assert_eq!(resources.get::<Counter>(), Some(&Counter(5)));

        // name + ids
        assert_eq!(
            <CountingSystem as ErasedSystem>::name(&s),
            std::any::type_name::<CountingSystem>()
        );
        let wids = <CountingSystem as ErasedSystem>::writes_ids(&s);
        assert!(wids.contains(&TypeId::of::<Counter>()));
    }
}
