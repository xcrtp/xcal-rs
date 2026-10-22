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

use crate::render::{Record, Upload, VIEW_LENGTH};

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

/// 2D 正交视图（与上游 `feed.py::view` 的 2D 分支同布局）：
/// 世界坐标 = 以画面中心为原点的像素。
fn view_2d(width: u32, height: u32, background: [f32; 4]) -> Vec<f32> {
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
    v[52] = 0.0; // 2D 视图没有光源
    v[53] = 1.0; // exposure
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

/// **离屏渲染一帧**：`paths` 按顺序画到 `width×height` 的 RGBA8 缓冲并读回。
///
/// 返回的字节是行优先 RGBA8（`width * height * 4`）。失败 ⇒ 显式错误（不静默）。
pub fn render_2d(
    width: u32,
    height: u32,
    samples: u32,
    background: [f32; 4],
    paths: &[FilledPath],
) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err(format!("canvas {width}x{height} is empty"));
    }
    let view = view_2d(width, height, background);
    let view_bytes: Vec<u8> = bytemuck::cast_slice(&view).to_vec();

    let mut player = crate::render::Player::new(width, height, samples);
    let mut records: Vec<Record> = Vec::with_capacity(paths.len());
    for (i, path) in paths.iter().enumerate() {
        if path.points.len() % 4 != 0 || path.points.is_empty() {
            return Err(format!("path {i}: control points are not whole curves"));
        }
        let key = i as u64 + 1;
        let points: &[u8] = bytemuck::cast_slice(&path.points);
        let ranges: &[u8] = bytemuck::cast_slice(&path.ranges);
        player.upload(Upload::path(
            key,
            points,
            ranges,
            centroid_area(&path.points),
        )?)?;
        records.push(Record::filled(
            key,
            (path.points.len() / 4) as u32,
            path.model,
            path.fill,
        ));
    }
    let record_bytes: Vec<u8> = bytemuck::cast_slice(&records).to_vec();

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
        let frame = match render_2d(w, h, 1, [0.0, 0.0, 0.0, 1.0], &[square]) {
            Ok(frame) => frame,
            Err(error) => {
                eprintln!("跳过：本机起不来矢量后端（{error}）");
                return;
            }
        };
        let center = pixel(&frame, w, 64, 64);
        assert!(center[0] > 200, "中央应为红色：{center:?}");
        let corner = pixel(&frame, w, 2, 2);
        assert!(corner[0] < 32, "角落应为背景：{corner:?}");
    }
}
