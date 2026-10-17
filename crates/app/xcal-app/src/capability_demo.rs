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
// # ⚠️ 实测教训（第一次写就踩到的三处，见交付说明）
//
// 1. **光挂 `Group` 不会随组开关走** —— `Group` 只是"我属于哪组"的**组织标签**，
//    真正随开关走的是 `VisibilityTag::Group` 这条**可见性关系**（`Visibility::of_group`）。
//    两个组件**正交**，必须都挂；只挂 `Group` 时隐藏组对画面**毫无影响**（静默无效）。
// 2. **两条曲线相交 ⇒ 像素读数互相污染** —— `sin` 与 `cos` 在 `[-π, π]` 上相交三次，
//    后画的橙线会盖住相交处的青线（实测：隐藏 cos 后青像素 688 → 714）。故本 demo 把
//    `cos` 在竖直方向错开（[`COS_OFFSET`]），使两条曲线互不接触。
// 3. **`Opacity` 组件缺席 ⇒ 动画静默失效** —— `OpacityValue` 只是每帧写字段，
//    `with_component` 在组件缺席时静默跳过（不 panic、不报错），表现为"淡出没反应"。
//
// # 装配口径（B0/B1/B2 后更新）
//
// - 对象经**场景脚本** `SceneEncoder` 创建（`enc.spawn` → 句柄 → `commit`）：
//   走契约层 `spawn_dyn` ⇒ 过 `add_known` 登记表 —— 这正是"四个组件登记齐备"的
//   可证伪检查（漏登记 ⇒ `commit` 直接 `Err`，不再靠运气）。
// - 相机（N3 的 `Visibility::for_camera` 目标）以 [`handle_ref`](xcal_scene::handle_ref)
//   **声明期引用**，`commit` 时解析为真实 `ObjectId`。
// - N1 的 `find_by_name` 仍成立：`commit` 后以名字与脚本句柄**互相印证**。
//
// # 已知的比较别扭之处（交付说明里逐条展开）
//
// - **N2 与 N4 落在同一对象上**：`curve_cos` 既要"能淡出"又要"能被组开关关掉"，
//   于是两者无法**同时**目视验证（组关掉 ⇒ 看不到淡出）。运行态只演示淡出，
//   组开关由测试与 GPU 用例证明。
// - 场景只有**一台** 2D 相机 ⇒ 相机过滤只能验证"命中 ⇒ 可见 / 不命中 ⇒ 不可见"，
//   双相机切换本轮不做（记账）。

use xc_notion::Path2D;
use xc_object::{Camera2DHandle, Framing2D};
use xcal_render::components::Path;
use xcal_render::{MeasureSpace, Opacity, StrokeSolid};
use xcal_scene::animation::OpacityValue;
use xcal_scene::contract::{
    Group, GroupId, GroupVisibility, Name, ObjectId, Visibility, find_by_name,
};
use xcal_scene::handle_ref;
use xcal_scene::scene::Scene;
use xcal_scene::scene_script::SceneEncoder;
use xcal_scene::{AnimValue, BindingTable, ClockForest, Curve, Interp};

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
const COS_OFFSET: f32 = -2.5;

/// `curve_cos` 的淡出时长（N4）：`t ∈ [0, FADE_SECONDS]` 把 `Opacity` 从 1.0 推到 0.0。
const FADE_SECONDS: f64 = 2.0;

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
        glam::Vec2::ZERO,
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
        // 本模块的测试即如此（隐藏 `GROUP_COS`）。
        group_visibility: GroupVisibility::new(),
    }
}

// ─── 装配辅助 ─────────────────────────────────────────────────────────────

/// 在 `[-π, π]` 上按 `f` 采样（首尾点分别在 `∓π`）。
fn samples(f: impl Fn(f32) -> f32) -> Vec<glam::Vec2> {
    let step = std::f32::consts::TAU / STEP_COUNT as f32;
    (0..=STEP_COUNT)
        .map(|i| {
            let x = -std::f32::consts::PI + (i as f32) * step;
            glam::Vec2::new(x, f(x))
        })
        .collect()
}

/// 对照组几何：一段远离两条曲线的折线（世界坐标），只在"相机 tag 命中"时才会出现。
fn control_samples() -> Vec<glam::Vec2> {
    vec![
        glam::Vec2::new(-1.0, 3.0),
        glam::Vec2::new(0.0, 3.4),
        glam::Vec2::new(1.0, 3.0),
    ]
}

/// 段内把标量从 1.0 线性插到 0.0（`Keyframes`，定义域是**段内局部时间**）。
fn fade_out(dur: f64) -> Curve {
    Curve::Keyframes {
        keys: vec![(0.0, AnimValue::Scalar(1.0)), (dur, AnimValue::Scalar(0.0))],
        interpolation: Interp::Linear,
    }
}

// ─── 测试：验收 1（headless 断言） + 验收 2（GPU 像素级） ───────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use xc_object::Camera2D;
    use xcal_render::{DrawQueue, SceneRecordSystem, ViewRect, record_scene_tagged};
    use xcal_scene::TimeState;
    use xcal_scene::animation::BindingApplySystem;
    use xcal_scene::contract::{SceneRead, WorldView, entity_of};
    use xcal_scene::types::Rect2D;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};

    /// 录制测试用视口（`800×600`，与 `ViewRect::default` 无关，显式给出以免依赖缺省值）。
    fn view_rect() -> Rect2D {
        Rect2D::new(
            xcal_scene::types::Vec2::ZERO,
            xcal_scene::types::Vec2::new(800.0, 600.0),
        )
    }

    /// 把 demo 场景灌进真实 System 链路（`SceneRecordSystem`）跑一帧，返回队列里的描边色。
    ///
    /// 走 System 而不是直接调 `record_scene_tagged`：这样验的是**接线**（`Reads` 里的
    /// `GroupVisibility` 真的被读到了、`camera_tag` 真的从 `Camera2DHandle` 取了）。
    fn recorded_stroke_colors(extra: impl FnOnce(&mut Resources)) -> Vec<(u8, u8, u8, u8)> {
        let mut scene = Scene::new();
        let d = build(&mut scene);
        let world = scene.into_world();

        let mut res = Resources::new();
        res.insert(d.camera);
        res.insert(ViewRect { rect: view_rect() });
        res.insert(DrawQueue::new());
        res.insert(d.group_visibility);
        extra(&mut res);

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::RenderCommit, SceneRecordSystem);
        sched.init_resources();
        sched.run_frame(0.0);

        sched
            .resources()
            .get::<DrawQueue>()
            .unwrap()
            .commands()
            .iter()
            .filter_map(|c| c.style.stroke)
            .map(|c| (c.r(), c.g(), c.b(), c.a()))
            .collect()
    }

    /// **N1**：`find_by_name` 取回的 id 确实指向同名对象（且三条名字互不混淆）。
    #[test]
    fn find_by_name_returns_the_named_objects() {
        let mut scene = Scene::new();
        let _ = build(&mut scene);

        let sin = find_by_name(&scene, NAME_SIN).expect("curve_sin");
        let cos = find_by_name(&scene, NAME_COS).expect("curve_cos");
        let ctl = find_by_name(&scene, NAME_CONTROL).expect("curve_control");
        assert_ne!(sin, cos);
        assert_ne!(sin, ctl);
        assert_ne!(cos, ctl);

        // 取回的对象确实**就是这个名字、且真有几何**（不是只对上名字的空壳）。
        let name_of = |id| scene.with::<Name, _>(id, |n| n.as_str().to_string());
        assert_eq!(name_of(sin).as_deref(), Some(NAME_SIN));
        assert_eq!(name_of(cos).as_deref(), Some(NAME_COS));
        assert_eq!(name_of(ctl).as_deref(), Some(NAME_CONTROL));
        assert!(
            scene.with::<Path, _>(sin, |_| ()).is_some(),
            "curve_sin 应有 Path"
        );
        assert!(
            scene.with::<Path, _>(cos, |_| ()).is_some(),
            "curve_cos 应有 Path"
        );

        assert_eq!(
            find_by_name(&scene, "不存在的名字"),
            None,
            "未知名字 ⇒ None"
        );
    }

    /// **N3**：`Visibility::for_camera` 精确匹配 —— 相机 tag 命中才进队列。
    ///
    /// 对照组声明的是"另一台相机"⇒ **任何**传入的 tag 都不该让它出现（场景里根本没有那台相机）。
    #[test]
    fn camera_tag_is_matched_exactly() {
        let colors = recorded_stroke_colors(|_| {});
        assert_eq!(
            colors.len(),
            2,
            "只有 sin + cos 可见（对照组被相机 tag 滤掉）：{colors:?}"
        );
        assert!(colors.contains(&COLOR_SIN), "curve_sin 命中本相机 ⇒ 可见");
        assert!(
            colors.contains(&COLOR_COS),
            "curve_cos 无 Visibility ⇒ 全局可见"
        );
        assert!(
            !colors.contains(&COLOR_CONTROL),
            "对照组只对不存在的相机可见 ⇒ 永不进队列"
        );
    }

    /// **N2**：隐藏 `GROUP_COS` ⇒ `curve_cos` 整条不进队列，`curve_sin` **不受影响**。
    #[test]
    fn hiding_a_group_removes_only_that_group() {
        let groups = {
            let mut g = GroupVisibility::new();
            g.hide(GROUP_COS);
            g
        };
        let colors = recorded_stroke_colors(|res| {
            res.insert(groups);
        });
        assert_eq!(
            colors,
            vec![COLOR_SIN],
            "隐藏 GROUP_COS ⇒ 只剩 curve_sin（实测 {colors:?}）"
        );
    }

    /// **N4**：`Timeline` 把 `curve_cos` 的 `Opacity` 从 1.0 推到 0.0（且中途真的在变）。
    #[test]
    fn timeline_fades_opacity_from_one_to_zero() {
        let mut scene = Scene::new();
        let d = build(&mut scene);
        // id 也按名字取（N1 的用法示范；不用 spawn 的返回值）。
        let cos_id = find_by_name(&scene, NAME_COS).expect("curve_cos");
        let cos_entity = entity_of(cos_id).expect("curve_cos 是活对象");
        let world = scene.into_world();

        let mut res = Resources::new();
        res.insert(TimeState::default());
        res.insert(d.forest);
        res.insert(d.bindings);

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::SceneSync, BindingApplySystem);
        sched.init_resources();

        let opacity_at = |sched: &mut Scheduler, t: f32| -> f32 {
            sched
                .resources_mut()
                .get_mut::<TimeState>()
                .unwrap()
                .elapsed = t;
            sched.run_frame(0.0);
            sched.world().get::<&Opacity>(cos_entity).unwrap().0
        };

        // t = 0 / 1（段中）/ 2（段末）。
        assert!(
            (opacity_at(&mut sched, 0.0) - 1.0).abs() < 1e-6,
            "起始应为 1.0"
        );
        let mid = opacity_at(&mut sched, 1.0);
        assert!((mid - 0.5).abs() < 1e-3, "t=1s（段中）应为 0.5，实测 {mid}");
        let end = opacity_at(&mut sched, FADE_SECONDS as f32);
        assert!(end <= 1e-6, "段末应为 0.0（全透明），实测 {end}");
    }

    // ─── 验收 2：GPU 像素级（真实设备门控） ────────────────────────────────

    /// 离屏回读宽度（紧凑 `w*4` 行距）；与 [`gpu_geometry`] 的视口一致。
    const GPU_W: u32 = 240;
    /// 离屏回读高度。
    const GPU_H: u32 = 200;

    /// 视口 `240×200`（原点落在屏幕 `(100,50)`），相机 `rect` 取同一矩形、scale 25。
    ///
    /// 世界可见范围 `x ∈ [-4.8, 4.8]`、`y ∈ [-4, 4]`：
    /// - `curve_sin`（`y ∈ [-1,1]`）完整落在视口内；
    /// - `curve_cos`（`y ∈ [-3.5,-1.5]`，含 `COS_OFFSET`）也在内，且与 sin **不接触**。
    fn gpu_geometry() -> xcal_render_backends::Frame2DGeometry {
        xcal_render_backends::Frame2DGeometry {
            viewport_points: Rect2D::new(
                xcal_scene::types::Vec2::new(100.0, 50.0),
                xcal_scene::types::Vec2::new(340.0, 250.0),
            ),
            pixels_per_point: 1.0,
            screen_size_px: [400, 300],
        }
    }

    fn gpu_camera() -> Camera2D {
        Camera2D::new(
            xcal_scene::types::Vec2::ZERO,
            25.0,
            Rect2D::new(
                xcal_scene::types::Vec2::new(100.0, 50.0),
                xcal_scene::types::Vec2::new(340.0, 250.0),
            ),
        )
    }

    /// 走**完整录制路径**渲染一帧并回读像素（`200×100` 紧凑像素；
    /// 目标格式字节序，默认 BGRA）。
    ///
    /// `tweak` 在"场景已建好、尚未录制"之间介入（用于把某个组件的值直接推到目标态，
    /// 例如把 `Opacity` 设成 0）—— 这样像素断言验的是**渲染通道**，不是组件写入。
    /// 它拿得到 `&mut hecs::World`：需要某个对象时同样用 `find_by_name` 查（而不是
    /// 让 `build` 把 id 漏出来）。
    fn render_demo_tweaked(
        groups: &GroupVisibility,
        tweak: impl FnOnce(&mut hecs::World),
    ) -> Vec<u8> {
        use xc_notion::Transform2D;
        use xcal_render_backends::{RenderContext, WgpuBackend2D};
        use xcal_scene::domain::LayerId;

        let ctx = RenderContext::headless_with_format(wgpu::TextureFormat::Bgra8UnormSrgb)
            .expect("headless GPU 上下文");
        let mut backend = WgpuBackend2D::new(ctx.device(), ctx.queue());

        let mut scene = Scene::new();
        let d = build(&mut scene);
        let mut world = scene.into_world();
        tweak(&mut world);

        let mut queue = DrawQueue::new();
        record_scene_tagged(
            &WorldView::new(&world),
            &gpu_camera(),
            Some(d.camera.0),
            groups,
            &mut queue,
        );

        let g = gpu_geometry();
        backend.begin_frame_at(g);
        for cmd in queue.commands() {
            backend.push(&cmd.shape, &Transform2D::identity(), &cmd.style, cmd.z.0);
        }
        let mut enc = ctx
            .device()
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("xcal-capability-demo-frame"),
            });
        backend.finish_frame(&mut enc, LayerId(0), &g);
        ctx.queue().submit([enc.finish()]);
        ctx.read_texture(
            backend.offscreen_texture(LayerId(0)).unwrap(),
            (GPU_W, GPU_H),
        )
    }

    /// 默认形态：不额外改动场景。
    fn render_demo(groups: &GroupVisibility) -> Vec<u8> {
        render_demo_tweaked(groups, |_| {})
    }

    /// 橙色像素数（`COLOR_COS` 的预乘残影也计入：`r > g > b` 且不透明度过阈值）。
    ///
    /// 判据在 `i32` 上算 —— `u8` 相加会溢出（实测踩过：`p[1] + 30` 在 debug 下 panic）。
    fn count_orange(px: &[u8]) -> usize {
        px.chunks_exact(4)
            .filter(|p| {
                let (r, g, b, a) = (p[0] as i32, p[1] as i32, p[2] as i32, p[3] as i32);
                a > 32 && r > g + 30 && g > b + 30
            })
            .count()
    }

    /// 青色像素数（`COLOR_SIN`）。与橙色的判据**互斥**（橙 `r>g>b` / 青 `g,b>r`）。
    fn count_cyan(px: &[u8]) -> usize {
        px.chunks_exact(4)
            .filter(|p| {
                let (r, g, b, a) = (p[0] as i32, p[1] as i32, p[2] as i32, p[3] as i32);
                a > 32 && g > r + 30 && b > r + 30
            })
            .count()
    }

    /// 品红像素数（`COLOR_CONTROL`）。
    fn count_magenta(px: &[u8]) -> usize {
        px.chunks_exact(4)
            .filter(|p| {
                let (r, g, b, a) = (p[0] as i32, p[1] as i32, p[2] as i32, p[3] as i32);
                a > 32 && r > g + 30 && b > g + 30
            })
            .count()
    }

    /// 🔴 **验收 2**：隐藏 `GROUP_COS` ⇒ **该曲线的像素真的消失**（不是只断言结构）。
    ///
    /// 三个数一起看才可证伪：
    /// - 橙色（cos）从 > 0 掉到 **0** ⇒ 组控制真的作用到了光栅化结果；
    /// - 青色（sin）**逐字节不变** ⇒ 组控制没有误伤别的对象；
    /// - 品红（对照组）**恒为 0** ⇒ 相机 tag 的反向判据在像素层也成立。
    #[test]
    fn hidden_group_removes_that_curves_pixels() {
        let visible = render_demo(&GroupVisibility::new());
        let hidden = {
            let mut g = GroupVisibility::new();
            g.hide(GROUP_COS);
            render_demo(&g)
        };

        let (sin_a, cos_a, ctl_a) = (
            count_cyan(&visible),
            count_orange(&visible),
            count_magenta(&visible),
        );
        let (sin_b, cos_b, ctl_b) = (
            count_cyan(&hidden),
            count_orange(&hidden),
            count_magenta(&hidden),
        );
        eprintln!("[capability-demo] 组全开：sin(青)={sin_a} cos(橙)={cos_a} 对照组(品红)={ctl_a}",);
        eprintln!(
            "[capability-demo] 隐藏 GROUP_COS：sin(青)={sin_b} cos(橙)={cos_b} 对照组(品红)={ctl_b}",
        );

        assert!(cos_a > 0, "组全开时 cos 必须有像素（实测 {cos_a}）");
        assert_eq!(
            cos_b, 0,
            "隐藏 GROUP_COS 后 cos 像素必须归零（组全开时 {cos_a}，现在 {cos_b}）"
        );
        assert_eq!(
            sin_b, sin_a,
            "sin 像素不受组开关影响（组全开 {sin_a}，隐藏后 {sin_b}）"
        );
        assert_eq!(ctl_a, 0, "对照组永不出现（相机 tag 不命中）");
        assert_eq!(ctl_b, 0, "对照组在隐藏组后同样为 0");
    }

    /// **N4 的像素层判据**：`Opacity = 0` 的曲线在像素层**完全消失**。
    ///
    /// 与上一条的分工：那条证明"**组**过滤落到光栅化结果"，本条证明"**不透明度**过滤
    /// 落到光栅化结果" —— 两条通道都真的影响画面，而不是只改了组件值。
    #[test]
    fn zero_opacity_curve_has_no_pixels() {
        let opaque = render_demo(&GroupVisibility::new());
        let transparent = render_demo_tweaked(&GroupVisibility::new(), |world| {
            // 等价于 Timeline 走到段末（`Opacity` → 0.0）。
            let cos_id = find_by_name(&WorldView::new(world), NAME_COS).expect("curve_cos");
            world
                .get::<&mut Opacity>(entity_of(cos_id).unwrap())
                .unwrap()
                .0 = 0.0;
        });

        let (sin_a, cos_a) = (count_cyan(&opaque), count_orange(&opaque));
        let (sin_b, cos_b) = (count_cyan(&transparent), count_orange(&transparent));
        eprintln!("[capability-demo] opacity=1：sin(青)={sin_a} cos(橙)={cos_a}");
        eprintln!("[capability-demo] opacity=0：sin(青)={sin_b} cos(橙)={cos_b}");

        assert!(cos_a > 0, "opacity = 1 时 cos 必须有像素（实测 {cos_a}）");
        assert_eq!(
            cos_b, 0,
            "opacity = 0 ⇒ cos 像素必须归零（opacity = 1 时 {cos_a}，现在 {cos_b}）"
        );
        assert_eq!(sin_b, sin_a, "sin 的像素不受 cos 不透明度影响");
    }
}
