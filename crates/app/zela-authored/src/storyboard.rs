// storyboard.rs — 首个**数学动画原型**（P5）：§1.1 六条分镜的**声明处**。
//
// # 定位（A2/A3 后）
//
// 本文件是 `storyboard.lua` / `storyboard.py` 的**等价性参照基准**，不是死代码。
// 生产路径（默认启动 / `--script` / `--python`）已统一走脚本版；本文件只被
// 等价性用例引用。**不得因"默认路径已切脚本版"而删除。**
//
// # §1.1 分镜（本文件逐条落地）
//
// 1. **常驻**：`Axes2D` 网格与坐标轴（对象层已有，`AxesRefreshSystem` 逐帧 refresh）；
// 2. **镜头初始位**：2D 相机作为 scene 里的对象被 spawn，脚本取出 handle；
// 3. **0–2s**：曲线 `y = sin x` 从左到右**被"写"出来**（R0 书写动画：`Reveal` 组件 +
//    `RevealRange` 目标驱动区间 `[0, end]`，逐帧只有 uniform 变 —— 几何静态）；
// 4. **2–2.5s**：`wait`；
// 5. **2.5–3.5s**：**镜头推进**到曲线峰值（驱动 `Framing2D.center` / `scale`）；
// 6. **3.5–4.5s**：公式 `y = \sin x` **被"写"出来** —— 每个描边子轮廓一个 `Reveal`，
//    并**逐轮廓依次**占一段（第 i 个轮廓独占 `[3.5 + i·1/N, 3.5 + (i+1)·1/N]`）。
//    顺序来自 `Timeline` 的**顺序游标**：连续 `play` 即首尾相接；用 `parallel` 会让
//    全部轮廓**同时**被写出（实测踩过 —— 那看起来像"整体浮现"，不是写字）。

use std::f32::consts::FRAC_PI_2;

use ze_notion::{MeasureSpace, Path2D};
use ze_scene_core::{Arrow, Axes2D, Camera2DHandle, Framing2D, Path, Transform};
use ze_prerender::InMemoryEngine;
use ze_render_ir::{FillSolid, StrokeSolid};

use zela_scene_ecs::animation::{Camera2DFraming, RevealRange};
use zela_scene_ecs::content;
use zela_scene_ecs::scene::Scene;
use zela_scene_ecs::scene::components::Reveal;
use zela_scene_ecs::scene_script::{Handle, SceneEncoder};
use zela_scene_ecs::{AnimValue, BindingTable, ClockForest, Curve, Interp};

/// 主 2D 相机初始取景（世界原点居中、40 px/世界单位）。
const CAMERA_START: Framing2D = Framing2D {
    center: ze_notion::Vec2::ZERO,
    scale: 40.0,
};
/// 镜头推进目标：曲线峰值 `(π/2, 1)`，并放大到 110 px/世界单位。
const CAMERA_PEAK: ze_notion::Vec2 = ze_notion::Vec2::new(FRAC_PI_2, 1.0);
const CAMERA_SCALE_PUSHED: f32 = 110.0;

/// 分镜 6（公式书写）的总时长：各轮廓**平摊**这一段（见 `build` 的编排）。
///
/// 固定"总窗口"而不是"每轮廓固定时长"：保持六条分镜声明的时序（3.5–4.5s）不变，
/// 轮廓多则每笔更快（字形越多写得越急），但整段不会溢出到 4.5s 之后。
const FORMULA_WRITE_SECONDS: f64 = 1.0;

/// **T3 标注文字**：纯文本标注的摆放位置（世界坐标，经 `Transform` 施加）。
///
/// 选点使它在**镜头推进前后都在可见范围内**（推进后可见 x ∈ [-2.07, 5.21]、
/// y ∈ [-1.73, 3.73]）；文字几何在原点编译，位置由 `Transform` 负责 ⇒ T1 + T3 联用。
const TEXT_POS: ze_notion::Vec3 = ze_notion::Vec3::new(-1.5, 3.0, 0.0);
/// 标注文字的目标字号（世界单位高度）。
const TEXT_FONT_SIZE: f32 = 0.55;

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
///
/// # B1/B2：本函数改用**场景脚本**（`SceneEncoder`）——装配代码的简化
///
/// 改动前是"手工 `scene.spawn` + 接住 `Entity` + `object_id_of` + 手写 `Timeline`
/// + 手工 `compile`"；现在只声明**句柄 → 曲线 → 时长**，`commit` 时一次性创建对象、
/// 解析句柄、编译时序。收益：
/// - 不再出现 `object_id_of` / 手写 `ClockNode` / 手写 `Binding`；
/// - 相机 / 曲线 / 公式轮廓的**目标绑定**全部经句柄（声明期无需 `ObjectId`）。
pub fn build(scene: &mut Scene, engine: &InMemoryEngine) -> Storyboard {
    // ── 分镜 1：常驻网格与坐标轴 ────────────────────────────────────────
    // 几何在对象层 `Axes2D`；`AxesRefreshSystem` 逐帧按相机 refresh。
    Axes2D::unbounded().build(scene).expect("Axes2D 已登记");

    // ── 场景脚本：声明期只有句柄，`commit` 时才创建对象 / 编译时序 ─────────
    let mut enc = SceneEncoder::new();

    // 分镜 2：2D 相机作为场景对象（脚本句柄）。
    let camera_h = enc.spawn(vec![Box::new(CAMERA_START)]);

    // 分镜 3：曲线 y = sin x 的**书写动画**对象（R0）──────────────────────
    // `Reveal`：几何 = 完整折线（帧间不变），可见性由归一化弧长区间 `[start, end]` 决定
    // ⇒ 0–2s 把 `end` 从 0 推到 1，即为"笔尖从左写到右"。
    let curve_h = enc.spawn(vec![
        Box::new(Reveal::new(Path2D::from_polyline(&sin_samples(), false))),
        Box::new(StrokeSolid::new_in(
            2.0,
            (0, 200, 200, 255),
            MeasureSpace::Screen,
        )),
    ]);

    // 分镜 6：公式 `y = sin x` 的书写动画对象（每个描边子轮廓一个）。
    let formula_h = spawn_formula_reveal(&mut enc, engine);

    // ── T3 标注文字：纯文本 → 描边路径，经 `Transform` 摆放（T1 + T3 联用）──
    // 每个描边子轮廓一个对象（与公式同口径）；静态，不参与分镜时序。
    spawn_text_label(&mut enc, engine);

    // ── T2 箭头：实心箭头指向曲线峰值 (π/2, 1)（一等图元 + Group 降解）────────
    // 静态标注，不参与分镜时序；`FillSolid` ⇒ 3B1B 式的实心箭头。
    enc.spawn(vec![
        Box::new(Arrow::new(
            (0.2_f32, 2.1_f32),
            (CAMERA_PEAK.x - 0.15, CAMERA_PEAK.y + 0.15),
            0.25,
            0.25,
        )),
        Box::new(FillSolid::new((255, 220, 120, 255))),
    ]);

    // ── 时间编排（顺序 + 段内并行）─────────────────────────────────────
    // 0–2s：曲线书写（`end` 在段内 0 → 1）
    enc.animate::<RevealRange>(curve_h, ramp(2.0), 2.0);
    // 2–2.5s：停
    enc.wait(0.5);
    // 2.5–3.5s：镜头推进 —— `scale` 与 `center` 在**同一区间**并行驱动
    enc.animate::<Camera2DFraming>(
        camera_h,
        keyframes_scalar(1.0, CAMERA_START.scale, CAMERA_SCALE_PUSHED),
        1.0,
    )
    .parallel::<Camera2DFraming>(
        camera_h,
        keyframes_vec2(1.0, CAMERA_START.center, CAMERA_PEAK),
    );
    // 3.5–4.5s：公式书写（**逐轮廓依次写出** —— 每个描边子轮廓占一段，顺序游标自动排期；
    // 共用 `parallel` 会让全部轮廓**同时**被写出，那不是"写字"）
    if let Some((first, rest)) = formula_h.split_first() {
        // 把 1s 平摊给各轮廓：N 个轮廓 ⇒ 每段 `1/N`，段间严格首尾相接。
        let n = 1 + rest.len();
        let each = FORMULA_WRITE_SECONDS / n as f64;
        // 用 `Keyframes` 而非 `Phase{omega: N}`：端点值精确为 0 / 1（不受浮点除法影响）。
        enc.animate::<RevealRange>(*first, keyframes_scalar(each, 0.0, 1.0), each);
        for h in rest {
            enc.animate::<RevealRange>(*h, keyframes_scalar(each, 0.0, 1.0), each);
        }
    }

    // ── 提交到**已有 Scene**（不新建）：创建对象 + 编译时序一次完成 ─────────
    let recipe = enc
        .finish()
        .commit(scene)
        .expect("分镜脚本 commit 成功（组件均已登记）");
    // 先解析句柄再搬运字段（`recipe.forest` / `bindings` 一旦被移出，`recipe` 即部分移动）。
    let camera = Camera2DHandle(recipe.id(camera_h).expect("相机句柄已解析"));

    Storyboard {
        forest: recipe.forest,
        bindings: recipe.bindings,
        camera,
    }
}

// ─── 采样 / 曲线构造（纯函数）─────────────────────────────────────────────

/// `y = sin x` 在 `[-π, π]` 上的等步长采样（首尾点都恰为 `y = 0`）。
fn sin_samples() -> Vec<ze_notion::Vec2> {
    let step = std::f32::consts::TAU / CURVE_STEP_COUNT as f32;
    (0..=CURVE_STEP_COUNT)
        .map(|i| {
            let x = -std::f32::consts::PI + (i as f32) * step;
            ze_notion::Vec2::new(x, x.sin())
        })
        .collect()
}

/// 段内从 0 线性到 1 的曲线（`Phase{omega: 1/dur}`）。
fn ramp(dur: f64) -> Curve {
    Curve::Phase {
        omega: (1.0 / dur) as f32,
    }
}

/// 段内把标量从 `from` 插值到 `to`（`Keyframes`，定义域是**段内局部时间**）。
fn keyframes_scalar(dur: f64, from: f32, to: f32) -> Curve {
    Curve::Keyframes {
        keys: vec![(0.0, AnimValue::Scalar(from)), (dur, AnimValue::Scalar(to))],
        interpolation: Interp::Linear,
    }
}

/// 段内把 `Vec2` 从 `from` 插值到 `to`（驱动 `Framing2D.center`）。
fn keyframes_vec2(dur: f64, from: ze_notion::Vec2, to: ze_notion::Vec2) -> Curve {
    Curve::Keyframes {
        keys: vec![(0.0, AnimValue::Vec2(from)), (dur, AnimValue::Vec2(to))],
        interpolation: Interp::Linear,
    }
}

/// 公式 `y = \sin x` → **每个描边子轮廓**一个书写动画对象，返回其**脚本句柄**
/// （**已按阅读顺序** = 包围盒 `min_x` 升序）。
///
/// 每个子轮廓独立成对象（而不是把整条公式塞进一个 `Reveal`）：`Reveal` 的弧长
/// `s` 是在**整条路径**上跨子路径累加的（`normalize_advance`），若合成一条，
/// 字形轮廓会"一个接一个"地写，但切口**无法**在轮廓之间停顿/独立排期；
/// 拆成多个对象后由 `Timeline` 给每个轮廓一段（见 `build`）⇒ 真正的**逐笔**书写。
///
/// **顺序显式定义**（`ze_content::order_by_reading_order`）：返回顺序 = 书写顺序 =
/// 从左往右。**局限**：只按 `min_x` 单键排序 ⇒ 单行公式正确；多行 / 上下标可能不符
/// 阅读顺序。
///
/// 编译失败（占位引擎也会给方块轮廓）时返回空 —— 调用方据此跳过第 6 条分镜
/// （优雅降级，不 panic）。
fn spawn_formula_reveal(enc: &mut SceneEncoder, engine: &InMemoryEngine) -> Vec<Handle> {
    // 字号 0.8 / 锚点 (1.0, 2.0)：镜头推进后仍完整落在可见范围内
    // （推进后可见 x ∈ [-2.14, 5.21]、y ∈ [-1.73, 3.73]）。
    let Some(ir) = content::build_ir(engine, "y = sin x") else {
        eprintln!("[zela-scene] 分镜 6：公式 `y = sin x` 编译失败，跳过书写");
        return Vec::new();
    };

    // 编译产物 → **按阅读顺序排序** → 录进脚本。排序在此处（而非通用工具内）：
    // 书写方向是动画语义，通用路径工具不该知道它。
    let outlines = ze_content::stroke_outlines(&ir, 0.8, ze_notion::Vec2::new(1.0, 2.0), true);

    let mut handles = Vec::new();
    for outline in outlines {
        handles.push(enc.spawn(vec![
            Box::new(Reveal::new(outline.path)),
            Box::new(StrokeSolid::new_in(
                outline.width,
                outline.color,
                MeasureSpace::World,
            )),
        ]));
    }
    handles
}

/// **T3 标注文字**：纯文本 → 描边路径 → 每个子轮廓一个**带 `Transform` 的对象**。
///
/// 与 [`spawn_formula_reveal`] 同口径（逐轮廓独立对象，避免轮廓间桥接线），
/// 差别只在：这里是**静态**文字（不进时间轴），位置由 T1 的 `Transform` 施加
/// （文字几何在原点编译）—— 同时验证 Text 与摆放通道可组合。
///
/// 编译失败（如 Typst 引擎不可用）时静默跳过（优雅降级，不 panic）。
fn spawn_text_label(enc: &mut SceneEncoder, engine: &InMemoryEngine) {
    let source = ze_content::text_literal_source("math animation");
    let Some(ir) = content::build_ir(engine, &source) else {
        eprintln!("[zela-scene] T3 标注文字编译失败，跳过");
        return;
    };
    for outline in ze_content::stroke_outlines(&ir, TEXT_FONT_SIZE, ze_notion::Vec2::ZERO, false) {
        enc.spawn(vec![
            Box::new(Path::new(outline.path)),
            Box::new(StrokeSolid::new_in(
                outline.width,
                outline.color,
                MeasureSpace::World,
            )),
            Box::new(Transform {
                position: TEXT_POS,
                ..Default::default()
            }),
        ]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zela_scene_ecs::animation::{Target, Timeline};
    use zela_scene_ecs::contract::{ObjectId, SceneRead};
    use ze_prerender::placeholder_engine;

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

        // 分镜 3 + 6：书写动画对象（曲线 1 个 + 公式子轮廓若干）。
        let mut reveals = 0;
        scene.for_each::<Reveal>(|_, _| reveals += 1);
        assert!(reveals >= 2, "曲线 + 公式至少两个书写对象，实际 {reveals}");

        // 分镜 4/5/6 的时序：0–2 / (wait 0.5) / 2.5–3.5 / 3.5–4.5。
        // 由编译产物反推：节点 offset + span 即区间。
        let segs = timeline_segments(&sb);
        assert_eq!(segs[0], (0.0, 2.0), "曲线书写 0–2s");
        assert_eq!(segs[1], (2.5, 1.0), "镜头推进 2.5–3.5s");

        // 分镜 6 是**逐轮廓顺序段**（不是一段并行动作）：每个轮廓各自一个 `play`
        // ⇒ 各有自己的节点；段数与轮廓数相同、严格首尾相接、合计 1s。
        let formula = &segs[2..];
        assert!(
            formula.len() >= 2,
            "公式应逐轮廓分段（≥2 段），实际 {} 段：{segs:?}",
            formula.len()
        );
        assert_eq!(formula[0].0, 3.5, "公式书写始于 3.5s");
        let total: f64 = formula.iter().map(|(_, dur)| dur).sum();
        assert!(
            (total - FORMULA_WRITE_SECONDS).abs() < 1e-9,
            "公式书写共占 1s（各轮廓平摊），实际 {total}"
        );
        assert!(
            formula.iter().all(|(_, dur)| *dur > 0.0),
            "每段必须有正时长：{formula:?}"
        );
        for w in formula.windows(2) {
            assert!(
                (w[0].0 + w[0].1 - w[1].0).abs() < 1e-9,
                "逐轮廓段必须**首尾相接**（顺序游标）：{w:?}"
            );
        }

        // 分镜 5 是**并行**两动作（scale + center）：同区间、同节点。
        // T0-②：按**对象**筛选（指向本分镜那台 2D 相机的绑定），比按类别更精确。
        let cam_nodes: Vec<_> = sb
            .bindings
            .bindings
            .iter()
            .filter(|b| b.target.target_id() == sb.camera.0)
            .map(|b| b.clock)
            .collect();
        assert_eq!(cam_nodes.len(), 2, "scale 与 center 各一条绑定");
        assert_eq!(cam_nodes[0], cam_nodes[1], "两者共享同一时钟节点（同区间）");
    }

    /// **书写方向**：公式轮廓必须按包围盒 `min_x` **升序**排期（从左往右写），
    /// 而不是"引擎给什么顺序就写什么顺序"。
    ///
    /// 证伪：把阅读顺序的比较改成降序（`y.cmp(&x)`）⇒ 本用例 RED。
    #[test]
    fn formula_contours_are_ordered_left_to_right() {
        let engine = placeholder_engine();
        let mut enc = SceneEncoder::new();
        let handles = spawn_formula_reveal(&mut enc, &engine);
        assert!(
            handles.len() >= 2,
            "公式至少两个轮廓，实际 {}",
            handles.len()
        );

        let mut scene = Scene::new();
        let recipe = enc.finish().commit(&mut scene).expect("commit 成功");

        let xs: Vec<f32> = handles
            .iter()
            .map(|h| {
                let id = recipe.id(*h).expect("句柄已解析");
                scene
                    .with::<Reveal, _>(id, |r| r.path.compute_bounds().map(|b| b.min.x))
                    .expect("Reveal 对象在场景里")
                    .expect("轮廓应有包围盒")
            })
            .collect();
        for w in xs.windows(2) {
            assert!(
                w[1] >= w[0],
                "轮廓必须按 min_x 升序排期（从左往右写），实际 {xs:?}"
            );
        }
    }

    /// **T3 + T1 在分镜里的落点**：标注文字对象存在、带真实几何，且经 `Transform` 摆放。
    #[test]
    fn storyboard_spawns_transformed_text_label() {
        let mut scene = Scene::new();
        let _ = build(&mut scene, &placeholder_engine());

        // 分镜里的 `Path` 组件只属于 T3 标注文字（公式用 `Reveal`，网格用 `Line`）。
        let mut ids = Vec::new();
        scene.for_each::<Path>(|id, _| ids.push(id));
        assert!(!ids.is_empty(), "T3 标注文字应已装配");

        let mut placed = 0usize;
        for id in ids {
            if scene.with::<Transform, _>(id, |t| t.position) == Some(TEXT_POS) {
                placed += 1;
                let has_geometry = scene
                    .with::<Path, _>(id, |p| p.0.compute_bounds().is_some())
                    .unwrap_or(false);
                assert!(has_geometry, "文字对象应带真实几何");
            }
        }
        assert!(placed > 0, "标注文字必须经 `Transform` 摆放到 {TEXT_POS:?}");
    }

    /// **T2 在分镜里的落点**：箭头图元已装配（同时证明 `add_known` 登记齐备 ——
    /// 未登记时 `commit` 会直接 `Err`）。
    #[test]
    fn storyboard_spawns_arrow() {
        let mut scene = Scene::new();
        let _ = build(&mut scene, &placeholder_engine());
        let mut n = 0;
        scene.for_each::<Arrow>(|_, _| n += 1);
        assert_eq!(n, 1, "分镜应装配一支静态箭头");
    }

    /// §11.2：新增驱动目标**不需要改枚举、也不需要改 System 源码** ——
    /// 本脚本用的 `Camera2DFraming` 就是 P0 新增、无 System 改动的目标。
    ///
    /// T0-② 后判据从 `kind()` 改为 **`name()` / `target_id()`**：名字由 target 自带，
    /// 全仓**没有**需要同步维护的目标清单。
    #[test]
    fn new_target_needs_no_enum_or_system_change() {
        let t = Camera2DFraming {
            id: ObjectId::DANGLING,
        };
        assert_eq!(t.name(), "Camera2DFraming");
        assert_eq!(t.target_id(), ObjectId::DANGLING);
    }

    /// §11.3：改掉第一段动画的时长后，后续动画的时刻**自动跟随**。
    #[test]
    fn later_shots_follow_first_duration_change() {
        let mut tl = Timeline::new();
        // 与 `build` 相同的编排（此处只关心时间，不需要场景对象）。
        tl.play(
            ramp(2.0),
            RevealRange {
                id: ObjectId::DANGLING,
            },
            2.0,
        );
        tl.wait(0.5);
        tl.play(
            keyframes_scalar(1.0, 40.0, 110.0),
            Camera2DFraming {
                id: ObjectId::DANGLING,
            },
            1.0,
        );
        tl.play(
            ramp(1.0),
            RevealRange {
                id: ObjectId::DANGLING,
            },
            1.0,
        );

        assert_eq!(tl.schedule()[2].start, 3.5);
        tl.set_duration(0, 3.0);
        assert_eq!(tl.schedule()[1].start, 3.5, "镜头推进自动顺延");
        assert_eq!(tl.schedule()[2].start, 4.5, "公式书写自动顺延");
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
