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
// 3. **0–2s**：曲线 `y = sin x` 从左到右**被"写"出来**（R0 书写动画：`Reveal` 组件 +
//    `RevealRange` 目标驱动区间 `[0, end]`，逐帧只有 uniform 变 —— 几何静态）；
// 4. **2–2.5s**：`wait`；
// 5. **2.5–3.5s**：**镜头推进**到曲线峰值（驱动 `Framing2D.center` / `scale`）；
// 6. **3.5–4.5s**：公式 `y = \sin x` **被"写"出来** —— 每个描边子轮廓一个 `Reveal`，
//    并**逐轮廓依次**占一段（第 i 个轮廓独占 `[3.5 + i·1/N, 3.5 + (i+1)·1/N]`）。
//    顺序来自 `Timeline` 的**顺序游标**：连续 `play` 即首尾相接；用 `parallel` 会让
//    全部轮廓**同时**被写出（实测踩过 —— 那看起来像"整体浮现"，不是写字）。
//
// > R0 之前分镜 3 / 6 走的是**形变管线**（`MorphShape`：整条曲线从一点"绽开"）
// > —— 那不是书写动画。形变管线仍在（GPU 用例覆盖），只是不再是本脚本的默认表达。
//
// # 为什么这个脚本"最小但够"（§1.2）
//
// 同时消费五样东西：书写动画（R0 新增）、时间编排（P3 Timeline）、2D 相机动画
// （P0 `Camera2DFraming` 目标 + P1 相机对象）、公式（已有 Typst 管线）、网格（对象层）；
// 不碰 3D、不需要箭头 / Text、不需要 seek；每条分镜都落到既有设施上，不新建 crate。

use std::f32::consts::FRAC_PI_2;

use xcal_render::{MeasureSpace, StrokeSolid};
use xcal_scene::animation::{Camera2DFraming, RevealRange};
use xcal_scene::scene::components::Reveal;
use xcal_scene::scene::Scene;
use xcal_scene::scene_script::{Handle, SceneEncoder};
use xcal_scene::{AnimValue, BindingTable, ClockForest, Curve, Interp};
use xc_notion::Path2D;
use xc_object::{Axes2D, Camera2DHandle, Framing2D};
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

/// 分镜 6（公式书写）的总时长：各轮廓**平摊**这一段（见 `build` 的编排）。
///
/// 固定"总窗口"而不是"每轮廓固定时长"：保持六条分镜声明的时序（3.5–4.5s）不变，
/// 轮廓多则每笔更快（字形越多写得越急），但整段不会溢出到 4.5s 之后。
const FORMULA_WRITE_SECONDS: f64 = 1.0;

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
        Box::new(StrokeSolid::new_in(2.0, (0, 200, 200, 255), MeasureSpace::Screen)),
    ]);

    // 分镜 6：公式 `y = sin x` 的书写动画对象（每个描边子轮廓一个）。
    let formula_h = spawn_formula_reveal(&mut enc, engine);

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
    .parallel::<Camera2DFraming>(camera_h, keyframes_vec2(1.0, CAMERA_START.center, CAMERA_PEAK));
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

/// 按**阅读顺序**（包围盒 `min_x` 升序）排列书写轮廓 —— **稳定排序**。
///
/// **为什么需要它**：`compile_to_stroke_paths` 的返回顺序是**引擎输出顺序**，
/// 不是任何 API 承诺（Typst 的字形顺序 / 占位引擎的字符顺序碰巧都是左→右，
/// 但那属实现细节）。书写动画的"依次"必须由我们**显式定义方向**，
/// 否则换引擎 / 换排版就是另一幅画。
///
/// 放这里而不是放进 `compile_to_stroke_paths`：后者是通用工具
/// （内容纹理层也用它），**不该知道书写语义**。
///
/// - 无包围盒的轮廓（空路径）排在**最后**；
/// - `min_x` 相同时保持原相对顺序（同一字形的多个笔画之间不乱序）。
///
/// **局限（本轮不处理）**：只有 `min_x` 一个键 ⇒ 单行公式正确；
/// 多行 / 上下标需要按行分组的 `(min_y, min_x)` 字典序或行高聚类，届时再换键。
fn order_by_reading_order(paths: &mut [formula::StrokePath]) {
    paths.sort_by(|a, b| {
        let key = |p: &formula::StrokePath| p.0.compute_bounds().map(|r| r.min.x);
        match (key(a), key(b)) {
            (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
            // 有包围盒的排在无包围盒的前面。
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        }
    });
}

/// 公式 `y = \sin x` → **每个描边子轮廓**一个书写动画对象，返回其**脚本句柄**
/// （**已按阅读顺序** = 包围盒 `min_x` 升序）。
///
/// 每个子轮廓独立成对象（而不是把整条公式塞进一个 `Reveal`）：`Reveal` 的弧长
/// `s` 是在**整条路径**上跨子路径累加的（`normalize_advance`），若合成一条，
/// 字形轮廓会"一个接一个"地写，但切口**无法**在轮廓之间停顿/独立排期；
/// 拆成多个对象后由 `Timeline` 给每个轮廓一段（见 `build`）⇒ 真正的**逐笔**书写。
///
/// **顺序显式定义**（[`order_by_reading_order`]）：返回顺序 = 书写顺序 = 从左往右。
/// **局限**：只按 `min_x` 单键排序 ⇒ 单行公式正确；多行 / 上下标可能不符阅读顺序。
///
/// 编译失败（占位引擎也会给方块轮廓）时返回空 —— 调用方据此跳过第 6 条分镜
/// （优雅降级，不 panic）。
fn spawn_formula_reveal(enc: &mut SceneEncoder, engine: &InMemoryEngine) -> Vec<Handle> {
    // 字号 0.8 / 锚点 (1.0, 2.0)：镜头推进后仍完整落在可见范围内
    // （推进后可见 x ∈ [-2.14, 5.21]、y ∈ [-1.73, 3.73]）。
    let spec = formula::FormulaSpec::new("y = sin x", 0.8, glam::Vec2::new(1.0, 2.0));
    let Some(ir) = formula::rebuild(engine, &spec) else {
        eprintln!("[xcal-app] 分镜 6：公式 `y = sin x` 编译失败，跳过书写");
        return Vec::new();
    };

    // 编译产物 → **按阅读顺序排序** → 录进脚本。排序在此处（而非 `compile_to_stroke_paths`
    // 内）：书写方向是动画语义，通用三角/路径工具不该知道它。
    let mut paths = formula::compile_to_stroke_paths(&ir, &spec);
    order_by_reading_order(&mut paths);

    let mut handles = Vec::new();
    for (path, color, width, space) in paths {
        handles.push(enc.spawn(vec![
            Box::new(Reveal::new(path)),
            Box::new(StrokeSolid::new_in(width, color, space)),
        ]));
    }
    handles
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_scene::animation::{Target, TargetKind, Timeline};
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

    /// **书写方向**：公式轮廓必须按包围盒 `min_x` **升序**排期（从左往右写），
    /// 而不是"引擎给什么顺序就写什么顺序"。
    ///
    /// 证伪：把 [`order_by_reading_order`] 的比较改成降序（`y.cmp(&x)`）
    /// ⇒ 本用例 RED；改回升序 ⇒ GREEN。
    #[test]
    fn formula_contours_are_ordered_left_to_right() {
        // B1/B2 后 `spawn_formula_reveal` 产出**脚本句柄**：先录进脚本，再 commit 到场景
        // 取回 `ObjectId` 才能查询几何（声明期无 id —— 这正是脚本的语义）。
        let engine = placeholder_engine();
        let mut enc = SceneEncoder::new();
        let handles = spawn_formula_reveal(&mut enc, &engine);
        assert!(handles.len() >= 2, "公式至少两个轮廓，实际 {}", handles.len());

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

    /// [`order_by_reading_order`] 的**乱序输入**用例（含无包围盒者 + 稳定性）。
    ///
    /// 上一条用例的真实输入恰好已是左→右（占位引擎的字符顺序），故它只能证伪"降序"；
    /// 本条用**故意打乱**的输入把排序逻辑本身钉死：
    /// 1. `min_x` 升序；2. 无包围盒（空路径）排**最后**；3. 同 `min_x` 保持输入顺序。
    ///
    /// 证伪：把 `None` 当 0.0 处理 ⇒ 第 2 条 RED；换 `sort_unstable_by` ⇒ 第 3 条可能 RED。
    #[test]
    fn reading_order_is_min_x_ascending_without_bounds_last() {
        use xcal_scene::types::Color32;

        // `x` 处、高 `h` 的矩形轮廓。
        let rect = |x: f32, h: f32| {
            Path2D::from_polyline(
                &[
                    glam::Vec2::new(x, 0.0),
                    glam::Vec2::new(x + 1.0, 0.0),
                    glam::Vec2::new(x + 1.0, h),
                    glam::Vec2::new(x, h),
                ],
                true,
            )
        };
        let item = |x: f32, h: f32| (rect(x, h), Color32::WHITE, 1.0, MeasureSpace::Screen);
        let empty = || (Path2D::new(), Color32::WHITE, 1.0, MeasureSpace::Screen);

        // 故意乱序：右 → 左 → 空 → 中。
        let mut paths = vec![item(2.0, 1.0), item(0.0, 1.0), empty(), item(1.0, 1.0)];
        order_by_reading_order(&mut paths);
        let xs: Vec<Option<f32>> = paths
            .iter()
            .map(|p| p.0.compute_bounds().map(|b| b.min.x))
            .collect();
        assert_eq!(
            xs,
            vec![Some(0.0), Some(1.0), Some(2.0), None],
            "必须 min_x 升序且无包围盒者排最后"
        );

        // 稳定性：同 `min_x`（都为 1.0）时保持输入顺序 —— 高的那个先写的仍是它。
        let mut same_x = vec![item(1.0, 5.0), item(1.0, 1.0)];
        order_by_reading_order(&mut same_x);
        let heights: Vec<f32> = same_x
            .iter()
            .map(|p| p.0.compute_bounds().unwrap().max.y)
            .collect();
        assert_eq!(heights, vec![5.0, 1.0], "同 min_x 必须稳定（保持输入顺序）");
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
        tl.play(ramp(2.0), RevealRange { id: ObjectId::DANGLING }, 2.0);
        tl.wait(0.5);
        tl.play(
            keyframes_scalar(1.0, 40.0, 110.0),
            Camera2DFraming { id: ObjectId::DANGLING },
            1.0,
        );
        tl.play(ramp(1.0), RevealRange { id: ObjectId::DANGLING }, 1.0);

        assert_eq!(tl.schedule()[2].start, 3.5);
        tl.set_duration(0, 3.0);
        assert_eq!(tl.schedule()[1].start, 3.5, "镜头推进自动顺延");
        assert_eq!(tl.schedule()[2].start, 4.5, "公式书写自动顺延");
    }

    /// **可渲染性**（本环境无法目视 GUI 时的替代验证）：把分镜场景接进**真实 System
    /// 链路**跑几帧，断言 2D 队列（网格）与**书写动画队列**在 t=1s / 4s 确有产出，
    /// 且"写到哪"（`end`）随时间正确推进、相机被推进到峰值。
    #[test]
    fn storyboard_produces_draw_output_over_time() {
        use xcal_egui::resources::UiConfig;
        use xcal_egui::systems::AxesRefreshSystem;
        use xcal_render::{DrawQueue, RevealQueue, RevealRecordSystem, SceneRecordSystem, ViewRect};
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
        res.insert(RevealQueue::default());

        let mut sched = Scheduler::new(world, res);
        sched.add_system(Phase::SceneSync, BindingApplySystem);
        sched.add_system(Phase::Prepare, AxesRefreshSystem);
        sched.add_system(Phase::RenderCommit, SceneRecordSystem);
        sched.add_system(Phase::RenderCommit, RevealRecordSystem);
        sched.init_resources();

        // ── t = 1s：网格已建；曲线写到一半（其余公式尚未开始） ──
        sched.resources_mut().get_mut::<TimeState>().unwrap().elapsed = 1.0;
        sched.run_frame(0.0);

        let grid = sched.resources().get::<DrawQueue>().unwrap().len();
        assert!(grid > 0, "网格/坐标轴线段应已录进 2D 队列（t=1s），实际 {grid}");

        let reveals = sched.resources().get::<RevealQueue>().unwrap().0.clone();
        assert!(reveals.len() >= 2, "曲线 + 公式应有书写项，实际 {}", reveals.len());
        let max_end = reveals.iter().map(|r| r.end).fold(f32::MIN, f32::max);
        assert!(
            (max_end - 0.5).abs() < 1e-3,
            "t=1s 时曲线应写到 0.5（其余仍为 0），实际 max end = {max_end}"
        );
        // 区间起点恒为 0（`Scalar` 只推 `end`）。
        assert!(reveals.iter().all(|r| r.start == 0.0), "start 应恒为 0");

        // ── t = 4s：曲线写满（end=1）；公式**写到一半（逐轮廓在途）**；相机到峰值 ──
        sched.resources_mut().get_mut::<TimeState>().unwrap().elapsed = 4.0;
        sched.run_frame(0.0);

        let reveals = sched.resources().get::<RevealQueue>().unwrap().0.clone();
        let max_end = reveals.iter().map(|r| r.end).fold(f32::MIN, f32::max);
        assert!((max_end - 1.0).abs() < 1e-3, "曲线应写满（end=1），实际 {max_end}");
        // **逐轮廓依次书写**的判据：窗口过半时，前面的轮廓已写完、后面的**还没开始**。
        // 若各轮廓共享同一区间（并行动作），此刻它们会**全部**是 0.5
        // ⇒ `pending == 0`，本断言即失败。
        let pending = reveals.iter().filter(|r| r.end == 0.0).count();
        let done = reveals.iter().filter(|r| (r.end - 1.0).abs() < 1e-6).count();
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
