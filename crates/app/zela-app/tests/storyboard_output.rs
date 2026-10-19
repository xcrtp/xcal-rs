// storyboard 的**可渲染性**回归（A2：用例留在 app 侧，场景作者在 zela-scene）。
//
// 本环境无法目视 GUI 时的替代验证：把分镜场景接进**真实 System 链路**跑几帧，
// 断言 2D 队列（网格）与**书写动画队列**在 t=1s / 4s 确有产出，
// 且"写到哪"（`end`）随时间正确推进、相机被推进到峰值。

use std::f32::consts::FRAC_PI_2;

use zela_render::{DrawQueue, RevealQueue, RevealRecordSystem, SceneRecordSystem, ViewRect};
use zela_scene::TimeState;
use zela_scene::animation::BindingApplySystem;
use zela_scene::authored::storyboard::build;
use zela_scene::contract::entity_of;
use zela_scene::scene::Scene;
use zela_schedule::resource::Resources;
use zela_schedule::scheduler::{Phase, Scheduler};

const CAMERA_SCALE_PUSHED: f32 = 110.0;
const CAMERA_PEAK: ze_notion::Vec2 = ze_notion::Vec2::new(FRAC_PI_2, 1.0);

#[test]
fn storyboard_produces_draw_output_over_time() {
    use zela_scene::AxesRefreshSystem;
    use zela_scene::domain::UiConfig;

    let mut scene = Scene::new();
    let engine = ze_prerender::placeholder_engine();
    let sb = build(&mut scene, &engine);
    let camera_id = sb.camera.0;
    let camera_entity = entity_of(camera_id).unwrap();
    let world = scene.into_world();

    let mut res = Resources::new();
    res.insert(sb.camera);
    res.insert(ViewRect::default());
    res.insert(UiConfig::default());
    res.insert(TimeState::default());
    res.insert(sb.forest);
    res.insert(sb.bindings);
    res.insert(DrawQueue::new());
    res.insert(RevealQueue::default());

    let mut sched = Scheduler::new(world, res);
    sched.add_system(Phase::SceneSync, BindingApplySystem);
    sched.add_system(Phase::Prepare, AxesRefreshSystem);
    sched.add_system(Phase::RenderCommit, SceneRecordSystem);
    sched.add_system(Phase::RenderCommit, RevealRecordSystem);
    sched.init_resources();

    // ── t = 1s：网格已建；曲线写到一半（其余公式尚未开始） ──
    sched
        .resources_mut()
        .get_mut::<TimeState>()
        .unwrap()
        .elapsed = 1.0;
    sched.run_frame(0.0);

    let grid = sched.resources().get::<DrawQueue>().unwrap().len();
    assert!(
        grid > 0,
        "网格/坐标轴线段应已录进 2D 队列（t=1s），实际 {grid}"
    );

    let reveals = sched.resources().get::<RevealQueue>().unwrap().0.clone();
    assert!(
        reveals.len() >= 2,
        "曲线 + 公式应有书写项，实际 {}",
        reveals.len()
    );
    let max_end = reveals.iter().map(|r| r.end).fold(f32::MIN, f32::max);
    assert!(
        (max_end - 0.5).abs() < 1e-3,
        "t=1s 时曲线应写到 0.5（其余仍为 0），实际 max end = {max_end}"
    );
    // 区间起点恒为 0（`Scalar` 只推 `end`）。
    assert!(reveals.iter().all(|r| r.start == 0.0), "start 应恒为 0");

    // ── t = 4s：曲线写满（end=1）；公式**写到一半（逐轮廓在途）**；相机到峰值 ──
    sched
        .resources_mut()
        .get_mut::<TimeState>()
        .unwrap()
        .elapsed = 4.0;
    sched.run_frame(0.0);

    let reveals = sched.resources().get::<RevealQueue>().unwrap().0.clone();
    let max_end = reveals.iter().map(|r| r.end).fold(f32::MIN, f32::max);
    assert!(
        (max_end - 1.0).abs() < 1e-3,
        "曲线应写满（end=1），实际 {max_end}"
    );
    // **逐轮廓依次书写**的判据：窗口过半时，前面的轮廓已写完、后面的**还没开始**。
    let pending = reveals.iter().filter(|r| r.end == 0.0).count();
    let done = reveals
        .iter()
        .filter(|r| (r.end - 1.0).abs() < 1e-6)
        .count();
    assert!(
        pending >= 1,
        "t=4s（公式窗口过半）应仍有未开始的轮廓（依次书写），实际 pending={pending}、\
         各轮廓 end = {:?}",
        reveals.iter().map(|r| r.end).collect::<Vec<_>>()
    );
    assert!(
        done >= 1,
        "t=4s 应已有写完的轮廓（曲线 + 公式前几笔），实际 done={done}"
    );

    let view = sched
        .world()
        .get::<&ze_object::Framing2D>(camera_entity)
        .unwrap();
    assert!(
        (view.scale - CAMERA_SCALE_PUSHED).abs() < 1e-3,
        "相机应推进到 scale={CAMERA_SCALE_PUSHED}，实际 {}",
        view.scale
    );
    assert!(
        (view.center - CAMERA_PEAK).length() < 1e-3,
        "相机中心应到峰值"
    );
}
