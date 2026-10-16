// target.rs — 绑定目标（原 `BindingTarget` 枚举的 **trait 化**形态，P0）。
//
// # 为什么是 trait 而不是枚举（已实测，这是结论不是偏好）
//
// 1. `BindingTable` 是**一个** Resource，装异构的所有 `Binding`，由单个
//    `BindingApplySystem` 遍历；
// 2. 若 `Binding<T: Target>`：表装不下 `Binding<CircleRadius>` 与 `Binding<Camera2D…>`；
// 3. 若 `BindingTable<T>` 也泛型：按 Resources 的 TypeId 机制，不同 `T` 是**不同 Resource**
//    —— 要么注册多个 System，要么一个 System 却无法遍历所有 `T`；
// 4. **异构容器是硬需求 ⇒ 只能类型擦除**，泛型在这里没有落点。
//
// # 为什么 `Arc<dyn Target>` 而不是 `Box<dyn Target>`
//
// `BindingApplySystem` 每帧 `ctx.get::<BindingTable>().cloned()`（见 `system.rs`），
// **`Binding: Clone` 是硬约束**；`Arc` 的 clone 是引用计数，天然满足；`Box` 要写
// `clone_box` 样板，不值。
//
// # 代价（记账，`§3.5`）
//
// 1. **失去 exhaustiveness** —— 不再有"全部已支持 target"的单一清单（编译期不再
//    指出所有 match 处）；这正是要消除的耦合，但 IDE 不再能列出全部；
// 2. `dyn Trait` 不能 serde —— 将来动画脚本若要存盘/序列化，这里会痛；
// 3. `Debug` supertrait 强制所有实现可 `Debug`。

use hecs::World;
use xc_object::{Camera3D, Circle, Framing2D};
use xc_scene_contract::ObjectId;
use xc_time::AnimValue;

use crate::contract::entity_of;
use crate::scene::components::{MorphShape, PhaseAnim};

/// 驱动目标：把 `Curve::sample` 产出的 [`AnimValue`] 写进**某对象**的**某个组件字段**。
///
/// 对象用 [`ObjectId`] 而非 `hecs::Entity`：target 不依赖 hecs，且能接上
/// `ObjectId::is_stale` 的失效检测（`entity_of` 转换在 `contract`）。
pub trait Target: Send + Sync + std::fmt::Debug + 'static {
    /// 把本帧的动画值写入世界。
    ///
    /// **既有语义逐位保留**：曲线输出类型与目标字段不符时**跳过本帧写入，不 panic**。
    fn apply(&self, world: &mut World, v: &AnimValue);

    /// 目标**类别**（无载荷的判别式）。
    ///
    /// 为什么需要它：UI 参数面板（`ParamPanelSystem` / `host::draw_param_panel`）要按
    /// "曲线 × 目标类别"匹配来编辑曲线参数，而 `dyn Target` 无法模式匹配。
    /// 它是**分类**而不是"驱动目标清单"——新增 target 不必改它（UI 不认识就不显示）。
    fn kind(&self) -> TargetKind;
}

/// 目标类别（供 UI 分类；不含载荷）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetKind {
    /// 写 `Circle.radius`（2D 圆半径振荡）。
    CircleRadius,
    /// 写 `PhaseAnim.phase`（函数图像水平采样滑动）。
    Phase,
    /// 写 `Camera3D.eye`（3D 轨道相机）。
    CameraEye,
    /// 写 `MorphShape.t`（形状形变参数）。
    MorphT,
    /// 写 `Framing2D`（2D 相机取景：`Scalar` → `scale`，`Vec2` → `center`）。
    ///
    /// 一个组件 = 一个 target：`Framing2D` 是单 target，**按 [`AnimValue`] 变体分派**，
    /// 将来给相机加字段不必加 target 类型。
    Camera2DFraming,
}

/// 取 `id` 对象上的组件 `T` 并就地修改；对象 / 组件不存在时**静默跳过**（不 panic）。
///
/// 四个实现里"取组件 + 类型匹配 + 赋值"的公共骨架只此一份。
fn with_component<T, F>(world: &mut World, id: ObjectId, f: F)
where
    T: hecs::Component,
    F: FnOnce(&mut T),
{
    let Some(e) = entity_of(id) else {
        return;
    };
    if let Ok(mut c) = world.get::<&mut T>(e) {
        f(&mut c);
    }
}

/// 写 `Circle.radius`（值取 `AnimValue::Scalar`）。
#[derive(Debug, Clone, Copy)]
pub struct CircleRadius {
    pub id: ObjectId,
}

impl Target for CircleRadius {
    fn apply(&self, world: &mut World, v: &AnimValue) {
        let Some(v) = v.try_scalar() else {
            return;
        };
        with_component::<Circle, _>(world, self.id, |c| c.radius = v);
    }

    fn kind(&self) -> TargetKind {
        TargetKind::CircleRadius
    }
}

/// 写 `PhaseAnim.phase`（供函数图像水平采样滑动）。
#[derive(Debug, Clone, Copy)]
pub struct Phase {
    pub id: ObjectId,
}

impl Target for Phase {
    fn apply(&self, world: &mut World, v: &AnimValue) {
        let Some(v) = v.try_scalar() else {
            return;
        };
        with_component::<PhaseAnim, _>(world, self.id, |c| c.phase = v);
    }

    fn kind(&self) -> TargetKind {
        TargetKind::Phase
    }
}

/// 写 `Camera3D.eye`（3D 轨道相机）。
#[derive(Debug, Clone, Copy)]
pub struct CameraEye {
    pub id: ObjectId,
}

impl Target for CameraEye {
    fn apply(&self, world: &mut World, v: &AnimValue) {
        let Some(v) = v.try_vec3() else {
            return;
        };
        with_component::<Camera3D, _>(world, self.id, |c| c.eye = v);
    }

    fn kind(&self) -> TargetKind {
        TargetKind::CameraEye
    }
}

/// 写 `MorphShape.t`（形状形变参数）。
#[derive(Debug, Clone, Copy)]
pub struct MorphT {
    pub id: ObjectId,
}

impl Target for MorphT {
    fn apply(&self, world: &mut World, v: &AnimValue) {
        let Some(v) = v.try_scalar() else {
            return;
        };
        with_component::<MorphShape, _>(world, self.id, |c| c.t = v);
    }

    fn kind(&self) -> TargetKind {
        TargetKind::MorphT
    }
}

/// 写 `Framing2D`（2D 相机取景）：`Scalar` → `scale`、`Vec2` → `center`。
///
/// 一个组件 = 一个 target：相机是**单 target**，按 [`AnimValue`] 变体分派 ——
/// 将来给相机加字段不必加类型（依据：`try_scalar` / `try_vec2` 正好支撑）。
#[derive(Debug, Clone, Copy)]
pub struct Camera2DFraming {
    pub id: ObjectId,
}

impl Target for Camera2DFraming {
    fn apply(&self, world: &mut World, v: &AnimValue) {
        if let Some(scale) = v.try_scalar() {
            with_component::<Framing2D, _>(world, self.id, |f| f.scale = scale);
        } else if let Some(center) = v.try_vec2() {
            with_component::<Framing2D, _>(world, self.id, |f| f.center = center);
        }
    }

    fn kind(&self) -> TargetKind {
        TargetKind::Camera2DFraming
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::{Vec2, Vec3};
    use xc_object::{Camera3D, Circle, Projection};

    /// 契约层句柄（`ObjectId`）↔ hecs 实体的往返。
    fn id_of(e: hecs::Entity) -> ObjectId {
        crate::contract::object_id_of(e)
    }

    /// `CircleRadius`：`Scalar` 写入半径；类型不符跳过（不 panic）。
    #[test]
    fn circle_radius_writes_scalar_and_skips_mismatch() {
        let mut world = World::new();
        let e = world.spawn((Circle::new((0.0, 0.0), 1.0),));
        let t = CircleRadius { id: id_of(e) };

        t.apply(&mut world, &AnimValue::Scalar(2.5));
        assert_eq!(world.get::<&Circle>(e).unwrap().radius, 2.5);

        // Vec3 与目标字段不符 ⇒ 跳过，半径保持不变。
        t.apply(&mut world, &AnimValue::Vec3(Vec3::ONE));
        assert_eq!(world.get::<&Circle>(e).unwrap().radius, 2.5);
    }

    /// `Phase`：写 `PhaseAnim.phase`。
    #[test]
    fn phase_writes_phase_anim() {
        let mut world = World::new();
        let e = world.spawn((PhaseAnim::new(0.0, 1.5),));
        let t = Phase { id: id_of(e) };
        t.apply(&mut world, &AnimValue::Scalar(0.75));
        assert_eq!(world.get::<&PhaseAnim>(e).unwrap().phase, 0.75);
    }

    /// `CameraEye`：写 `Camera3D.eye`。
    #[test]
    fn camera_eye_writes_camera3d() {
        let mut world = World::new();
        let e = world.spawn((Camera3D::new(
            Vec3::ZERO,
            Vec3::ZERO,
            Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        ),));
        let t = CameraEye { id: id_of(e) };
        let eye = Vec3::new(1.0, 2.0, 3.0);
        t.apply(&mut world, &AnimValue::Vec3(eye));
        assert_eq!(world.get::<&Camera3D>(e).unwrap().eye, eye);
    }

    /// `Camera2DFraming`：单 target 按 `AnimValue` 变体分派 —— `Scalar` → `scale`、
    /// `Vec2` → `center`（同一条绑定可以只驱动其中一个）。
    #[test]
    fn camera_framing_dispatches_by_variant() {
        let mut world = World::new();
        let e = world.spawn((Framing2D::new(Vec2::ZERO, 40.0),));
        let t = Camera2DFraming { id: id_of(e) };

        t.apply(&mut world, &AnimValue::Scalar(120.0));
        assert_eq!(world.get::<&Framing2D>(e).unwrap().scale, 120.0);
        assert_eq!(world.get::<&Framing2D>(e).unwrap().center, Vec2::ZERO);

        t.apply(&mut world, &AnimValue::Vec2(Vec2::new(1.5, 1.0)));
        assert_eq!(
            world.get::<&Framing2D>(e).unwrap().center,
            Vec2::new(1.5, 1.0)
        );
        assert_eq!(world.get::<&Framing2D>(e).unwrap().scale, 120.0);
    }

    /// 失效 / 不存在的句柄：静默跳过（不 panic）。
    #[test]
    fn dangling_handle_is_noop() {
        let mut world = World::new();
        let t = CircleRadius {
            id: ObjectId::DANGLING,
        };
        t.apply(&mut world, &AnimValue::Scalar(1.0));
    }

    /// `kind()` 与实现的对应关系（UI 参数面板按它分类）。
    #[test]
    fn kinds_are_distinct() {
        let id = ObjectId::DANGLING;
        assert_eq!(CircleRadius { id }.kind(), TargetKind::CircleRadius);
        assert_eq!(Phase { id }.kind(), TargetKind::Phase);
        assert_eq!(CameraEye { id }.kind(), TargetKind::CameraEye);
        assert_eq!(MorphT { id }.kind(), TargetKind::MorphT);
        assert_eq!(
            Camera2DFraming { id }.kind(),
            TargetKind::Camera2DFraming
        );
    }
}
