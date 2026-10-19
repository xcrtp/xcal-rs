// capability_demo.rs — N 线「**能力测试 demo**」：用**最小场景**把 N1–N4 四项能力跑通。
//
// # 定位：这不是作品，是**能力检验**
//
// 场景刻意最小（无网格、无公式、无 3D、无 Reveal/Morph），每条能力恰好落在**一处**，
// 便于"哪条能力坏了"一眼定位。四项能力的落点：
//
// | 能力 | 落点 |
// |------|------|
// | N1 `Name` + `find_by_name` | 三条对象各挂 `Name`；装配时以 `find_by_name` 与**脚本句柄**互相印证 |
// | N2 `Group` + `VisibilityTag::Group` | `curve_sin` → `GROUP_SIN`、`curve_cos` → `GROUP_COS`（**两者都要挂**，见下）|
// | N3 `Visibility` | `curve_sin` → `for_camera(本相机)`；对照组 → `for_camera(别的 id)`（应不可见）|
// | N4 `Opacity` | `curve_cos` 的 `Opacity` 由 `Timeline` 从 1.0 淡到 0.0 |
//
// # 三条曲线的角色
//
// - `curve_sin`：**正例** —— 名字引用 + 组标签 + 相机 tag 命中 ⇒ 可见；
// - `curve_cos`：**组控制的对象** —— 淡出（N4）；隐藏 `GROUP_COS` 后应整条消失（N2）；
// - `curve_control`：**反例** —— 只对"另一台（不存在的）相机"可见 ⇒ 任何相机下都不进队列。
//
// # ⚠️ 实测教训（第一次写就踩到的三处）
//
// 1. **光挂 `Group` 不会随组开关走** —— `Group` 只是"我属于哪组"的**组织标签**，
//    真正随开关走的是 `VisibilityTag::Group` 这条**可见性关系**（`Visibility::of_group`）。
//    两个组件**正交**，必须都挂；只挂 `Group` 时隐藏组对画面**毫无影响**（静默无效）。
// 2. **两条曲线相交 ⇒ 像素读数互相污染** —— `sin` 与 `cos` 在 `[-π, π]` 上相交三次，
//    后画的橙线会盖住相交处的青线（实测：隐藏 cos 后青像素 688 → 714）。故本 demo 把
//    `cos` 在竖直方向错开（[`COS_OFFSET`]），使两条曲线互不接触。
// 3. **`Opacity` 组件缺席 ⇒ 动画静默失效** —— `OpacityValue` 只是每帧写字段，
//    `with_component` 在组件缺席时静默跳过（不 panic、不报错），表现为"淡出没反应"。

use xc_notion::{MeasureSpace, Path2D};
use xc_object::{Camera2DHandle, Framing2D, Path};
use xc_render_ir::{Opacity, StrokeSolid};

use crate::animation::OpacityValue;
use crate::contract::{Group, GroupId, GroupVisibility, Name, ObjectId, Visibility, find_by_name};
use crate::handle_ref;
use crate::scene::Scene;
use crate::scene_script::SceneEncoder;
use crate::{AnimValue, BindingTable, ClockForest, Curve, Interp};

/// 主 2D 相机的初始取景（世界原点居中、45 px/世界单位）。
const CAMERA_SCALE: f32 = 45.0;

/// 组号（N2）：两条曲线各属一组 ⇒ 组级开关可整体控制其中一条。
pub const GROUP_SIN: GroupId = GroupId::new(1);
pub const GROUP_COS: GroupId = GroupId::new(2);

/// 名字（N1）：装配后一律**按名字**取回 `ObjectId`，不手递 `spawn` 的返回值。
pub const NAME_SIN: &str = "curve_sin";
pub const NAME_COS: &str = "curve_cos";
pub const NAME_CONTROL: &str = "curve_control";

/// 对照组声明的"另一台相机"——场景里**不存在**这个对象 ⇒ 该 tag 永不命中
/// （N3 的反向判据：`Camera(_)` 必须**精确匹配**，不是"有相机就行"）。
const OTHER_CAMERA: ObjectId = ObjectId::new(999, 1);

/// 描边颜色。像素级验收要靠颜色区分三条曲线，故必须是**可分辨的常量**
/// （青色 / 橙色 / 品红，两两在 RGB 上互斥）。
pub const COLOR_SIN: (u8, u8, u8, u8) = (0, 200, 200, 255);
pub const COLOR_COS: (u8, u8, u8, u8) = (255, 160, 0, 255);
pub const COLOR_CONTROL: (u8, u8, u8, u8) = (255, 0, 255, 255);

/// 曲线采样区间 `[-π, π]` 的等步长点数（与分镜同一口径）。
const STEP_COUNT: usize = 128;

/// `curve_cos` 在竖直方向的整体偏移。
///
/// **为什么不用"原样叠在一起的两条曲线"**：`sin` 与 `cos` 在 `[-π, π]` 上**相交三次**，
/// 后画的橙线会盖掉相交处的青线 ⇒ 任何"这条曲线的像素数"读数都被另一条污染
/// （实测：隐藏 cos 后青像素 688 → 714，差值正是被橙线盖住的那部分）。
/// 最小 demo 里两条曲线**互不接触**，像素判据才是一一对应的。
pub const COS_OFFSET: f32 = -2.5;

/// `curve_cos` 的淡出时长（N4）：`t ∈ [0, FADE_SECONDS]` 把 `Opacity` 从 1.0 推到 0.0。
pub const FADE_SECONDS: f64 = 2.0;

/// demo 产物：与 `storyboard::Storyboard` **同形**（时钟树 + 绑定表 + 主 2D 相机句柄），
/// 外加一个 demo 专属的东西：`group_visibility`（待插入的**组级开关**，默认空 ⇒ 全可见；
/// 测试把它填上以验证 N2）。
///
/// **刻意不导出 `sin_id` / `cos_id`**：demo 自己按名字取回 id 就够了（N1 的用法示范），
/// 消费方（app / 测试）若需要 id，同样应该走 `find_by_name` —— 这才是"声明式引用"。
pub struct CapabilityDemo {
    pub forest: ClockForest,
    pub bindings: BindingTable,
    pub camera: Camera2DHandle,
    pub group_visibility: GroupVisibility,
}

/// 把能力 demo 装配进场景，并返回产物（可渲染性由调用方经既有 System 链路验证）。
///
/// # B0/B1/B2 后：改用**场景脚本**装配（装配代码的简化）
///
/// 改动前是"手工 `scene.spawn` 元组 + `find_by_name` 取 id + 手写 `Timeline` +
/// 手工 `compile`"；现在只声明**句柄 → 组件 → 曲线**：
/// - 对象经 `enc.spawn` 创建 ⇒ 走契约层 `spawn_dyn` ⇒ 过 `add_known` 登记表
///   （"四个组件登记齐备"的可证伪检查）；
/// - N3 的 `Visibility::for_camera` 目标用 [`handle_ref`] **声明期引用**相机句柄，
///   `commit` 时解析为真实 `ObjectId`（B1 句柄引用的真实用例）；
/// - N4 的淡出目标直接是 `cos` 的脚本句柄（声明期无需 `ObjectId`）。
///
/// N1（`find_by_name`）仍成立：commit 后以名字与脚本句柄**互相印证**（见函数尾）。
pub fn build(scene: &mut Scene) -> CapabilityDemo {
    // ── 场景脚本：声明期只有句柄，`commit` 时才创建对象 / 编译时序 ─────────
    let mut enc = SceneEncoder::new();

    // ── 相机（**必须先建**：`Visibility::for_camera` 需要引用它的句柄）──────
    let camera_h = enc.spawn(vec![Box::new(Framing2D::new(
        xc_notion::Vec2::ZERO,
        CAMERA_SCALE,
    ))]);

    // ── N1 + N2 + N3：两条曲线 + 一个对照组 ────────────────────────────────
    // 三条对象各挂 `Name`（N1）；**不接住** `spawn` 的返回值也可以，但这里接住
    // `cos` 的句柄作为 N4 的动画目标。
    enc.spawn(vec![
        Box::new(Path::new(Path2D::from_polyline(&samples(f32::sin), false))),
        Box::new(StrokeSolid::new_in(3.0, COLOR_SIN, MeasureSpace::Screen)),
        Box::new(Name::new(NAME_SIN)),
        Box::new(Group::new(GROUP_SIN)),
        // N3：只对**本相机**可见（声明期句柄引用，commit 时解析）。
        Box::new(Visibility::for_camera(handle_ref(camera_h))),
    ]);
    let cos_h = enc.spawn(vec![
        Box::new(Path::new(Path2D::from_polyline(
            &samples(|x| x.cos() + COS_OFFSET),
            false,
        ))),
        Box::new(StrokeSolid::new_in(3.0, COLOR_COS, MeasureSpace::Screen)),
        Box::new(Name::new(NAME_COS)),
        // ⚠️ 实测发现：**光挂 `Group` 标签**不会让对象随组开关走 —— 组控制是
        // `VisibilityTag::Group` 这条**可见性关系**在起作用，而 `Group` 只是"我属于哪组"
        // 的组织标签（两个组件**正交**）。两者都挂上才等于"这条曲线随该组开关"。
        Box::new(Group::new(GROUP_COS)),
        Box::new(Visibility::of_group(GROUP_COS)),
        // N4：`Opacity` 必须**在 spawn 时就有** —— `OpacityValue` 目标只是每帧写它的
        // 字段；组件缺席时 `with_component` 静默跳过（动画会静默失效）。
        Box::new(Opacity::new(1.0)),
    ]);
    // 对照组：只对**另一台**相机可见 ⇒ 本相机下永不进队列（N3 的反向判据）。
    enc.spawn(vec![
        Box::new(Path::new(Path2D::from_polyline(&control_samples(), false))),
        Box::new(StrokeSolid::new_in(
            3.0,
            COLOR_CONTROL,
            MeasureSpace::Screen,
        )),
        Box::new(Name::new(NAME_CONTROL)),
        Box::new(Visibility::for_camera(OTHER_CAMERA)),
    ]);

    // ── N4：`curve_cos` 的 `Opacity` 1.0 → 0.0（淡出）── 目标 = `cos` 脚本句柄 ──
    enc.animate::<OpacityValue>(cos_h, fade_out(FADE_SECONDS), FADE_SECONDS);

    // ── 提交到**已有 Scene**（不新建）──
    let recipe = enc
        .finish()
        .commit(scene)
        .expect("能力 demo 脚本 commit 成功（组件均已登记）");
    let camera = Camera2DHandle(recipe.id(camera_h).expect("相机句柄已解析"));

    // ── N1：名字引用与脚本句柄**互相印证**（两条引用路径指向同一对象）────────
    // 用 `assert!`（非 `debug_assert!`）：这是 demo 的**能力检验**，不是开发期便利。
    assert_eq!(
        find_by_name(scene, NAME_COS),
        Some(recipe.id(cos_h).expect("cos 句柄已解析")),
        "`find_by_name` 与脚本句柄必须指向同一对象（N1）"
    );

    CapabilityDemo {
        forest: recipe.forest,
        bindings: recipe.bindings,
        camera,
        // N2 的"另一半"：默认**空**（全可见）。要验证组控制就把它填上 ——
        // 消费方的测试即如此（隐藏 `GROUP_COS`）。
        group_visibility: GroupVisibility::new(),
    }
}

// ─── 装配辅助 ─────────────────────────────────────────────────────────────

/// 在 `[-π, π]` 上按 `f` 采样（首尾点分别在 `∓π`）。
fn samples(f: impl Fn(f32) -> f32) -> Vec<xc_notion::Vec2> {
    let step = std::f32::consts::TAU / STEP_COUNT as f32;
    (0..=STEP_COUNT)
        .map(|i| {
            let x = -std::f32::consts::PI + (i as f32) * step;
            xc_notion::Vec2::new(x, f(x))
        })
        .collect()
}

/// 对照组几何：一段远离两条曲线的折线（世界坐标），只在"相机 tag 命中"时才会出现。
fn control_samples() -> Vec<xc_notion::Vec2> {
    vec![
        xc_notion::Vec2::new(-1.0, 3.0),
        xc_notion::Vec2::new(0.0, 3.4),
        xc_notion::Vec2::new(1.0, 3.0),
    ]
}

/// 段内把标量从 1.0 线性插到 0.0（`Keyframes`，定义域是**段内局部时间**）。
fn fade_out(dur: f64) -> Curve {
    Curve::Keyframes {
        keys: vec![(0.0, AnimValue::Scalar(1.0)), (dur, AnimValue::Scalar(0.0))],
        interpolation: Interp::Linear,
    }
}
