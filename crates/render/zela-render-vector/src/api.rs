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
    /// **B6 Step 1.2**：3D 视图里按光照着色 —— 上游 `feed.py:1134` 的 `shade_in_3d`
    /// （`flags |= LIT`）。2D 视图里无意义（上游同样只在 `three_d` 时置位）。
    pub shade_in_3d: bool,
    /// **B6 Step 1.2**：材质 `[metallic, roughness, reflectance]` —— 上游 `feed.py:1150`
    /// （`material = (m, r, f, 1)`）。`Some(_)` ⇒ 该平面路径按 PBR / 环境光受光；
    /// `None` ⇒ 上游的 `NO_MATERIAL`（`material[3] = 0`）。
    pub material: Option<[f32; 3]>,
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
            shade_in_3d: false,
            material: None,
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

/// 一条 **3D 网格**记录（B10 Step 2）：模型空间顶点 + 三角形索引 + 材质。
///
/// 输入精度与上游一致：位置 / UV / 法线都是 **f64**，三角形是 u32。上游
/// `Mesh::new` 要求"每个点一个 (u, v)" ⇒ [`uvs`](Self::uvs) 为 `None` 时本入口补零；
/// `normals` 为 `None` 时由上游按面积加权自动求法线（`smooth_normals`）。
///
/// 本批**不带纹理**（GAPS V2-13：纹理 / 渐变的宿主入口仍缺）。
#[derive(Debug, Clone, PartialEq)]
pub struct MeshRecord {
    /// 顶点位置（模型空间）。
    pub positions: Vec<[f64; 3]>,
    /// 可选逐顶点 UV（长度须等于 `positions`；`None` ⇒ 上游补 `(0, 0)`）。
    pub uvs: Option<Vec<[f64; 2]>>,
    /// 可选逐顶点法线（长度须等于 `positions`；`None` ⇒ 上游按面积加权求）。
    pub normals: Option<Vec<[f64; 3]>>,
    /// 三角形索引（三个一组；`positions` 的下标）。
    pub triangles: Vec<u32>,
    /// 表面色（straight RGBA，0..1）；受光时作为反照率。
    pub fill: [f32; 4],
    /// 3×4 模型矩阵（行主序）。
    pub model: [[f32; 4]; 3],
    /// 材质 `[metallic, roughness, reflectance]`；`Some(_)` ⇒ 该网格按材质逐片元受光
    /// （上游 `material[3] = 1`）。`None` ⇒ 不受光（只按 `fill` 平涂）。
    pub material: Option<[f32; 3]>,
}

impl MeshRecord {
    /// 几何自洽性检查（显式失败，不静默丢三角形）。
    pub fn validate(&self) -> Result<(), String> {
        if self.positions.is_empty() {
            return Err("mesh: 顶点为空".to_string());
        }
        if self.triangles.is_empty() || self.triangles.len() % 3 != 0 {
            return Err(format!(
                "mesh: 三角形索引必须是三个一组（实际 {} 个）",
                self.triangles.len()
            ));
        }
        if let Some(index) = self
            .triangles
            .iter()
            .find(|&&index| index as usize >= self.positions.len())
        {
            return Err(format!(
                "mesh: 索引 {index} 超出顶点数 {}",
                self.positions.len()
            ));
        }
        if let Some(uvs) = &self.uvs {
            if uvs.len() != self.positions.len() {
                return Err(format!(
                    "mesh: UV 数 {} ≠ 顶点数 {}",
                    uvs.len(),
                    self.positions.len()
                ));
            }
        }
        if let Some(normals) = &self.normals {
            if normals.len() != self.positions.len() {
                return Err(format!(
                    "mesh: 法线数 {} ≠ 顶点数 {}",
                    normals.len(),
                    self.positions.len()
                ));
            }
        }
        Ok(())
    }
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

/// 一盏灯（B6 Step 1.2）：编码移植自上游 `rendering/feed.py::lighting`，
/// 每盏 16 float：`位置/方向 xyz`、`kind`、`线性色 rgb × 强度`、`reach`、
/// `轴线 xyz`、`scale`、`offset`、`是否投影`、0、0。
///
/// `kind`：0 = 环境光、1 = 平行光、2 = 点光、3 = 聚光、4 = 环境图（与上游一致）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Light {
    Ambient {
        color: [f32; 3],
        intensity: f32,
    },
    Sun {
        /// 光来自的方向（世界坐标；上游会归一化）。
        direction: [f32; 3],
        color: [f32; 3],
        intensity: f32,
        shadows: bool,
    },
    Point {
        at: [f32; 3],
        color: [f32; 3],
        intensity: f32,
        /// 影响半径（0 = 无衰减）。
        reach: f32,
    },
    Spot {
        at: [f32; 3],
        toward: [f32; 3],
        color: [f32; 3],
        intensity: f32,
        /// 半角（弧度）与软化比例，与上游 `angle` / `softness` 同义。
        angle: f32,
        softness: f32,
        shadows: bool,
    },
    /// **B10 Step 2**：环境图（上游 `kind = 4`）。`id` 指向 [`Player`] 里
    /// `add_environment` 上传的那张图；上游把它放在"点光 reach"的槽位（`slot[7]`）。
    ///
    /// AO 的生效条件正是"存在环境光（kind 0）或环境图（kind 4）"（见 `occlusion_of`）。
    Environment {
        id: u32,
    },
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-9);
    [v[0] / n, v[1] / n, v[2] / n]
}

/// sRGB 编码值 → 线性光（上游 `feed.py::linear`）。
fn linear(rgb: [f32; 3]) -> [f32; 3] {
    rgb.map(|c| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    })
}

impl Light {
    fn slot(&self) -> [f32; 16] {
        let mut out = [0.0f32; 16];
        match *self {
            Light::Ambient { color, intensity } => {
                out[3] = 0.0;
                let c = linear(color).map(|c| c * intensity);
                out[4..7].copy_from_slice(&c);
            }
            Light::Sun {
                direction,
                color,
                intensity,
                shadows,
            } => {
                out[0..3].copy_from_slice(&normalize3(direction));
                out[3] = 1.0;
                let c = linear(color).map(|c| c * intensity);
                out[4..7].copy_from_slice(&c);
                out[13] = if shadows { 1.0 } else { 0.0 };
            }
            Light::Point {
                at,
                color,
                intensity,
                reach,
            } => {
                out[0..3].copy_from_slice(&at);
                out[3] = 2.0;
                let c = linear(color).map(|c| c * intensity);
                out[4..7].copy_from_slice(&c);
                out[7] = reach;
            }
            Light::Spot {
                at,
                toward,
                color,
                intensity,
                angle,
                softness,
                shadows,
            } => {
                out[0..3].copy_from_slice(&at);
                out[3] = 3.0;
                let c = linear(color).map(|c| c * intensity);
                out[4..7].copy_from_slice(&c);
                let axis = normalize3([toward[0] - at[0], toward[1] - at[1], toward[2] - at[2]]);
                out[8..11].copy_from_slice(&axis);
                let outer = angle.cos();
                let inner = (angle * (1.0 - softness)).cos();
                let scale = 1.0 / (inner - outer).max(1e-4);
                out[11] = scale;
                out[12] = -outer * scale;
                out[13] = if shadows { 1.0 } else { 0.0 };
            }
            // B10 Step 2：环境图槽位 —— `kind = 4`，id 放在"点光 reach"的位置
            // （上游 `environment_of` 读 `slot[7]`）。
            Light::Environment { id } => {
                out[3] = 4.0;
                out[7] = id as f32;
            }
        }
        out
    }
}

/// 一台 3D 相机视图（B6 Step 1.2）：移植自上游 `feed.py::view` 的 3D 分支。
///
/// `axes` 是**世界 → 相机**的 3×3（行 = 相机右 / 上 / 朝外）。宿主已有 `Camera3D`，
/// 由它算出三个轴即可；本 crate 不做轨道角（phi/theta/gamma）的约定。
#[derive(Debug, Clone, PartialEq)]
pub struct View3 {
    /// 相机画幅（世界单位）。
    pub frame: (f32, f32),
    /// 画幅中心（世界坐标）。
    pub center: [f32; 3],
    /// 世界 → 相机轴（行主序）。
    pub axes: [[f32; 3]; 3],
    pub zoom: f32,
    /// 焦距（眼到画幅的距离）。
    pub focal: f32,
    pub size: (u32, u32),
    pub background: [f32; 4],
    /// 默认光的方向（上游 `camera.light_source`；本入口只在 `lights` 为空时用于记录）。
    pub light_source: [f32; 3],
    pub exposure: f32,
    pub tone: Option<ze_render_ir::ToneCurve>,
    pub bloom: f32,
    /// ⚠️ **AO 依赖环境光**（`B6 §2.4`）：上游 `occlusion_of()` 要求 `lights` 里存在
    /// 环境光（类型 0）或环境图（类型 4）；只有方向光 / 点光 / 聚光时 **静默不生效**。
    pub ao: Option<(f32, f32)>,
    pub lights: Vec<Light>,
}

/// 上游常数：近平面（reversed Z 的基准）与"贴在表面上"的深度偏置。
const NEAR: f32 = 0.05;
const ON_3D: f32 = 1e-6;

fn view_3d(v: &View3) -> Vec<f32> {
    let (hw, hh) = (v.frame.0 * 0.5, v.frame.1 * 0.5);
    let [rx, ry, rz] = v.axes;
    let r = [rx, ry, rz];
    let rc = [0, 1, 2].map(|k| rx[k] * v.center[0] + ry[k] * v.center[1] + rz[k] * v.center[2]);
    let f = v.focal.max(1e-6);
    let zoom = v.zoom;
    // 上游的 projection（行主序，`feed.py:438`），再按 WGSL 需要转置写入。
    let projection = [
        [
            zoom / hw * r[0][0],
            zoom / hw * r[0][1],
            zoom / hw * r[0][2],
            -zoom / hw * rc[0],
        ],
        [
            zoom / hh * r[1][0],
            zoom / hh * r[1][1],
            zoom / hh * r[1][2],
            -zoom / hh * rc[1],
        ],
        [0.0, 0.0, 0.0, NEAR / f],
        [-r[2][0] / f, -r[2][1] / f, -r[2][2] / f, (f + rc[2]) / f],
    ];
    let overlay = [
        [1.0 / hw, 0.0, 0.0, 0.0],
        [0.0, 1.0 / hh, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mut out = vec![0.0f32; VIEW_LENGTH];
    for c in 0..4 {
        for row in 0..4 {
            out[c * 4 + row] = projection[row][c];
            out[16 + c * 4 + row] = overlay[row][c];
        }
    }
    out[32] = v.size.0 as f32;
    out[33] = v.size.1 as f32;
    out[34] = v.size.1 as f32 / v.frame.1;
    out[35] = zoom;
    out[36..39].copy_from_slice(&v.light_source);
    out[39] = 1.0; // three_d
    // `toward` = 相机 z 轴（世界），bias = ON_3D。
    out[40..43].copy_from_slice(&rz);
    out[43] = ON_3D;
    out[44..48].copy_from_slice(&v.background);
    let eye = [
        v.center[0] + f * rz[0],
        v.center[1] + f * rz[1],
        v.center[2] + f * rz[2],
        1.0,
    ];
    out[VIEW_LIGHTING..VIEW_LIGHTING + 4].copy_from_slice(&eye);
    let count = v.lights.len().min(8);
    out[VIEW_LIGHTING + 4] = count as f32;
    out[VIEW_LIGHTING + 5] = v.exposure;
    out[VIEW_LIGHTING + 6] = match v.tone {
        None | Some(ze_render_ir::ToneCurve::Clip) => 0.0,
        Some(ze_render_ir::ToneCurve::AgX) => 1.0,
    };
    out[VIEW_LIGHTING + 7] = v.bloom.clamp(0.0, 1.0);
    for (i, light) in v.lights.iter().take(8).enumerate() {
        let at = VIEW_LIGHTING + 8 + 16 * i;
        out[at..at + 16].copy_from_slice(&light.slot());
    }
    if let Some((intensity, radius)) = v.ao {
        out[VIEW_OCCLUSION] = intensity.max(0.0);
        out[VIEW_OCCLUSION + 1] = radius.max(0.0);
    }
    out
}

/// 用给定 view 渲染一帧（`view` 必须是 `VIEW_LENGTH` 个 float）。
fn render_with_view(
    view: Vec<f32>,
    width: u32,
    height: u32,
    samples: u32,
    records: &[PathRecord],
) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err(format!("canvas {width}x{height} is empty"));
    }
    let view_bytes: Vec<u8> = bytemuck::cast_slice(&view).to_vec();

    let mut player = crate::render::Player::new(width, height, samples);
    let mut built: Vec<Record> = Vec::with_capacity(records.len());
    for (i, record) in records.iter().enumerate() {
        let key = i as u64 + 1;
        match record {
            PathRecord::Fill(path) => {
                let curves = upload_path(&mut player, key, i, &path.points, &path.ranges)?;
                built.push(Record::filled(
                    key,
                    curves,
                    path.model,
                    path.fill,
                    path.shade_in_3d,
                    path.material,
                ));
            }
            PathRecord::Stroke(path) => {
                let curves = upload_path(&mut player, key, i, &path.points, &path.ranges)?;
                built.push(Record::stroked(
                    key,
                    curves,
                    path.model,
                    path.stroke,
                    path.width,
                    path.shape_bits,
                ));
            }
            PathRecord::Mesh(mesh) => {
                mesh.validate()
                    .map_err(|error| format!("record {i}: {error}"))?;
                // 上游要求"每个点一个 (u, v)"：没有 UV 时补零（不改变几何）。
                let zero_uvs;
                let uvs: &[[f64; 2]] = match &mesh.uvs {
                    Some(uvs) => uvs,
                    None => {
                        zero_uvs = vec![[0.0, 0.0]; mesh.positions.len()];
                        &zero_uvs
                    }
                };
                let point_bytes: &[u8] = bytemuck::cast_slice(&mesh.positions);
                let uv_bytes: &[u8] = bytemuck::cast_slice(uvs);
                let normal_bytes: &[u8] = match &mesh.normals {
                    Some(normals) => bytemuck::cast_slice(normals),
                    None => &[],
                };
                let triangle_bytes: &[u8] = bytemuck::cast_slice(&mesh.triangles);
                player.upload(Upload::mesh(
                    key,
                    point_bytes,
                    uv_bytes,
                    normal_bytes,
                    triangle_bytes,
                    0,
                    0,
                )?)?;
                built.push(Record::mesh(
                    key,
                    (mesh.triangles.len() / 3) as u32,
                    mesh.model,
                    mesh.fill,
                    mesh.material,
                ));
            }
        }
    }
    let record_bytes: Vec<u8> = bytemuck::cast_slice(&built).to_vec();

    crate::render::with_gpu(|gpu| {
        let frames = player.views(&view_bytes, &record_bytes, Vec::new())?;
        let mut out = vec![0u8; (width as usize) * (height as usize) * 4];
        player.pixels(gpu, &frames, &mut out)?;
        Ok(out)
    })?
}

/// 上传一条路径并返回曲线数（填充与描边共用）。
fn upload_path(
    player: &mut crate::render::Player,
    key: u64,
    index: usize,
    points: &[[f64; 3]],
    ranges: &[[u32; 4]],
) -> Result<u32, String> {
    if points.len() % 4 != 0 || points.is_empty() {
        return Err(format!("record {index}: control points are not whole curves"));
    }
    player.upload(Upload::path(
        key,
        bytemuck::cast_slice(points),
        bytemuck::cast_slice(ranges),
        centroid_area(points),
    )?)?;
    Ok((points.len() / 4) as u32)
}

/// **3D 路径 / 网格渲染**（B6 Step 1.2 / B10 Step 2）：`View3`（含灯 / tone / bloom / AO）
/// + 混合记录（路径与网格按声明顺序绘制）。
pub fn render_3d(view: &View3, records: &[PathRecord]) -> Result<Vec<u8>, String> {
    render_3d_with(view.size.0, view.size.1, 1, view, records)
}

/// 同 [`render_3d`]，但显式给画布尺寸与**采样数**（`frame_loop` 的 3D 分支用 4，
/// 与 2D 路径同口径；`render_3d` 固定 1 是 B6 遗留的调用约定）。
pub fn render_3d_with(
    width: u32,
    height: u32,
    samples: u32,
    view: &View3,
    records: &[PathRecord],
) -> Result<Vec<u8>, String> {
    render_with_view(view_3d(view), width, height, samples, records)
}

/// 一条绘制记录（保持声明顺序 = 绘制顺序）。
#[derive(Debug, Clone)]
pub enum PathRecord {
    Fill(FilledPath),
    Stroke(StrokedPath),
    /// **B10 Step 2**：3D 网格（可受光）。
    Mesh(MeshRecord),
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

impl From<MeshRecord> for PathRecord {
    fn from(mesh: MeshRecord) -> Self {
        Self::Mesh(mesh)
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
    render_with_view(
        view_2d_with(width, height, background, options),
        width,
        height,
        samples,
        records,
    )
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

    /// **B6 Step 1.2**：3D view 的槽位布局与上游 `feed.py::view` 一致 ——
    /// 投影矩阵（列主序）、`three_d`、`toward`/bias、eye、灯数、tone、bloom、AO、灯槽。
    #[test]
    fn view3_lays_out_the_same_slots_as_the_upstream_packer() {
        let view = View3 {
            frame: (8.0, 6.0),
            center: [0.0, 0.0, 0.0],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            zoom: 1.0,
            focal: 8.0,
            size: (64, 48),
            background: [0.0, 0.0, 0.0, 1.0],
            light_source: [0.0, -1.0, 0.0],
            exposure: 2.0,
            tone: Some(ze_render_ir::ToneCurve::AgX),
            bloom: 0.5,
            ao: Some((0.7, 0.3)),
            lights: vec![
                Light::Sun {
                    direction: [0.0, 0.0, -1.0],
                    color: [1.0, 1.0, 1.0],
                    intensity: 2.0,
                    shadows: true,
                },
                Light::Ambient {
                    color: [1.0, 1.0, 1.0],
                    intensity: 0.1,
                },
            ],
        };
        let v = super::view_3d(&view);
        assert_eq!(v.len(), crate::render::VIEW_LENGTH);
        assert_eq!([v[32], v[33], v[34], v[35]], [64.0, 48.0, 8.0, 1.0]);
        assert_eq!(v[39], 1.0, "three_d");
        assert_eq!([v[40], v[41], v[42]], [0.0, 0.0, 1.0], "toward = 相机 z 轴");
        assert_eq!(v[43], super::ON_3D, "3D 路径的深度偏置");
        assert_eq!([v[48], v[49], v[50], v[51]], [0.0, 0.0, 8.0, 1.0], "eye = center + f·z");
        assert_eq!(v[super::VIEW_LIGHTING + 4], 2.0, "灯数");
        assert_eq!(v[super::VIEW_LIGHTING + 5], 2.0, "曝光");
        assert_eq!(v[super::VIEW_LIGHTING + 6], 1.0, "tone = AgX");
        assert_eq!(v[super::VIEW_LIGHTING + 7], 0.5, "bloom 强度");
        assert_eq!(v[super::VIEW_OCCLUSION], 0.7, "AO 强度");
        assert_eq!(v[super::VIEW_OCCLUSION + 1], 0.3, "AO 半径");
        // 第一盏灯：方向 + kind=1 + 线性色 × 强度 + 投影位。
        let sun = super::VIEW_LIGHTING + 8;
        assert_eq!([v[sun], v[sun + 1], v[sun + 2]], [0.0, 0.0, -1.0]);
        assert_eq!(v[sun + 3], 1.0);
        assert!((v[sun + 4] - 2.0).abs() < 1e-6, "线性白 × 2");
        assert_eq!(v[sun + 13], 1.0, "casts shadows");
        // 第二盏：kind=0（环境光）。
        let ambient = sun + 16;
        assert_eq!(v[ambient + 3], 0.0);
        assert!((v[ambient + 4] - 0.1).abs() < 1e-3);
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

    // ─── B6 Step 1 验收：3D 视图上的着色栈（tone / bloom / AO）真的有像素效果 ────

    /// 固定场景：一盏强方向光 + 环境光 + 一块 ±1 世界单位的**受光**平面路径。
    ///
    /// 为什么必须带 `material`：`plan.lit` 由 `vector.rs:1058` 按 `material[3] > 0` 判定，
    /// 而 bloom / tone 的合成 pass 的入场券 `glow` 要求 `light || plan.lit`
    /// （`render.rs:2182`）—— 没有材质就没有那条 pass，tone 与 bloom 都不会生效。
    fn lit_square() -> FilledPath {
        let mut path = FilledPath::polygon(
            &[
                [-1.0, -1.0, 0.0],
                [1.0, -1.0, 0.0],
                [1.0, 1.0, 0.0],
                [-1.0, 1.0, 0.0],
            ],
            [0.8, 0.8, 0.8, 1.0],
        );
        path.shade_in_3d = true;
        path.material = Some([0.0, 0.15, 1.0]); // 低粗糙、全反射
        path
    }

    fn lit_view(
        tone: Option<ze_render_ir::ToneCurve>,
        bloom: f32,
        ao: Option<(f32, f32)>,
        ambient: bool,
    ) -> View3 {
        let mut lights = vec![Light::Sun {
            direction: [0.0, 0.0, -1.0],
            color: [1.0, 1.0, 1.0],
            intensity: 4.0,
            shadows: false,
        }];
        if ambient {
            lights.push(Light::Ambient {
                color: [1.0, 1.0, 1.0],
                intensity: 1.0,
            });
        }
        View3 {
            frame: (4.0, 4.0),
            center: [0.0, 0.0, 0.0],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            zoom: 1.0,
            focal: 8.0,
            size: (64, 64),
            background: [0.0, 0.0, 0.0, 1.0],
            light_source: [0.0, -1.0, 0.0],
            exposure: 2.0,
            tone,
            bloom,
            ao,
            lights,
        }
    }

    fn lit_frame(
        tone: Option<ze_render_ir::ToneCurve>,
        bloom: f32,
        ao: Option<(f32, f32)>,
        ambient: bool,
    ) -> Vec<u8> {
        let records = [PathRecord::from(lit_square())];
        match render_3d(&lit_view(tone, bloom, ao, ambient), &records) {
            Ok(frame) => frame,
            Err(error) => panic!("矢量 3D 后端起不来：{error}"),
        }
    }

    /// **B6 Step 1 验收（tone）**：有 3D 计划时 `AgX ≠ clip`，且 clip 下高光死白、
    /// AgX 下被压回可分辨区间 —— 这正是 V2-15 记的"2D-only 帧无消费者"的补证。
    #[test]
    fn tone_mapping_changes_a_lit_3d_frame() {
        let clip = lit_frame(Some(ze_render_ir::ToneCurve::Clip), 1.0, None, true);
        let agx = lit_frame(Some(ze_render_ir::ToneCurve::AgX), 1.0, None, true);
        let at = pixel(&clip, 64, 32, 32);
        let bt = pixel(&agx, 64, 32, 32);
        assert_ne!(at, bt, "有 3D 计划时 AgX 必须与 clip 不同：{at:?} vs {bt:?}");
        assert!(
            at[..3].iter().all(|c| *c >= 250),
            "clip 下高光应死白：{at:?}"
        );
        assert!(
            bt[..3].iter().any(|c| *c < 250),
            "AgX 下高光应被压回：{bt:?}"
        );
    }

    /// **B6 Step 1 验收（bloom）**：亮区外溢 —— 方块外 3 像素处的亮度必须上升。
    ///
    /// 方块占 ±1 世界单位、画幅 4 ⇒ 每单位 16 px，边缘在 y = 16（上）/ 48（下）；
    /// 采样点 `(32, 13)` 在方块外 3 px。
    #[test]
    fn bloom_spreads_bright_pixels_outward() {
        let off = lit_frame(Some(ze_render_ir::ToneCurve::AgX), 0.0, None, true);
        let on = lit_frame(Some(ze_render_ir::ToneCurve::AgX), 1.0, None, true);
        let luma = |p: [u8; 4]| p[0] as i32 + p[1] as i32 + p[2] as i32;
        let outside_off = luma(pixel(&off, 64, 32, 13));
        let outside_on = luma(pixel(&on, 64, 32, 13));
        assert!(
            outside_on > outside_off,
            "bloom 应让亮区外溢：off = {outside_off}，on = {outside_on}"
        );
    }

    /// **B10 Step 2 验收（网格）**：三角形网格在 3D 视图里真的画出来，且
    /// `material = Some(_)` 让它走"逐片元受光"，于是 tone 映射（bloom 合成 pass 里的
    /// AgX）真的改像素 —— 这正是 §0.2 那条链（`Kind::Mesh` → material → plan.lit →
    /// glow → bloom/tone）的入口。
    #[test]
    fn mesh_records_draw_and_are_lit() {
        let mesh = |material: Option<[f32; 3]>| MeshRecord {
            positions: vec![
                [-1.0, -1.0, 0.0],
                [1.0, -1.0, 0.0],
                [1.0, 1.0, 0.0],
                [-1.0, 1.0, 0.0],
            ],
            uvs: None,
            normals: None,
            triangles: vec![0, 1, 2, 0, 2, 3],
            fill: [0.8, 0.2, 0.2, 1.0],
            model: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            material,
        };
        let view = |tone: ze_render_ir::ToneCurve| View3 {
            frame: (4.0, 4.0),
            center: [0.0, 0.0, 0.0],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            zoom: 1.0,
            focal: 8.0,
            size: (64, 64),
            background: [0.0, 0.0, 0.0, 1.0],
            light_source: [0.0, 0.0, 1.0],
            exposure: 4.0,
            tone: Some(tone),
            bloom: 1.0,
            ao: None,
            lights: vec![
                // 光来自 +Z（相机在 +Z 侧，看到的是被照亮的一面）。
                Light::Sun {
                    direction: [0.0, 0.0, 1.0],
                    color: [1.0, 1.0, 1.0],
                    intensity: 3.0,
                    shadows: false,
                },
                Light::Ambient {
                    color: [1.0, 1.0, 1.0],
                    intensity: 0.4,
                },
            ],
        };
        let frame = |material: Option<[f32; 3]>, tone: ze_render_ir::ToneCurve| {
            let records = [PathRecord::from(mesh(material))];
            match render_3d(&view(tone), &records) {
                Ok(frame) => frame,
                Err(error) => panic!("矢量 3D 网格起不来：{error}"),
            }
        };

        let lit = frame(Some([0.0, 0.5, 1.0]), ze_render_ir::ToneCurve::AgX);
        let center = pixel(&lit, 64, 32, 32);
        assert!(
            center[..3].iter().any(|c| *c > 16),
            "网格应真的画出来（中央非背景）：{center:?}"
        );

        let clip = frame(Some([0.0, 0.5, 1.0]), ze_render_ir::ToneCurve::Clip);
        assert_ne!(
            pixel(&clip, 64, 32, 32),
            center,
            "有材质的网格 ⇒ plan.lit ⇒ tone（bloom 合成 pass）必须生效：AgX ≠ clip"
        );

        let unlit = frame(None, ze_render_ir::ToneCurve::AgX);
        assert_ne!(
            pixel(&unlit, 64, 32, 32),
            center,
            "material = None 时不应走受光路径（与 Some 必须不同）"
        );
    }

    /// 网格几何自洽性：空顶点 / 非三元组索引 / 越界索引 / UV 数不匹配都显式报错。
    #[test]
    fn mesh_record_validates_its_geometry() {
        let base = MeshRecord {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: None,
            normals: None,
            triangles: vec![0, 1, 2],
            fill: [1.0; 4],
            model: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
            material: None,
        };
        base.validate().expect("合法三角面");

        let mut empty = base.clone();
        empty.positions.clear();
        assert!(empty.validate().is_err());

        let mut partial = base.clone();
        partial.triangles = vec![0, 1];
        assert!(partial.validate().is_err());

        let mut out_of_range = base.clone();
        out_of_range.triangles = vec![0, 1, 9];
        assert!(out_of_range.validate().is_err());

        let mut bad_uv = base.clone();
        bad_uv.uvs = Some(vec![[0.0, 0.0]; 2]);
        assert!(bad_uv.validate().is_err());
    }

    /// **闭合孔洞**：同一路径里的两条子路径（外圈 CCW + 内圈 CW）按 nonzero 规则
    /// 必须留下孔；孔被填满说明填充规则 / 子路径方向在某一层丢了。
    ///
    /// 这是 `vector_paths` 里字母 `o` / `e` 计数器（counter）被填色的最小复现。
    #[test]
    fn concave_ring_keeps_its_hole() {
        let ring = |points: &[[f64; 3]]| {
            let n = points.len();
            let mut curves = Vec::new();
            for i in 0..n {
                let a = points[i];
                let b = points[(i + 1) % n];
                let lerp = |t: f64| [0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * t);
                curves.extend_from_slice(&[a, lerp(1.0 / 3.0), lerp(2.0 / 3.0), b]);
            }
            FilledPath {
                points: curves,
                ranges: vec![[0, n as u32, 1, 0]],
                fill: [1.0, 0.0, 0.0, 1.0],
                model: [
                    [1.0, 0.0, 0.0, 0.0],
                    [0.0, 1.0, 0.0, 0.0],
                    [0.0, 0.0, 1.0, 0.0],
                ],
                shade_in_3d: false,
                material: None,
            }
        };
        // 外圈 CCW、内圈 CW（世界 y 向上）。
        // 2D 视图里 1 世界单位 = 1 px（`projection = 1/(size/2)`）⇒ 外圈 ±16、内圈 ±8。
        let mut merged = ring(&[
            [-16.0, -16.0, 0.0],
            [16.0, -16.0, 0.0],
            [16.0, 16.0, 0.0],
            [-16.0, 16.0, 0.0],
        ]);
        let inner = ring(&[
            [-8.0, -8.0, 0.0],
            [-8.0, 8.0, 0.0],
            [8.0, 8.0, 0.0],
            [8.0, -8.0, 0.0],
        ]);
        let offset = (merged.points.len() / 4) as u32;
        merged.points.extend_from_slice(&inner.points);
        merged.ranges.extend(
            inner
                .ranges
                .iter()
                .map(|r| [r[0] + offset, r[1] + offset, r[2], r[3]]),
        );

        let frame = match render_2d(64, 64, 1, [0.0, 0.0, 0.0, 1.0], &[merged.into()]) {
            Ok(frame) => frame,
            Err(error) => panic!("矢量后端起不来：{error}"),
        };
        let center = pixel(&frame, 64, 32, 32);
        // 环带中点：(32, 20) 在外圈 16 与内圈 24 之间。
        let edge = pixel(&frame, 64, 32, 20);
        assert!(edge[0] > 200, "环带应被填充：{edge:?}");
        assert!(center[0] < 32, "孔洞必须保持背景色：{center:?}");
    }

    /// **B6 Step 1 验收（AO）**：AO 是屏幕空间的深度遮挡，**单块平面没有凹陷 ⇒ 不会变暗**
    /// （这是正确行为，不是没接上）。所以用"墙 + 地板"的内角来验：有环境光时内角变暗；
    /// 只有方向光时 AO **无效**（上游 `occlusion_of()` 的结构）。
    #[test]
    fn ambient_occlusion_requires_ambient_light() {
        // 墙（z = −1）与地板（y = −1）在 y = −1, z = −1 处形成内角。
        let corner = || {
            let mut wall = FilledPath::polygon(
                &[
                    [-1.0, -1.0, -1.0],
                    [1.0, -1.0, -1.0],
                    [1.0, 1.0, -1.0],
                    [-1.0, 1.0, -1.0],
                ],
                [0.8, 0.8, 0.8, 1.0],
            );
            let mut floor = FilledPath::polygon(
                &[
                    [-1.0, -1.0, -1.0],
                    [1.0, -1.0, -1.0],
                    [1.0, -1.0, 0.0],
                    [-1.0, -1.0, 0.0],
                ],
                [0.8, 0.8, 0.8, 1.0],
            );
            for path in [&mut wall, &mut floor] {
                path.shade_in_3d = true;
                path.material = Some([0.0, 0.9, 0.0]); // 漫反射为主
            }
            vec![PathRecord::from(wall), PathRecord::from(floor)]
        };
        let frame = |ao: Option<(f32, f32)>, ambient: bool| {
            let mut view = lit_view(Some(ze_render_ir::ToneCurve::AgX), 0.0, ao, ambient);
            // 只看环境光：AO 乘的正是环境光项（方向光不受 AO 影响）。
            view.lights.retain(|light| {
                if ambient {
                    true
                } else {
                    matches!(light, Light::Sun { .. })
                }
            });
            match render_3d(&view, &corner()) {
                Ok(frame) => frame,
                Err(error) => panic!("矢量 3D 后端起不来：{error}"),
            }
        };
        let luma_sum = |frame: &[u8]| frame.chunks_exact(4).map(|p| p[0] as i64 + p[1] as i64 + p[2] as i64).sum::<i64>();

        let plain = frame(None, true);
        let occluded = frame(Some((1.0, 0.3)), true);
        assert_ne!(plain, occluded, "有环境光时 AO 必须改变像素");
        let (before, after) = (luma_sum(&plain), luma_sum(&occluded));
        assert!(
            after < before,
            "内角应被 AO 压暗：before = {before}，after = {after}"
        );

        let sun_only = frame(None, false);
        let sun_only_occluded = frame(Some((1.0, 0.3)), false);
        assert_eq!(
            sun_only, sun_only_occluded,
            "只有方向光时 AO 不生效（上游 `occlusion_of` 依赖环境光）；若这里开始不同，请更新 V2-15"
        );
    }
}
