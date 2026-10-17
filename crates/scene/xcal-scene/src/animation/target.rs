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
use std::sync::Arc;
use xc_notion::Vec3;
use xc_object::{Camera3D, Circle, Framing2D, Transform};
use xc_render_ir::Opacity;
use xc_scene_contract::ObjectId;
use xc_time::AnimValue;

use crate::contract::entity_of;
use crate::scene::components::{MorphShape, PhaseAnim, Reveal};

/// 驱动目标：把 `Curve::sample` 产出的 [`AnimValue`] 写进**某对象**的**某个组件字段**。
///
/// 对象用 [`ObjectId`] 而非 `hecs::Entity`：target 不依赖 hecs，且能接上
/// `ObjectId::is_stale` 的失效检测（`entity_of` 转换在 `contract`）。
pub trait Target: Send + Sync + std::fmt::Debug + 'static {
    /// 把本帧的动画值写入世界。
    ///
    /// **既有语义逐位保留**：曲线输出类型与目标字段不符时**跳过本帧写入，不 panic**。
    fn apply(&self, world: &mut World, v: &AnimValue);

    /// 目标**语义名**（如 `"CircleRadius"`）—— 供 UI 显示 / 分类。
    ///
    /// **由 target 自带**：新增 target 只需在实现里写一行字面量，
    /// **不改任何集中清单**（这正是删掉集中分类枚举的收益）。
    /// UI 面板可按此名做增强映射（如中文标签），**缺失时 fallback 到本名**，
    /// ⇒ 映射表是可选增强，不是必需清单。
    fn name(&self) -> &'static str;

    /// 目标作用的**对象句柄** —— 供"按对象筛选绑定"用
    /// （例如"作用于这台 2D 相机的绑定"，比按类别筛选更精确）。
    fn target_id(&self) -> ObjectId;
}

/// **声明期可引用的动画目标构造器**（场景脚本 P6）。
///
/// `Target::apply(&self, ..)` 的全部实现都**持有 `ObjectId`**；而场景脚本的声明期
/// 还没有 `ObjectId`（`commit` 时才分配）。本 trait 让"目标"在声明期只以**类型**出现：
/// `commit` 时按真实 id 调 `Self::make(id)` 生成 trait object。
///
/// # 为什么是 trait 而不是闭包 / 枚举（§2.2）
///
/// 1. 闭包 `Box<dyn FnOnce(ObjectId) -> Arc<dyn Target>>` 要 `Send + Sync` 才能进
///    `SceneScript`，而带捕获的闭包需要额外约束；
/// 2. 枚举所有 target ⇒ 退化成"加一个改一处"。
///
/// 本 trait 的 `make` 是**关联函数**（语义上等价于函数指针，不捕获环境）⇒ 天然
/// `Send + Sync + 'static`，且新增 target 只需多写一行 `impl` ⇒ **保持开放**。
/// [`Target`] trait 本身**零改动**，运行时零额外开销（`commit` 时构造一次）。
pub trait TargetCtor: Send + Sync + 'static {
    /// 按真实 [`ObjectId`] 生成驱动目标。
    fn make(id: ObjectId) -> Arc<dyn Target>;
}

impl TargetCtor for CircleRadius {
    fn make(id: ObjectId) -> Arc<dyn Target> {
        Arc::new(Self { id })
    }
}

impl TargetCtor for Phase {
    fn make(id: ObjectId) -> Arc<dyn Target> {
        Arc::new(Self { id })
    }
}

impl TargetCtor for CameraEye {
    fn make(id: ObjectId) -> Arc<dyn Target> {
        Arc::new(Self { id })
    }
}

impl TargetCtor for MorphT {
    fn make(id: ObjectId) -> Arc<dyn Target> {
        Arc::new(Self { id })
    }
}

impl TargetCtor for Camera2DFraming {
    fn make(id: ObjectId) -> Arc<dyn Target> {
        Arc::new(Self { id })
    }
}

impl TargetCtor for RevealRange {
    fn make(id: ObjectId) -> Arc<dyn Target> {
        Arc::new(Self { id })
    }
}

impl TargetCtor for OpacityValue {
    fn make(id: ObjectId) -> Arc<dyn Target> {
        Arc::new(Self { id })
    }
}

impl TargetCtor for TransformValue {
    fn make(id: ObjectId) -> Arc<dyn Target> {
        Arc::new(Self { id })
    }
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

    fn name(&self) -> &'static str {
        "CircleRadius"
    }

    fn target_id(&self) -> ObjectId {
        self.id
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

    fn name(&self) -> &'static str {
        "Phase"
    }

    fn target_id(&self) -> ObjectId {
        self.id
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

    fn name(&self) -> &'static str {
        "CameraEye"
    }

    fn target_id(&self) -> ObjectId {
        self.id
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

    fn name(&self) -> &'static str {
        "MorphT"
    }

    fn target_id(&self) -> ObjectId {
        self.id
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

    fn name(&self) -> &'static str {
        "Camera2DFraming"
    }

    fn target_id(&self) -> ObjectId {
        self.id
    }
}

/// 写 [`Reveal`] 的可见区间（书写动画，R0）。
///
/// - `Scalar(v)` → `end = v`（**"笔尖往前走"**：`start` 保持不动）；
/// - `Vec2(v)` → `(start, end) = (v.x, v.y)`（区间版本："只写中段"）。
///
/// 与 `Camera2DFraming` 同口径：类型不符**静默跳过**，不 panic。
#[derive(Debug, Clone, Copy)]
pub struct RevealRange {
    pub id: ObjectId,
}

impl Target for RevealRange {
    fn apply(&self, world: &mut World, v: &AnimValue) {
        if let Some(end) = v.try_scalar() {
            with_component::<Reveal, _>(world, self.id, |r| r.end = end);
        } else if let Some(range) = v.try_vec2() {
            with_component::<Reveal, _>(world, self.id, |r| {
                r.start = range.x;
                r.end = range.y;
            });
        }
    }

    fn name(&self) -> &'static str {
        "RevealRange"
    }

    fn target_id(&self) -> ObjectId {
        self.id
    }
}

/// 写 [`Transform`]（T1 的逐对象摆放通道）。
///
/// 按 [`AnimValue`] 变体分派（与 `Camera2DFraming` / `RevealRange` 同口径）：
/// - `Vec2(v)` → `position.xy`（z 不变）；
/// - `Vec3(v)` → `position`（整体）；
/// - `Scalar(v)` → **统一缩放**（`scale = Vec3::splat(v)`）；
/// - 其它变体静默跳过（不 panic）。
///
/// **不做旋转**：`Scalar` 已被统一缩放占用（见计划 §2.3 记账 —— 需要旋转时再拆
/// `PositionValue` / `ScaleValue` / `RotationValue` 三个 target）。
/// **也不做非统一缩放**（`scale.xy` 各自动画）：触发条件同上。
#[derive(Debug, Clone, Copy)]
pub struct TransformValue {
    pub id: ObjectId,
}

impl Target for TransformValue {
    fn apply(&self, world: &mut World, v: &AnimValue) {
        if let Some(p) = v.try_vec2() {
            with_component::<Transform, _>(world, self.id, |t| {
                t.position.x = p.x;
                t.position.y = p.y;
            });
        } else if let Some(p) = v.try_vec3() {
            with_component::<Transform, _>(world, self.id, |t| t.position = p);
        } else if let Some(s) = v.try_scalar() {
            with_component::<Transform, _>(world, self.id, |t| t.scale = Vec3::splat(s));
        }
    }

    fn name(&self) -> &'static str {
        "TransformValue"
    }

    fn target_id(&self) -> ObjectId {
        self.id
    }
}

/// 写 [`Opacity`]（对象级不透明度，N4 的淡入淡出通道）。
///
/// - `Scalar(v)` → `Opacity.0 = v`（`0.0` 全透明 … `1.0` 完全不透明）；
/// - 其它变体**静默跳过**（不 panic），与全部既有 target 同口径。
///
/// 施加位置不在本 target：它只写**组件值**，由录制路径
/// （`xcal_render::draw::apply_opacity`）乘进样式颜色 —— 唯一施加点纪律。
#[derive(Debug, Clone, Copy)]
pub struct OpacityValue {
    pub id: ObjectId,
}

impl Target for OpacityValue {
    fn apply(&self, world: &mut World, v: &AnimValue) {
        let Some(v) = v.try_scalar() else {
            return;
        };
        with_component::<Opacity, _>(world, self.id, |o| o.0 = v);
    }

    fn name(&self) -> &'static str {
        "OpacityValue"
    }

    fn target_id(&self) -> ObjectId {
        self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_notion::{Vec2, Vec3};
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

    /// `RevealRange`：`Scalar` 只推 `end`（"笔尖往前走"），`Vec2` 写整个区间。
    #[test]
    fn reveal_range_dispatches_by_variant() {
        let mut world = World::new();
        let e = world.spawn((crate::scene::components::Reveal::new(
            xc_notion::Path2D::from_polyline(
                &[xc_notion::Vec2::ZERO, xc_notion::Vec2::new(1.0, 0.0)],
                false,
            ),
        ),));
        let t = RevealRange { id: id_of(e) };

        t.apply(&mut world, &AnimValue::Scalar(0.4));
        {
            let r = world.get::<&crate::scene::components::Reveal>(e).unwrap();
            assert_eq!(r.end, 0.4);
            assert_eq!(r.start, 0.0, "Scalar 不得改动 start");
        }

        t.apply(&mut world, &AnimValue::Vec2(Vec2::new(0.3, 0.7)));
        {
            let r = world.get::<&crate::scene::components::Reveal>(e).unwrap();
            assert_eq!((r.start, r.end), (0.3, 0.7));
        }

        // 类型不符 ⇒ 跳过（保持上一帧的值）。
        t.apply(&mut world, &AnimValue::Vec3(Vec3::ONE));
        {
            let r = world.get::<&crate::scene::components::Reveal>(e).unwrap();
            assert_eq!((r.start, r.end), (0.3, 0.7));
        }
    }

    /// `name()` / `target_id()` 与实现的对应关系（UI 显示 + 按对象筛选）。
    ///
    /// ⚠️ 这不是"集中清单"——名字**由各 target 自带**；本用例只是把**既有**七个的
    /// 字面量钉住（防手滑改错），新增 target **不需要**在这里加一行。
    #[test]
    fn names_and_target_ids_are_reported() {
        let id = ObjectId::new(7, 3);
        assert_eq!(CircleRadius { id }.name(), "CircleRadius");
        assert_eq!(Phase { id }.name(), "Phase");
        assert_eq!(CameraEye { id }.name(), "CameraEye");
        assert_eq!(MorphT { id }.name(), "MorphT");
        assert_eq!(Camera2DFraming { id }.name(), "Camera2DFraming");
        assert_eq!(RevealRange { id }.name(), "RevealRange");
        assert_eq!(OpacityValue { id }.name(), "OpacityValue");
        assert_eq!(TransformValue { id }.name(), "TransformValue");
        // 全部 target 都如实报告自己的对象句柄（筛选的唯一依据）。
        assert_eq!(CircleRadius { id }.target_id(), id);
        assert_eq!(TransformValue { id }.target_id(), id);
        assert_eq!(OpacityValue { id }.target_id(), id);
    }

    /// `TransformValue`：按 `AnimValue` 变体分派 —— `Vec2` → `position.xy`（z 不动）、
    /// `Vec3` → 整个 `position`、`Scalar` → 统一缩放。
    #[test]
    fn transform_value_dispatches_by_variant() {
        let mut world = World::new();
        let e = world.spawn((Transform::default(),));
        let t = TransformValue { id: id_of(e) };
        let read = |world: &World| {
            let tr = world.get::<&Transform>(e).unwrap();
            (tr.position, tr.scale)
        };

        // 只动 xy，z 保持。
        t.apply(&mut world, &AnimValue::Vec2(Vec2::new(2.0, -3.0)));
        assert_eq!(read(&world).0, Vec3::new(2.0, -3.0, 0.0));
        assert_eq!(read(&world).1, Vec3::ONE, "Vec2 不得改缩放");

        // 整体写位置。
        t.apply(&mut world, &AnimValue::Vec3(Vec3::new(1.0, 2.0, 3.0)));
        assert_eq!(read(&world).0, Vec3::new(1.0, 2.0, 3.0));

        // 统一缩放。
        t.apply(&mut world, &AnimValue::Scalar(2.5));
        assert_eq!(read(&world).1, Vec3::splat(2.5));
        assert_eq!(read(&world).0, Vec3::new(1.0, 2.0, 3.0), "缩放不改位置");
    }

    /// `OpacityValue`：`Scalar` 写入 `Opacity.0`；其它变体跳过（不 panic）。
    #[test]
    fn opacity_value_writes_scalar_and_skips_mismatch() {
        let mut world = World::new();
        let e = world.spawn((Opacity::new(1.0),));
        let t = OpacityValue { id: id_of(e) };

        t.apply(&mut world, &AnimValue::Scalar(0.25));
        assert_eq!(world.get::<&Opacity>(e).unwrap().0, 0.25);

        t.apply(&mut world, &AnimValue::Vec2(Vec2::new(0.1, 0.2)));
        assert_eq!(
            world.get::<&Opacity>(e).unwrap().0,
            0.25,
            "非 Scalar 变体不得写入（静默跳过）"
        );
    }
}
