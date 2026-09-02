// executor.rs
//
// 核心执行原语：System trait + ExecutorContext。
// System 之间只通过 World 组件和 Resources 通信，不直接互调。
//
// 设计说明：
// ExecutorContext 把所有字段平铺（world / commands / resources / dt / time），
// 这样 Rust 的 split borrow 机制允许 System 同时 &mut 多个不同字段。
// egui UI 已降级为 Resource（EguiFrameContext），不再出现在 ExecutorContext 中，
// 使 executor 模块不再直接依赖 egui。

use std::any::TypeId;
use std::collections::HashMap;

// ─── Resources ────────────────────────────────────────────────────────────

/// 全局运行时资源容器。
///
/// 与 hecs::World 平级，存放非组件形式的全局状态（如相机、时间、配置等）。
/// 每个资源类型只能有一个实例；访问时用类型作为 key。
#[derive(Default)]
pub struct Resources {
    data: HashMap<TypeId, Box<dyn std::any::Any + Send + Sync>>,
}

impl Resources {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert<T: Send + Sync + 'static>(&mut self, resource: T) {
        self.data.insert(TypeId::of::<T>(), Box::new(resource));
    }

    pub fn borrow<T: 'static>(&self) -> Option<&T> {
        self.data
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref::<T>())
    }

    pub fn borrow_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.data
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut::<T>())
    }

    /// 移除并返回指定类型的资源。
    pub fn remove<T: 'static>(&mut self) -> Option<T> {
        self.data
            .remove(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast::<T>().ok())
            .map(|b| *b)
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

// ─── ExecutorContext ──────────────────────────────────────────────────────

/// System 执行时的上下文。
///
/// 所有字段平铺，利用 Rust 的 split borrow 机制：
/// 同一个 `&mut ExecutorContext` 上可以同时独立地 &mut 不同字段。
///
/// 借用规则（Rust 编译器会自动检查）：
/// - 同时 `&mut resources` + `&mut world` ✓（不同字段）
/// - 同类型 Resource 不能同时 & 和 &mut —— 自行按顺序管理
///
/// egui UI 不再直接出现在此结构中，而是作为 `EguiFrameContext` Resource
/// 存储在 `Resources` 里，System 通过 `ctx.resources.borrow_mut::<EguiFrameContext>()`
/// 获取。
pub struct ExecutorContext<'frame> {
    pub world: &'frame mut hecs::World,
    pub commands: &'frame mut hecs::CommandBuffer,
    pub resources: &'frame mut Resources,
    pub dt: f32,
    pub time: f32,
}

impl<'frame> ExecutorContext<'frame> {
    pub fn new(
        world: &'frame mut hecs::World,
        commands: &'frame mut hecs::CommandBuffer,
        resources: &'frame mut Resources,
        dt: f32,
        time: f32,
    ) -> Self {
        Self {
            world,
            commands,
            resources,
            dt,
            time,
        }
    }

    // ─── World 便捷访问 ──────────────────────────────────────

    pub fn borrow_world(&self) -> &hecs::World {
        self.world
    }

    pub fn borrow_world_mut(&mut self) -> &mut hecs::World {
        self.world
    }

    // ─── Resources 便捷访问 ──────────────────────────────────
    //
    // 注意：Rust 的 split borrow 只在 **不同字段间** 生效。
    // 对同一个 `resources` 字段，仍然遵循"不能同时 & 和 &mut"的规则。
    // 所以 System 中如需先 clone 几个只读 Resource，再 mutably borrow 另一个，
    // 请先把所有只读值 clone 出来，再调用 resources_mut。

    pub fn borrow_resource<T: 'static>(&self) -> Option<&T> {
        self.resources.borrow::<T>()
    }

    pub fn borrow_resource_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.resources.borrow_mut::<T>()
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

// ─── System Trait ─────────────────────────────────────────────────────────

/// 调度器中的单个 System。
///
/// 约束：`Send + Sync + 'static`，为将来并行调度预留接口。
/// 每帧调度器在对应阶段依次调用 [`run`](Self::run)。
/// System 之间禁止直接互调，只能通过 World 组件或 Resources 通信。
pub trait System: Send + Sync + 'static {
    fn run(&mut self, ctx: &mut ExecutorContext<'_>);
}

// ─── IntoSystem ：从闭包自动生成 System ─────────────────────────────────

impl<F> System for F
where
    F: FnMut(&mut ExecutorContext<'_>) + Send + Sync + 'static,
{
    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        (self)(ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resources_insert_and_borrow() {
        let mut r = Resources::new();
        r.insert(42_i32);
        r.insert(3.14_f64);
        assert_eq!(r.borrow::<i32>(), Some(&42));
        assert_eq!(r.borrow::<f64>(), Some(&3.14));
    }

    #[test]
    fn test_resources_borrow_mut() {
        let mut r = Resources::new();
        r.insert(42_i32);
        if let Some(val) = r.borrow_mut::<i32>() {
            *val = 100;
        }
        assert_eq!(r.borrow::<i32>(), Some(&100));
    }

    #[test]
    fn test_resources_missing_returns_none() {
        let r = Resources::new();
        assert!(r.borrow::<i32>().is_none());
    }

    #[test]
    fn test_executor_context_split_borrow() {
        let mut world = hecs::World::new();
        let mut cmd = hecs::CommandBuffer::new();
        let mut resources = Resources::new();
        let mut ctx = ExecutorContext::new(
            &mut world,
            &mut cmd,
            &mut resources,
            0.016,
            0.0,
        );

        // split borrow：同时 &mut world 和 &mut resources
        assert_eq!(ctx.borrow_world().len(), 0);
        let _ = ctx.borrow_resource_mut::<i32>();
    }

    #[test]
    fn test_system_closure_trait() {
        let _s: Box<dyn System> = Box::new(|_ctx: &mut ExecutorContext<'_>| {});
    }
}
