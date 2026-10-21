// scene_write.rs — 对象组件写入的**类型擦除入口**（B3-1b）
//
// # 为什么需要这一层
// 动画目标（`animation::target`）要按 `ObjectId` 就地写组件，但它属于**核心层**，
// 不得认识 ECS（hecs）或 `WorldScene` 适配器。契约层的 `Scene::get_mut<T>` 是
// 泛型方法，无法经 `dyn` 调用；而 `Target` 又必须是 trait object（`Arc<dyn Target>`）。
//
// ⇒ 用 object-safe 的 [`SceneWrite`] 做桥：按 `TypeId` 交出 `&mut dyn Any`，
// 由实现方（ECS）用组件登记表做具体分发。核心层只写一套 [`with_component`]。
//
// 未命中（对象不存在 / 未挂该组件）⇒ **静默跳过**（与迁移前的 `get_mut` 语义一致：
// 动画驱动的是"可能存在"的组件，缺席不是错误）。

use std::any::{Any, TypeId};

use ze_scene_contract::ObjectId;

/// 组件写入的类型擦除接口（object-safe）。
///
/// 实现方负责 `type_id` → 具体组件类型的分发；`f` 收到的是**已解引用**的
/// `&mut dyn Any`，调用方自行 `downcast_mut::<T>()`。
pub trait SceneWrite {
    /// 对 `id` 上类型为 `type_id` 的组件调用 `f`；对象或组件不存在 ⇒ `false`。
    fn with_component_mut(
        &mut self,
        id: ObjectId,
        type_id: TypeId,
        f: &mut dyn FnMut(&mut dyn Any),
    ) -> bool;
}

/// 取组件 `T` 并就地修改；对象 / 组件不存在时**静默跳过**（不 panic）。
pub fn with_component<T, F>(scene: &mut dyn SceneWrite, id: ObjectId, f: F)
where
    T: 'static + Send + Sync,
    F: FnOnce(&mut T),
{
    let mut f = Some(f);
    let mut cb = |any: &mut dyn Any| {
        if let Some(v) = any.downcast_mut::<T>() {
            if let Some(f) = f.take() {
                f(v);
            }
        }
    };
    let _ = scene.with_component_mut(id, TypeId::of::<T>(), &mut cb);
}
