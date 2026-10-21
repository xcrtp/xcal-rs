// capability_demo 的验收用例（A2：场景作者搬到 zela-scene 后，用例留在 app 侧）。
//
// 验收 1：headless 结构断言（N1 / N3 / N2 / N4）；
// 验收 2：GPU 像素级（组控制与不透明度真的作用到光栅化结果）。

use zela_render::{DrawQueue, SceneRecordSystem, ViewRect, record_scene_tagged};
use zela_render_backends::{RenderContext, WgpuBackend2D};
use zela_scene_ecs::TimeState;
use zela_scene_ecs::animation::BindingApplySystem;
use zela_authored::capability_demo::{
    COLOR_CONTROL, COLOR_COS, COLOR_SIN, COS_OFFSET, CapabilityDemo, FADE_SECONDS, GROUP_COS,
    NAME_CONTROL, NAME_COS, NAME_SIN, build,
};
use zela_scene_ecs::contract::{SceneRead, WorldView, entity_of, find_by_name};
use zela_scene_ecs::scene::Scene;
use zela_scene_ecs::types::Rect2D;
use zela_schedule::resource::Resources;
use zela_schedule::scheduler::{Phase, Scheduler};

use ze_scene_core::{Camera2D, Path};

/// 录制测试用视口（`800×600`，与 `ViewRect::default` 无关，显式给出以免依赖缺省值）。
fn view_rect() -> Rect2D {
    Rect2D::new(
        zela_scene_ecs::types::Vec2::ZERO,
        zela_scene_ecs::types::Vec2::new(800.0, 600.0),
    )
}

/// 把 demo 场景灌进真实 System 链路（`SceneRecordSystem`）跑一帧，返回队列里的描边色。
///
/// 走 System 而不是直接调 `record_scene_tagged`：这样验的是**接线**（`Reads` 里的
/// `GroupVisibility` 真的被读到了、`camera_tag` 真的从 `Camera2DHandle` 取了）。
fn recorded_stroke_colors(extra: impl FnOnce(&mut Resources)) -> Vec<(u8, u8, u8, u8)> {
    let mut scene = Scene::new();
    let d: CapabilityDemo = build(&mut scene);
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
    let name_of = |id| scene.with::<zela_scene_ecs::contract::Name, _>(id, |n| n.as_str().to_string());
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
        let mut g = zela_scene_ecs::contract::GroupVisibility::new();
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
    use zela_render::Opacity;

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
fn gpu_geometry() -> zela_render_backends::Frame2DGeometry {
    zela_render_backends::Frame2DGeometry {
        viewport_points: Rect2D::new(
            zela_scene_ecs::types::Vec2::new(100.0, 50.0),
            zela_scene_ecs::types::Vec2::new(340.0, 250.0),
        ),
        pixels_per_point: 1.0,
        screen_size_px: [400, 300],
    }
}

fn gpu_camera() -> Camera2D {
    Camera2D::new(
        zela_scene_ecs::types::Vec2::ZERO,
        25.0,
        Rect2D::new(
            zela_scene_ecs::types::Vec2::new(100.0, 50.0),
            zela_scene_ecs::types::Vec2::new(340.0, 250.0),
        ),
    )
}

/// 走**完整录制路径**渲染一帧并回读像素（`240×200` 紧凑像素；
/// 目标格式字节序，默认 BGRA）。
fn render_demo_tweaked(
    groups: &zela_scene_ecs::contract::GroupVisibility,
    tweak: impl FnOnce(&mut hecs::World),
) -> Vec<u8> {
    use ze_notion::Transform2D;
    use zela_scene_ecs::domain::LayerId;

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
            label: Some("zela-capability-demo-frame"),
        });
    backend.finish_frame(&mut enc, LayerId(0), &g);
    ctx.queue().submit([enc.finish()]);
    ctx.read_texture(
        backend.offscreen_texture(LayerId(0)).unwrap(),
        (GPU_W, GPU_H),
    )
}

/// 默认形态：不额外改动场景。
fn render_demo(groups: &zela_scene_ecs::contract::GroupVisibility) -> Vec<u8> {
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
    use zela_scene_ecs::contract::GroupVisibility;

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
    eprintln!("[capability-demo] 组全开：sin(青)={sin_a} cos(橙)={cos_a} 对照组(品红)={ctl_a}");
    eprintln!(
        "[capability-demo] 隐藏 GROUP_COS：sin(青)={sin_b} cos(橙)={cos_b} 对照组(品红)={ctl_b}"
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
#[test]
fn zero_opacity_curve_has_no_pixels() {
    use zela_render::Opacity;
    use zela_scene_ecs::contract::GroupVisibility;

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

// 常量用途锚点（避免误删）：COS_OFFSET / COLOR_CONTROL 在几何与像素判据中被引用。
const _: f32 = COS_OFFSET;
const _: (u8, u8, u8, u8) = COLOR_CONTROL;
