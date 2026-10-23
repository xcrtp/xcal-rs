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

use std::sync::Arc;
use ze_notion::Vec3;
use crate::object::{Camera3D, Circle, Framing2D, Transform};
use ze_pod_macro::PodType;
use ze_render_ir::Opacity;
use ze_scene_contract::ObjectId;
use ze_time::AnimValue;

// B3-1b：写入经 `SceneWrite`（类型擦除）—— 本文件不认识 hecs，也不认识任何具体 ECS。
use crate::scene_write::with_component;
use crate::scene_write::SceneWrite;
use crate::components::{MorphShape, PhaseAnim, Reveal};

/// 驱动目标：把 `Curve::sample` 产出的 [`AnimValue`] 写进**某对象**的**某个组件字段**。
///
/// 对象用 [`ObjectId`] 而非 `hecs::Entity`：target 不依赖 hecs，且能接上
/// `ObjectId::is_stale` 的失效检测（`entity_of` 转换在 `contract`）。
pub trait Target: Send + Sync + std::fmt::Debug + 'static {
    /// 把本帧的动画值写入世界。
    ///
    /// **既有语义逐位保留**：曲线输出类型与目标字段不符时**跳过本帧写入，不 panic**。
    ///
    /// # 去 hecs 化（B3-1b）
    ///
    /// 参数是核心层的 [`SceneWrite`]（按 `TypeId` 交出组件），本文件**不认识 hecs**，
    /// 也不认识任何具体 ECS 适配器。
    ///
    /// 为什么不是 `&mut dyn Scene`：`Scene` 含泛型访客方法（`for_each` / `with` …），
    /// **不 object-safe**；而 `Target` 必须保持 object-safe（`BindingTable` 装
    /// `Arc<dyn Target>`）。`SceneWrite` 是 object-safe 的擦除写入口，
    /// 实现方（ECS）用组件登记表做具体分发。
    fn apply(&self, scene: &mut dyn SceneWrite, v: &AnimValue);

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

/// **动画目标的稳定枚举**（持久化 + 脚本绑定的**唯一真源**）。
///
/// 变体名即持久化名字；[`Self::ctor`] 是唯一的 variant → 构造器映射。
/// 新增 target 只需加一个变体 —— 编译器会强制补全 `name()` / `ctor()` 两个
/// 穷尽 match（不存在第三张字符串表）。
///
/// 注意：它不能叫 `Target` —— 同名 trait 已存在（本枚举是"目标种类"）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, PodType)]
#[pod_type(name = "zela.Target")]
pub enum TargetKind {
    CircleRadius,
    Phase,
    CameraEye,
    MorphT,
    Camera2DFraming,
    RevealRange,
    OpacityValue,
}

impl TargetKind {
    /// 全部变体（文档 / 校验用）。
    pub const ALL: &'static [Self] = &[
        Self::CircleRadius,
        Self::Phase,
        Self::CameraEye,
        Self::MorphT,
        Self::Camera2DFraming,
        Self::RevealRange,
        Self::OpacityValue,
    ];

    /// 稳定语义名（= 变体名；持久化只写它）。
    pub const fn name(self) -> &'static str {
        match self {
            Self::CircleRadius => "CircleRadius",
            Self::Phase => "Phase",
            Self::CameraEye => "CameraEye",
            Self::MorphT => "MorphT",
            Self::Camera2DFraming => "Camera2DFraming",
            Self::RevealRange => "RevealRange",
            Self::OpacityValue => "OpacityValue",
        }
    }

    /// 声明期构造器（`commit` 时按真实 `ObjectId` 生成 trait object）。
    pub fn ctor(self) -> fn(ObjectId) -> Arc<dyn Target> {
        match self {
            Self::CircleRadius => CircleRadius::make,
            Self::Phase => Phase::make,
            Self::CameraEye => CameraEye::make,
            Self::MorphT => MorphT::make,
            Self::Camera2DFraming => Camera2DFraming::make,
            Self::RevealRange => RevealRange::make,
            Self::OpacityValue => OpacityValue::make,
        }
    }

    /// 名字 → 枚举（未知名字 ⇒ `None`）。
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|kind| kind.name() == name)
    }
}

/// **反序列化用**：按 target 名字取构造器（名字 ↔ 构造器唯一映射）。
///
/// 现在它只是 [`TargetKind`] 的薄查询：字符串表已删除，枚举是唯一真源。
/// 返回值同时给出 `&'static str`，供反序列化构造 `ActionTarget::Pending.name`。
pub fn target_ctor_by_name(name: &str) -> Option<(fn(ObjectId) -> Arc<dyn Target>, &'static str)> {
    // S3：Transform 的显式通道也走同一张表（名字稳定 ⇒ 可持久化 / 可反序列化）。
    match name {
        "TransformPosition" => return Some((make_transform_position, "TransformPosition")),
        "TransformRotation" => return Some((make_transform_rotation, "TransformRotation")),
        "TransformScale" => return Some((make_transform_scale, "TransformScale")),
        _ => {}
    }
    TargetKind::from_name(name).map(|kind| (kind.ctor(), kind.name()))
}

/// 写 `Circle.radius`（值取 `AnimValue::Scalar`）。
#[derive(Debug, Clone, Copy)]
pub struct CircleRadius {
    pub id: ObjectId,
}

impl Target for CircleRadius {
    fn apply(&self, scene: &mut dyn SceneWrite, v: &AnimValue) {
        let Some(v) = v.try_scalar() else {
            return;
        };
        with_component::<Circle, _>(scene, self.id, |c| c.radius = v);
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
    fn apply(&self, scene: &mut dyn SceneWrite, v: &AnimValue) {
        let Some(v) = v.try_scalar() else {
            return;
        };
        with_component::<PhaseAnim, _>(scene, self.id, |c| c.phase = v);
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
    fn apply(&self, scene: &mut dyn SceneWrite, v: &AnimValue) {
        let Some(v) = v.try_vec3() else {
            return;
        };
        with_component::<Camera3D, _>(scene, self.id, |c| c.eye = v);
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
    fn apply(&self, scene: &mut dyn SceneWrite, v: &AnimValue) {
        let Some(v) = v.try_scalar() else {
            return;
        };
        with_component::<MorphShape, _>(scene, self.id, |c| c.t = v);
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
    fn apply(&self, scene: &mut dyn SceneWrite, v: &AnimValue) {
        if let Some(scale) = v.try_scalar() {
            with_component::<Framing2D, _>(scene, self.id, |f| f.scale = scale);
        } else if let Some(center) = v.try_vec2() {
            with_component::<Framing2D, _>(scene, self.id, |f| f.center = center);
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
    fn apply(&self, scene: &mut dyn SceneWrite, v: &AnimValue) {
        if let Some(end) = v.try_scalar() {
            with_component::<Reveal, _>(scene, self.id, |r| r.end = end);
        } else if let Some(range) = v.try_vec2() {
            with_component::<Reveal, _>(scene, self.id, |r| {
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

/// `Transform` 的**写入通道**（S3：统一 Transform 动画）。
///
/// B7：`Auto`（按 `AnimValue` 类型猜写哪一列、猜不中静默跳过、且没有 Rotation 分支）
/// 已删除 —— 作者必须显式声明写哪一列。三条显式通道只写自己那一列，其余字段一概
/// 不碰 ⇒ 多条绑定（如 `Position` + `Rotation`）可并行而不互相覆盖。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformChannel {
    /// 只写 `position`（`Vec2` 写 xy，`Vec3` 整体）。
    Position,
    /// 只写 `rotation`（`Scalar` = 绕 Z 的弧度）。
    Rotation,
    /// 只写 `scale`（`Scalar` = 统一缩放）。
    Scale,
}

/// 写 [`Transform`] 的某一列（T1 的逐对象摆放通道）。
///
/// 按 [`TransformChannel`] 决定写哪一列：
/// - `Position`：`Vec2` / `Vec3` → `position`；
/// - `Rotation`：`Scalar` → `Quat::from_rotation_z(rad)`（绕 Z）；
/// - `Scale`：`Scalar` → `Vec3::splat(v)`。
///
/// 其它组合静默跳过（与全部既有 target 同口径）。组合顺序仍是 `scale → rotation →
/// translation`（见 [`Transform::to_affine2d`]），本 target 只写字段，不决定顺序。
#[derive(Debug, Clone, Copy)]
pub struct TransformChannelValue {
    pub id: ObjectId,
    pub channel: TransformChannel,
}

impl Target for TransformChannelValue {
    fn apply(&self, scene: &mut dyn SceneWrite, v: &AnimValue) {
        match self.channel {
            TransformChannel::Position => {
                if let Some(p) = v.try_vec2() {
                    with_component::<Transform, _>(scene, self.id, |t| {
                        t.position.x = p.x;
                        t.position.y = p.y;
                    });
                } else if let Some(p) = v.try_vec3() {
                    with_component::<Transform, _>(scene, self.id, |t| t.position = p);
                }
            }
            TransformChannel::Rotation => {
                if let Some(rad) = v.try_scalar() {
                    with_component::<Transform, _>(scene, self.id, |t| {
                        t.rotation = ze_notion::Quat::from_rotation_z(rad);
                    });
                }
            }
            TransformChannel::Scale => {
                if let Some(s) = v.try_scalar() {
                    with_component::<Transform, _>(scene, self.id, |t| t.scale = Vec3::splat(s));
                }
            }
        }
    }

    fn name(&self) -> &'static str {
        match self.channel {
            TransformChannel::Position => "TransformPosition",
            TransformChannel::Rotation => "TransformRotation",
            TransformChannel::Scale => "TransformScale",
        }
    }

    fn target_id(&self) -> ObjectId {
        self.id
    }
}

/// 通道化的 target 构造器（`target_ctor_by_name` 与脚本 `clip_transform` 共用）。
pub fn make_transform_position(id: ObjectId) -> Arc<dyn Target> {
    Arc::new(TransformChannelValue {
        id,
        channel: TransformChannel::Position,
    })
}

pub fn make_transform_rotation(id: ObjectId) -> Arc<dyn Target> {
    Arc::new(TransformChannelValue {
        id,
        channel: TransformChannel::Rotation,
    })
}

pub fn make_transform_scale(id: ObjectId) -> Arc<dyn Target> {
    Arc::new(TransformChannelValue {
        id,
        channel: TransformChannel::Scale,
    })
}

/// 写 [`Opacity`]（对象级不透明度，N4 的淡入淡出通道）。
///
/// - `Scalar(v)` → `Opacity.0 = v`（`0.0` 全透明 … `1.0` 完全不透明）；
/// - 其它变体**静默跳过**（不 panic），与全部既有 target 同口径。
///
/// 施加位置不在本 target：它只写**组件值**，由录制路径
/// （`zela_render::draw::apply_opacity`）乘进样式颜色 —— 唯一施加点纪律。
#[derive(Debug, Clone, Copy)]
pub struct OpacityValue {
    pub id: ObjectId,
}

impl Target for OpacityValue {
    fn apply(&self, scene: &mut dyn SceneWrite, v: &AnimValue) {
        let Some(v) = v.try_scalar() else {
            return;
        };
        with_component::<Opacity, _>(scene, self.id, |o| o.0 = v);
    }

    fn name(&self) -> &'static str {
        "OpacityValue"
    }

    fn target_id(&self) -> ObjectId {
        self.id
    }
}
