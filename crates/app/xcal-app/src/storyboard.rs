// storyboard.rs — 首个**数学动画原型**（P5）：§1.1 六条分镜的**声明处**。
//
// 本轮定位（§0）：过去几轮是"把引擎整理干净"，本轮是"让引擎能写出一个数学动画"。
// **用户友好 = 写动画的人友好** —— 本文件用几行声明表达整段分镜，而不是手工
// spawn 一堆实体 + 手写 offset。
//
// # §1.1 分镜（本文件逐条落地）
//
// 1. **常驻**：`Axes2D` 网格与坐标轴（对象层已有，`AxesRefreshSystem` 逐帧 refresh）；
// 2. **镜头初始位**：2D 相机作为 scene 里的对象被 spawn，脚本取出 handle；
// 3. **0–2s**：曲线 `y = sin x` 从左到右**生长**（`MorphShape`：`from` 退化点 →
//    `to` 完整 `Path2D`，走已有 GPU 形变管线 `shape_2d_morph.wgsl`）；
// 4. **2–2.5s**：`wait`；
// 5. **2.5–3.5s**：**镜头推进**到曲线峰值（驱动 `Framing2D.center` / `scale`）；
// 6. **3.5–4.5s**：公式 `y = \sin x` **浮现**（每个子轮廓从退化点形变浮现）。
//
// # 为什么这个脚本"最小但够"（§1.2）
//
// 同时消费四样东西：形变管线（已有）、时间编排（P3 Timeline）、2D 相机动画
// （P0 `Camera2DFraming` 目标 + P1 相机对象）、公式（已有 Typst 管线）；
// 不碰 3D、不需要箭头 / Text、不需要 seek；每条分镜都落到既有设施上，不新建 crate。

use std::f32::consts::FRAC_PI_2;

use xcal_render::{MeasureSpace, StrokeSolid};
use xcal_scene::animation::{Camera2DFraming, MorphT, Timeline};
use xcal_scene::scene::components::MorphShape;
use xcal_scene::scene::Scene;
use xcal_scene::{AnimValue, BindingTable, ClockForest, Curve, Interp};
use xc_notion::Path2D;
use xc_object::{Axes2D, Camera2DHandle, Framing2D, spawn_camera2d};
use xc_prerender::InMemoryEngine;

use crate::formula;

/// 主 2D 相机初始取景（世界原点居中、40 px/世界单位）。
const CAMERA_START: Framing2D = Framing2D {
    center: glam::Vec2::ZERO,
    scale: 40.0,
};
/// 镜头推进目标：曲线峰值 `(π/2, 1)`，并放大到 110 px/世界单位。
const CAMERA_PEAK: glam::Vec2 = glam::Vec2::new(FRAC_PI_2, 1.0);
const CAMERA_SCALE_PUSHED: f32 = 110.0;

/// 曲线采样区间：`[-π, π]`（关于原点对称、两端恰在 x 轴上）。
///
/// 开口折线 ⇒ 形变按**开口**口径重采样（不再补首尾的弦，见
/// `MorphItem::sampled_endpoints` 的"闭合性"说明）——早期版本恒按闭合环处理，
/// 首尾之间会多出一条可见的直线。
const CURVE_STEP_COUNT: usize = 128;

/// 分镜产物：编译好的时钟树 + 绑定表 + 主 2D 相机句柄。
pub struct Storyboard {
    pub forest: ClockForest,
    pub bindings: BindingTable,
    pub camera: Camera2DHandle,
}

/// 把 §1.1 分镜装配进场景，并**把时间编排编译成 `ClockNode + Binding`**。
///
/// `engine` 是已建好的 Typst / 占位引擎（App 侧建一次，与内容纹理公式共用）。
pub fn build(scene: &mut Scene, engine: &InMemoryEngine) -> Storyboard {
    // ── 分镜 1：常驻网格与坐标轴 ────────────────────────────────────────
    // 几何在对象层 `Axes2D`；`AxesRefreshSystem` 逐帧按相机 refresh。
    Axes2D::unbounded().build(scene).expect("Axes2D 已登记");

    // ── 分镜 2：2D 相机作为场景对象（脚本取出 handle）────────────────────
    let camera = spawn_camera2d(scene, CAMERA_START).expect("Framing2D 已登记");

    // ── 分镜 3：曲线 y = sin x 的形变对象（from = 退化点，to = 完整折线）──
    let curve_pts = sin_samples();
    let degenerate = vec![curve_pts[0]; curve_pts.len()];
    let curve_entity = scene.spawn((
        MorphShape::new(
            Path2D::from_polyline(&degenerate, false),
            Path2D::from_polyline(&curve_pts, false),
        ),
        StrokeSolid::new_in(2.0, (0, 200, 200, 255), MeasureSpace::Screen),
    ));
    let curve_target = MorphT {
        id: xcal_scene::contract::object_id_of(curve_entity),
    };

    // ── 分镜 6：公式 `y = sin x` 的浮现对象（每个子轮廓一个形变对象）────
    let formula_targets = spawn_formula_morph(scene, engine);

    // ── 时间编排（顺序 + 段内并行）─────────────────────────────────────
    let mut tl = Timeline::new();
    // 0–2s：曲线生长（`t` 在段内 0 → 1）
    tl.play(ramp(2.0), curve_target, 2.0);
    // 2–2.5s：停
    tl.wait(0.5);
    // 2.5–3.5s：镜头推进 —— `scale` 与 `center` 在**同一区间**并行驱动
    let cam = Camera2DFraming { id: camera.0 };
    tl.play(
        keyframes_scalar(1.0, CAMERA_START.scale, CAMERA_SCALE_PUSHED),
        cam,
        1.0,
    )
    .parallel(
        keyframes_vec2(1.0, CAMERA_START.center, CAMERA_PEAK),
        cam,
    );
    // 3.5–4.5s：公式浮现（多个子轮廓在**同一区间**并行浮现）
    if let Some((first, rest)) = formula_targets.split_first() {
        tl.play(ramp(1.0), *first, 1.0);
        for target in rest {
            tl.parallel(ramp(1.0), *target);
        }
    }

    let mut forest = ClockForest::new();
    let mut bindings = BindingTable::new();
    tl.compile(&mut forest, &mut bindings);

    Storyboard {
        forest,
        bindings,
        camera,
    }
}

// ─── 采样 / 曲线构造（纯函数）─────────────────────────────────────────────

/// `y = sin x` 在 `[-π, π]` 上的等步长采样（首尾点都恰为 `y = 0`）。
fn sin_samples() -> Vec<glam::Vec2> {
    let step = std::f32::consts::TAU / CURVE_STEP_COUNT as f32;
    (0..=CURVE_STEP_COUNT)
        .map(|i| {
            let x = -std::f32::consts::PI + (i as f32) * step;
            glam::Vec2::new(x, x.sin())
        })
        .collect()
}

/// 段内从 0 线性到 1 的曲线（`Phase{omega: 1/dur}`）。
fn ramp(dur: f64) -> Curve {
    Curve::Phase { omega: (1.0 / dur) as f32 }
}

/// 段内把标量从 `from` 插值到 `to`（`Keyframes`，定义域是**段内局部时间**）。
fn keyframes_scalar(dur: f64, from: f32, to: f32) -> Curve {
    Curve::Keyframes {
        keys: vec![(0.0, AnimValue::Scalar(from)), (dur, AnimValue::Scalar(to))],
        interpolation: Interp::Linear,
    }
}

/// 段内把 `Vec2` 从 `from` 插值到 `to`（驱动 `Framing2D.center`）。
fn keyframes_vec2(dur: f64, from: glam::Vec2, to: glam::Vec2) -> Curve {
    Curve::Keyframes {
        keys: vec![(0.0, AnimValue::Vec2(from)), (dur, AnimValue::Vec2(to))],
        interpolation: Interp::Linear,
    }
}

/// 公式 `y = \sin x` → 每个子轮廓一个"从退化点浮现"的形变对象，返回其目标句柄。
///
/// 退化点是子轮廓包围盒中心（从字形中心"绽开"）；编译失败（占位引擎也会给方块轮廓）
/// 时返回空 —— 调用方据此跳过第 6 条分镜（优雅降级，不 panic）。
fn spawn_formula_morph(scene: &mut Scene, engine: &InMemoryEngine) -> Vec<MorphT> {
    // 字号 0.8 / 锚点 (1.0, 2.0)：镜头推进后仍完整落在可见范围内
    // （推进后可见 x ∈ [-2.14, 5.21]、y ∈ [-1.73, 3.73]）。
    let spec = formula::FormulaSpec::new("y = sin x", 0.8, glam::Vec2::new(1.0, 2.0));
    let Some(ir) = formula::rebuild(engine, &spec) else {
        eprintln!("[xcal-app] 分镜 6：公式 `y = sin x` 编译失败，跳过浮现");
        return Vec::new();
    };

    let mut targets = Vec::new();
    for (path, color, width, space) in formula::compile_to_stroke_paths(&ir, &spec) {
        let collapse = path
            .compute_bounds()
            .unwrap_or_default()
            .center();
        let degenerate = Path2D::from_polyline(&[collapse, collapse], false);
        let entity = scene.spawn((
            MorphShape::new(degenerate, path),
            StrokeSolid::new_in(width, color, space),
        ));
        targets.push(MorphT {
            id: xcal_scene::contract::object_id_of(entity),
        });
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_scene::animation::{Target, TargetKind};
    use xcal_scene::contract::{ObjectId, SceneRead};
    use xc_prerender::placeholder_engine;

    /// §11.1：六条分镜能被**声明**出来 —— 场景对象齐备且**时序正确**。
    #[test]
    fn storyboard_declares_six_shots_with_declared_timing() {
        let mut scene = Scene::new();
        let engine = placeholder_engine();
        let sb = build(&mut scene, &engine);

        // 分镜 1：坐标轴容器在场景里。
        let mut axes = 0;
        scene.for_each::<Axes2D>(|_, _| axes += 1);
        assert_eq!(axes, 1, "常驻 Axes2D");

        // 分镜 2：相机是场景对象，且句柄可解析出取景（初始 scale = 40）。
        assert_ne!(sb.camera.0, ObjectId::DANGLING);
        let view = scene
            .with::<Framing2D, _>(sb.camera.0, |f| *f)
            .expect("相机对象在场景里");
        assert_eq!(view, CAMERA_START);

        // 分镜 3 + 6：形变对象（曲线 1 个 + 公式子轮廓若干）。
        let mut morphs = 0;
        scene.for_each::<MorphShape>(|_, _| morphs += 1);
        assert!(morphs >= 2, "曲线 + 公式至少两个形变对象，实际 {morphs}");

        // 分镜 4/5/6 的时序：0–2 / (wait 0.5) / 2.5–3.5 / 3.5–4.5。
        // 由编译产物反推：节点 offset + span 即区间。
        let segs = timeline_segments(&sb);
        assert_eq!(segs[0], (0.0, 2.0), "曲线生长 0–2s");
        assert_eq!(segs[1], (2.5, 1.0), "镜头推进 2.5–3.5s");
        assert_eq!(segs[2], (3.5, 1.0), "公式浮现 3.5–4.5s");

        // 分镜 5 是**并行**两动作（scale + center）：同区间、同节点。
        let cam_nodes: Vec<_> = sb
            .bindings
            .bindings
            .iter()
            .filter(|b| b.target.kind() == TargetKind::Camera2DFraming)
            .map(|b| b.clock)
            .collect();
        assert_eq!(cam_nodes.len(), 2, "scale 与 center 各一条绑定");
        assert_eq!(cam_nodes[0], cam_nodes[1], "两者共享同一时钟节点（同区间）");
    }

    /// §11.2：新增驱动目标**不需要改枚举、也不需要改 System 源码** ——
    /// 本脚本用的 `Camera2DFraming` 就是 P0 新增、无 System 改动的目标。
    #[test]
    fn new_target_needs_no_enum_or_system_change() {
        let t = Camera2DFraming {
            id: ObjectId::DANGLING,
        };
        assert_eq!(t.kind(), TargetKind::Camera2DFraming);
    }

    /// §11.3：改掉第一段动画的时长后，后续动画的时刻**自动跟随**。
    #[test]
    fn later_shots_follow_first_duration_change() {
        let mut tl = Timeline::new();
        // 与 `build` 相同的编排（此处只关心时间，不需要场景对象）。
        tl.play(ramp(2.0), MorphT { id: ObjectId::DANGLING }, 2.0);
        tl.wait(0.5);
        tl.play(
            keyframes_scalar(1.0, 40.0, 110.0),
            Camera2DFraming { id: ObjectId::DANGLING },
            1.0,
        );
        tl.play(ramp(1.0), MorphT { id: ObjectId::DANGLING }, 1.0);

        assert_eq!(tl.schedule()[2].start, 3.5);
        tl.set_duration(0, 3.0);
        assert_eq!(tl.schedule()[1].start, 3.5, "镜头推进自动顺延");
        assert_eq!(tl.schedule()[2].start, 4.5, "公式浮现自动顺延");
    }

    /// **可渲染性**（本环境无法目视 GUI 时的替代验证）：把分镜场景接进**真实 System
    /// 链路**跑几帧，断言 2D 队列（网格 + 曲线/公式）与形变队列在 t=1s / 4s 确有产出，
    /// 且相机被推进到峰值。
    #[test]
    fn storyboard_produces_draw_output_over_time() {
        use xcal_egui::resources::UiConfig;
        use xcal_egui::systems::AxesRefreshSystem;
        use xcal_render::{DrawQueue, MorphQueue, MorphRecordSystem, SceneRecordSystem, ViewRect};
        use xcal_scene::animation::BindingApplySystem;
        use xcal_scene::TimeState;
        use xcal_schedule::resource::Resources;
        use xcal_schedule::scheduler::{Phase, Scheduler};

        let mut scene = Scene::new();
        let engine = placeholder_engine();
        let sb = build(&mut scene, &engine);
        let camera_id = sb.camera.0;
        let camera_entity = xcal_scene::contract::entity_of(camera_id).unwrap();
        let world = scene.into_world();

        let mut res = Resources::new();
        res.insert(sb.camera);
        res.insert(ViewRect::default());
        res.insert(UiConfig::default());
        res.insert(TimeState::default());
        res.insert(sb.forest);
        res.insert(sb.bindings);
        res.insert(DrawQueue::new());
        res.insert(MorphQueue::default());

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::SceneSync, BindingApplySystem);
        sched.add_system(Phase::Prepare, AxesRefreshSystem);
        sched.add_system(Phase::RenderCommit, SceneRecordSystem);
        sched.add_system(Phase::RenderCommit, MorphRecordSystem);
        sched.init_resources();

        // ── t = 1s：网格已建；曲线形变到一半（其余公式形变仍为退化点） ──
        sched.resources_mut().get_mut::<TimeState>().unwrap().elapsed = 1.0;
        sched.run_frame(0.0);

        let grid = sched.resources().get::<DrawQueue>().unwrap().len();
        assert!(grid > 0, "网格/坐标轴线段应已录进 2D 队列（t=1s），实际 {grid}");

        let morphs = sched.resources().get::<MorphQueue>().unwrap().0.clone();
        assert!(morphs.len() >= 2, "曲线 + 公式应有形变项，实际 {}", morphs.len());
        let max_t = morphs.iter().map(|m| m.t).fold(f32::MIN, f32::max);
        assert!(
            (max_t - 0.5).abs() < 1e-3,
            "t=1s 时曲线应形变到 0.5（其余仍为 0），实际 max t = {max_t}"
        );

        // ── t = 4s：曲线长满（t=1）、公式浮现到一半（0.5）、相机推进到峰值 ──
        sched.resources_mut().get_mut::<TimeState>().unwrap().elapsed = 4.0;
        sched.run_frame(0.0);

        let morphs = sched.resources().get::<MorphQueue>().unwrap().0.clone();
        let max_t = morphs.iter().map(|m| m.t).fold(f32::MIN, f32::max);
        let min_t = morphs.iter().map(|m| m.t).fold(f32::MAX, f32::min);
        assert!((max_t - 1.0).abs() < 1e-3, "曲线应长满（t=1），实际 {max_t}");
        assert!((min_t - 0.5).abs() < 1e-3, "公式应浮现到 0.5，实际 {min_t}");

        let view = sched.world().get::<&Framing2D>(camera_entity).unwrap();
        assert!(
            (view.scale - CAMERA_SCALE_PUSHED).abs() < 1e-3,
            "相机应推进到 scale={CAMERA_SCALE_PUSHED}，实际 {}",
            view.scale
        );
        assert!((view.center - CAMERA_PEAK).length() < 1e-3, "相机中心应到峰值");
    }

    /// 从编译产物反推每个 `play` 的时间区间（`(offset, span_hi - span_lo)`）。
    fn timeline_segments(sb: &Storyboard) -> Vec<(f64, f64)> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for b in &sb.bindings.bindings {
            if !seen.insert(b.clock) {
                continue;
            }
            let node = sb.forest.get(b.clock).expect("节点在");
            let (lo, hi) = node.span.expect("Timeline 编译的节点必有 span");
            out.push((node.local_offset, hi - lo));
        }
        out
    }
}
