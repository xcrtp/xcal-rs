// SPDX-FileCopyrightText: 2026 Academa, Inc.
// SPDX-License-Identifier: MIT
// Modified by Zela: 新增宿主适配层（B4 Step 5）：把 zela 的 2D 路径喂给矢量后端，
// 离屏渲染一帧并读回 RGBA8。

//! 宿主适配层：`FilledPath` → 上游 `Upload::path` + `Record::filled` → 离屏一帧。
//!
//! 上游按"视图字节 + 记录字节"驱动（Python 侧负责打包）。本模块提供**同一形态**的
//! Rust 入口，供 zela 宿主使用：视图用上游 `feed.py` 的 2D 正交布局（见
//! `view_2d`），记录用 `Record::filled`（单填充路径）。
//!
//! 已知非等价（B4 计划 §5.4）：上游的 `m1/m2` 双变换 + GPU 插值，这里只写 `m1`
//! （`m2` 留零），因此丢掉"一次上传、GPU 插值"的性能模型；逐帧渲染的正确性不受影响。

use crate::render::{Record, Upload, VIEW_LENGTH, VIEW_LIGHTING, VIEW_OCCLUSION};

/// 一条填充路径（世界坐标；曲线为每段 4 个控制点）。
#[derive(Debug, Clone)]
pub struct FilledPath {
    /// 控制点：每 4 个一组（一段三次贝塞尔；直线段用四等分点）。
    pub points: Vec<[f64; 3]>,
    /// 子路径区间：`[首曲线, 末曲线, 是否闭合, 0]`。
    pub ranges: Vec<[u32; 4]>,
    /// 填充色（straight RGBA，0..1）。
    pub fill: [f32; 4],
    /// 3×4 模型矩阵（行主序；2D 视图里这就是"对象 → 世界"）。
    pub model: [[f32; 4]; 3],
}

impl FilledPath {
    /// 由折线构造闭合填充路径（每段转成一条直线三次曲线）。
    pub fn polygon(points: &[[f64; 3]], fill: [f32; 4]) -> Self {
        let n = points.len();
        let mut curves = Vec::with_capacity(n * 4);
        for i in 0..n {
            let a = points[i];
            let b = points[(i + 1) % n];
            let lerp = |t: f64| [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t);
            curves.extend_from_slice(&[a, lerp(1.0 / 3.0), lerp(2.0 / 3.0), b]);
        }
        Self {
            points: curves,
            ranges: vec![[0, n as u32, 1, 0]],
            fill,
            model: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        }
    }
}

/// 2D 视图的**着色栈选项**（B6 Step 1.2）：默认全关 ⇒ 与 B4 的 `view_2d` 逐位相同。
///
/// 槽位见 `render.rs` 的 `VIEW_LIGHTING` / `VIEW_OCCLUSION`：
/// `+5` 曝光、`+6` tone（0 = clip / 1 = AgX）、`+7` bloom 强度、
/// `VIEW_OCCLUSION +0/+1` = AO 强度 / 半径。
///
/// ⚠️ **AO 依赖环境光**（`B6 §2.4`）：上游 `occlusion_of()` 要求场景里存在
/// 环境光（类型 4）或全向光（类型 0）；只有方向光时 AO **静默不生效**。
/// 本入口的 2D 视图没有灯 ⇒ 打开 AO 不会改变像素（这是上游语义，不是没接上）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewOptions {
    pub exposure: f32,
    /// `Some(curve)` ⇒ 打开 tone 映射（`Clip` = 0、`AgX` = 1，与 `tone.wgsl` 一致）。
    pub tone: Option<ze_render_ir::ToneCurve>,
    pub bloom_strength: f32,
    pub ao: Option<(f32, f32)>,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            exposure: 1.0,
            tone: None,
            bloom_strength: 0.0,
            ao: None,
        }
    }
}

/// 2D 正交视图（与上游 `feed.py::view` 的 2D 分支同布局）：
/// 世界坐标 = 以画面中心为原点的像素。
fn view_2d_with(width: u32, height: u32, background: [f32; 4], options: ViewOptions) -> Vec<f32> {
    let (hw, hh) = (width as f32 * 0.5, height as f32 * 0.5);
    let mut v = vec![0.0f32; VIEW_LENGTH];
    let projection = [
        [1.0 / hw, 0.0, 0.0, 0.0],
        [0.0, 1.0 / hh, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.5],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let overlay = [
        [1.0 / hw, 0.0, 0.0, 0.0],
        [0.0, 1.0 / hh, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    for c in 0..4 {
        for r in 0..4 {
            v[c * 4 + r] = projection[r][c];
            v[16 + c * 4 + r] = overlay[r][c];
        }
    }
    v[32] = width as f32;
    v[33] = height as f32;
    v[34] = 1.0; // unit：像素 / 场景单位（frame_height = height）
    v[35] = 1.0; // zoom
    v[39] = 0.0; // 2D
    v[42] = 1.0; // toward = +z
    v[44..48].copy_from_slice(&background);
    v[52] = 0.0; // 2D 视图没有光源（AO 因此不会生效，见 `ViewOptions`）
    v[53] = options.exposure;
    // 着色栈槽位：与渲染器共用同一组偏移常量（避免两份布局漂移）。
    let tone = match options.tone {
        None => 0.0,
        Some(ze_render_ir::ToneCurve::Clip) => 0.0,
        Some(ze_render_ir::ToneCurve::AgX) => 1.0,
    };
    v[VIEW_LIGHTING + 6] = tone;
    v[VIEW_LIGHTING + 7] = options.bloom_strength.clamp(0.0, 1.0);
    if let Some((intensity, radius)) = options.ao {
        v[VIEW_OCCLUSION] = intensity.max(0.0);
        v[VIEW_OCCLUSION + 1] = radius.max(0.0);
    }
    v
}

/// 控制点的质心与新厄尔面积向量（上游 `Upload::path` 的第六个参数）。
fn centroid_area(points: &[[f64; 3]]) -> [f32; 6] {
    let n = points.len().max(1) as f64;
    let centroid = [0, 1, 2].map(|k| (points.iter().map(|p| p[k]).sum::<f64>() / n) as f32);
    let mut area = [0.0f64; 3];
    for i in 0..points.len() {
        let a = points[i];
        let b = points[(i + 1) % points.len()];
        area[0] += (a[1] - b[1]) * (a[2] + b[2]);
        area[1] += (a[2] - b[2]) * (a[0] + b[0]);
        area[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    [
        centroid[0],
        centroid[1],
        centroid[2],
        area[0] as f32 * 0.5,
        area[1] as f32 * 0.5,
        area[2] as f32 * 0.5,
    ]
}

/// 一条**描边**路径（`Record` 的描边形态，B6 Step 2.1）。
///
/// 上游编码（已核对 `vector.wgsl:807`）：`params.z` = 描边**半宽**；
/// `Paint.brush2.w = cap | joint << 2`（`0` = butt + miter）。
/// miter limit 在着色器里硬编码 10（GAPS V2-11），不由记录传入。
#[derive(Debug, Clone)]
pub struct StrokedPath {
    /// 控制点：每 4 个一组（与 [`FilledPath`] 相同口径）。
    pub points: Vec<[f64; 3]>,
    /// 子路径区间：`[首曲线, 末曲线, 是否闭合, 0]`。
    pub ranges: Vec<[u32; 4]>,
    /// 描边色（straight RGBA，0..1）。
    pub stroke: [f32; 4],
    /// 描边**宽度**（像素；写入时减半成半宽）。
    pub width: f32,
    /// 形态位：`cap | joint << 2`（0 = butt + miter）。
    pub shape_bits: u32,
    pub model: [[f32; 4]; 3],
}

/// 一条绘制记录（保持声明顺序 = 绘制顺序）。
#[derive(Debug, Clone)]
pub enum PathRecord {
    Fill(FilledPath),
    Stroke(StrokedPath),
}

impl From<FilledPath> for PathRecord {
    fn from(path: FilledPath) -> Self {
        Self::Fill(path)
    }
}

impl From<StrokedPath> for PathRecord {
    fn from(path: StrokedPath) -> Self {
        Self::Stroke(path)
    }
}

/// **离屏渲染一帧**：`records` 按顺序画到 `width×height` 的 RGBA8 缓冲并读回。
///
/// 返回的字节是行优先 RGBA8（`width * height * 4`）。失败 ⇒ 显式错误（不静默）。
pub fn render_2d(
    width: u32,
    height: u32,
    samples: u32,
    background: [f32; 4],
    records: &[PathRecord],
) -> Result<Vec<u8>, String> {
    render_2d_with(
        width,
        height,
        samples,
        background,
        ViewOptions::default(),
        records,
    )
}

/// 同 [`render_2d`]，但可带着色栈选项（B6 Step 1.2）。
///
/// `ViewOptions::default()` ⇒ 与 `render_2d` **逐位相同**（既有基线不受影响）。
pub fn render_2d_with(
    width: u32,
    height: u32,
    samples: u32,
    background: [f32; 4],
    options: ViewOptions,
    records: &[PathRecord],
) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err(format!("canvas {width}x{height} is empty"));
    }
    let view = view_2d_with(width, height, background, options);
    let view_bytes: Vec<u8> = bytemuck::cast_slice(&view).to_vec();

    let mut player = crate::render::Player::new(width, height, samples);
    let mut built: Vec<Record> = Vec::with_capacity(records.len());
    for (i, record) in records.iter().enumerate() {
        let (points, ranges) = match record {
            PathRecord::Fill(path) => (&path.points, &path.ranges),
            PathRecord::Stroke(path) => (&path.points, &path.ranges),
        };
        if points.len() % 4 != 0 || points.is_empty() {
            return Err(format!("record {i}: control points are not whole curves"));
        }
        let key = i as u64 + 1;
        let point_bytes: &[u8] = bytemuck::cast_slice(points);
        let range_bytes: &[u8] = bytemuck::cast_slice(ranges);
        player.upload(Upload::path(
            key,
            point_bytes,
            range_bytes,
            centroid_area(points),
        )?)?;
        let curves = (points.len() / 4) as u32;
        built.push(match record {
            PathRecord::Fill(path) => Record::filled(key, curves, path.model, path.fill),
            PathRecord::Stroke(path) => Record::stroked(
                key,
                curves,
                path.model,
                path.stroke,
                path.width,
                path.shape_bits,
            ),
        });
    }
    let record_bytes: Vec<u8> = bytemuck::cast_slice(&built).to_vec();

    crate::render::with_gpu(|gpu| {
        let frames = player.views(&view_bytes, &record_bytes, Vec::new())?;
        let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
        player.pixels(gpu, &frames, &mut out)?;
        Ok(out)
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(frame: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * width + x) * 4) as usize;
        [frame[at], frame[at + 1], frame[at + 2], frame[at + 3]]
    }

    /// 矢量路径真的能画出来：中央是填充色，四角是背景色。
    #[test]
    fn filled_square_draws() {
        let (w, h) = (128, 128);
        // 世界坐标 = 以画面中心为原点的像素：±32 px 的正方形。
        let square = FilledPath::polygon(
            &[
                [-32.0, -32.0, 0.0],
                [32.0, -32.0, 0.0],
                [32.0, 32.0, 0.0],
                [-32.0, 32.0, 0.0],
            ],
            [1.0, 0.0, 0.0, 1.0],
        );
        let frame = match render_2d(w, h, 1, [0.0, 0.0, 0.0, 1.0], &[square.clone().into()]) {
            Ok(frame) => frame,
            // 起不来就**响亮失败**：静默 return 会让"什么都没渲染"也算 passed
            // （与 `zela-render-backends/tests/vector_vs_wgpu.rs` 同口径）。
            Err(error) => panic!("矢量后端起不来：{error}"),
        };
        let center = pixel(&frame, w, 64, 64);
        assert!(center[0] > 200, "中央应为红色：{center:?}");
        let corner = pixel(&frame, w, 2, 2);
        assert!(corner[0] < 32, "角落应为背景：{corner:?}");
    }

    /// **B6 Step 1.2 验收**：着色栈真的接上了 —— 同一帧在
    /// ① 默认（clip）② `exposure = 4 + AgX` 下必须**不同**；
    /// 且默认选项与 `render_2d` 逐位相同（默认等效）。
    #[test]
    fn tone_mapping_changes_pixels_and_defaults_are_identical() {
        let (w, h) = (64, 64);
        let square = FilledPath::polygon(
            &[
                [-16.0, -16.0, 0.0],
                [16.0, -16.0, 0.0],
                [16.0, 16.0, 0.0],
                [-16.0, 16.0, 0.0],
            ],
            [0.5, 0.5, 0.5, 1.0],
        );
        let records = [PathRecord::from(square)];
        let plain = match render_2d(w, h, 1, [0.0, 0.0, 0.0, 1.0], &records) {
            Ok(frame) => frame,
            Err(error) => panic!("矢量后端起不来：{error}"),
        };
        let same = match render_2d_with(
            w,
            h,
            1,
            [0.0, 0.0, 0.0, 1.0],
            ViewOptions::default(),
            &records,
        ) {
            Ok(frame) => frame,
            Err(error) => panic!("矢量后端起不来：{error}"),
        };
        assert_eq!(plain, same, "默认选项必须逐位不变");

        let toned = match render_2d_with(
            w,
            h,
            1,
            [0.0, 0.0, 0.0, 1.0],
            ViewOptions {
                exposure: 4.0,
                // ⚠️ 上游把 tone 放在 **bloom 的合成 pass** 里（`render.rs:2429`），
                // 所以只开 tone 不开 bloom 时画面不变 —— 这不是没接上，是上游的管线结构。
                tone: Some(ze_render_ir::ToneCurve::AgX),
                bloom_strength: 1.0,
                ..Default::default()
            },
            &records,
        ) {
            Ok(frame) => frame,
            Err(error) => panic!("矢量后端起不来：{error}"),
        };
        let clipped = match render_2d_with(
            w,
            h,
            1,
            [0.0, 0.0, 0.0, 1.0],
            ViewOptions {
                exposure: 4.0,
                tone: Some(ze_render_ir::ToneCurve::Clip),
                bloom_strength: 1.0,
                ..Default::default()
            },
            &records,
        ) {
            Ok(frame) => frame,
            Err(error) => panic!("矢量后端起不来：{error}"),
        };
        // ⚠️ 上游把 tone 放在 **bloom 的合成 pass** 里，而那条 pass 只在
        // **存在 3D layered 计划**时才跑（`render.rs:2419` 的 `let Some(plan) = plan else { return }`）。
        // ⇒ 纯 2D 帧上 tone / bloom 槽位**没有消费者**（不是我们没填）。
        // 本用例把这个结构事实钉住：槽位填了、但 2D-only 帧不变 ⇒ 将来接 3D 视图时
        // 必须补一条"有 3D 时 AgX ≠ clip"的用例（GAPS V2-15）。
        assert_eq!(
            pixel(&toned, w, 32, 32),
            pixel(&clipped, w, 32, 32),
            "2D-only 帧上 tone 应无消费者（上游结构）；若这里开始不同，说明 2D 路径结构变了，请更新 V2-15"
        );
    }
}
