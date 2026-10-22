// SPDX-FileCopyrightText: 2026 Academa, Inc.
// SPDX-License-Identifier: MIT
// Modified by Zela: B4 Step 6 —— 双后端回归：同一填充形状分别走 wgpu 三角化后端与
// 矢量后端，按**容差**比较（解析 AA 与 MSAA 本来就不同，不追逐位相同）。

//! 双后端回归：wgpu（三角化 + MSAA 4x）对 矢量（解析 AA）。
//!
//! 判据（B4 计划 §6.1，**不断言像素相同**）：
//! 1. 两条路都产出非空帧；
//! 2. 覆盖率（非背景像素占比）相对差 ≤ 10%；
//! 3. 逐像素 RGB 平均绝对误差 ≤ 24/255（边缘抗锯齿方式不同，取宽容差）。
//!
//! 两个用例：轴对齐矩形（边缘落在像素边界上，两边应逐位一致）与斜边菱形
//! （边缘落在像素内部，解析 AA 与 MSAA 必然不同 —— 容差在这里才有意义）。
//! 两个后端在**同一进程**内同时挂载，顺带验证 §四 的注册机制。

use ze_notion::{Color32, Path2D, Rect2D, Vec2};
use ze_render_ir::{FillSolid, LayerBlend, LayerId, ZIndex};
use ze_scene_core::{Camera2D, LayerKind, Path};
use zela_render::{DrawQueue, DrawQueue3D, LightQueue, PostFilterChain, SceneLayer};
use zela_render_backends::{
    SceneCompositorCallback, ScreenParams, StandardBackends, render_frame_headless,
};
use zela_render_vector::FilledPath;

const SIZE: u32 = 128;

fn coverage(frame: &[u8]) -> usize {
    frame
        .chunks_exact(4)
        .filter(|p| p[0] > 24 || p[1] > 24 || p[2] > 24)
        .count()
}

/// headless 目标默认 `Bgra8UnormSrgb` ⇒ 回读是 BGRA；矢量后端直接给 RGBA。
fn bgra_to_rgba(frame: &[u8]) -> Vec<u8> {
    let mut out = frame.to_vec();
    for px in out.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    out
}

/// 同一形状分别交给两个后端，返回（按 RGBA 排好的）两帧。
fn render_both(
    fill_2d: impl FnOnce(&mut DrawQueue),
    vector_paths: &[FilledPath],
) -> (Vec<u8>, Vec<u8>) {
    let Some((_gpu, ctx)) = zela_render_backends::gpu_test::require_gpu() else {
        panic!("本机起不来 GPU（对拍需要真实设备）");
    };

    let mut queue = DrawQueue::new();
    fill_2d(&mut queue);
    let layers = vec![SceneLayer {
        id: LayerId(0),
        kind: LayerKind::Viewport2D {
            camera: Camera2D::new(
                Vec2::new(64.0, 64.0),
                1.0,
                Rect2D::from_min_size(Vec2::ZERO, Vec2::splat(SIZE as f32)),
            ),
        },
        blend: LayerBlend::Alpha,
        opacity: 1.0,
    }];
    let callback = SceneCompositorCallback::new(
        Rect2D::from_min_size(Vec2::ZERO, Vec2::splat(SIZE as f32)),
        layers,
        &queue,
        &DrawQueue3D::new(),
        &LightQueue::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        PostFilterChain::default(),
    );
    let mut backends = StandardBackends::new(&ctx);
    let wgpu = bgra_to_rgba(&render_frame_headless(
        &ctx,
        &mut backends,
        &callback,
        ScreenParams::new([SIZE, SIZE], 1.0),
        wgpu::Color::BLACK,
    ));

    let vector = zela_render_vector::render_2d(SIZE, SIZE, 1, [0.0, 0.0, 0.0, 1.0], vector_paths)
        .expect("矢量后端渲染失败");
    (wgpu, vector)
}

/// 三条判据；返回（覆盖率相对差，MAE）。
fn assert_within_tolerance(wgpu: &[u8], vector: &[u8]) -> (f64, f64) {
    // ① 两条路都非空。
    let (a, b) = (coverage(wgpu), coverage(vector));
    assert!(
        a > 0 && b > 0,
        "两条路都必须画出内容（wgpu={a} 像素，vector={b} 像素）"
    );

    // ② 覆盖率相对差 ≤ 10%。
    let relative = (a as f64 - b as f64).abs() / a.max(b) as f64;
    assert!(
        relative <= 0.10,
        "覆盖率差异超阈值：wgpu={a} vector={b}（相对差 {relative:.3}）"
    );

    // ③ 逐像素 RGB 平均绝对误差 ≤ 24/255。
    let mae: f64 = wgpu
        .chunks_exact(4)
        .zip(vector.chunks_exact(4))
        .map(|(p, q)| (0..3).map(|k| (p[k] as f64 - q[k] as f64).abs()).sum::<f64>() / 3.0)
        .sum::<f64>()
        / (SIZE as f64 * SIZE as f64);
    assert!(mae <= 24.0, "逐像素 MAE 超阈值：{mae:.2}/255");

    eprintln!("双后端对拍：wgpu={a} 像素 / vector={b} 像素（相对差 {relative:.3}），MAE={mae:.2}/255");
    (relative, mae)
}

/// 轴对齐矩形：边缘落在像素边界上 ⇒ 两边应当逐位一致。
#[test]
fn vector_backend_matches_wgpu_on_an_axis_aligned_rect() {
    let (wgpu, vector) = render_both(
        |queue| {
            queue.rect_fill(
                ZIndex::new(0),
                Rect2D::new(Vec2::new(32.0, 32.0), Vec2::new(96.0, 96.0)),
                FillSolid::new(Color32::RED),
            );
        },
        &[FilledPath::polygon(
            &[
                [-32.0, -32.0, 0.0],
                [32.0, -32.0, 0.0],
                [32.0, 32.0, 0.0],
                [-32.0, 32.0, 0.0],
            ],
            [1.0, 0.0, 0.0, 1.0],
        )],
    );
    let (relative, mae) = assert_within_tolerance(&wgpu, &vector);
    assert!(relative < 1e-9 && mae < 1e-9, "轴对齐矩形应逐位一致：相对差 {relative:.6}，MAE {mae:.6}");
}

/// 斜边三角形：边缘以非 45° 斜率穿过像素内部 ⇒ 解析 AA 与 MSAA 必然不同，
/// 容差判据在这里生效（45° 斜边恰好切在像素角上，两边会逐位相同，测不出差异）。
#[test]
fn vector_backend_matches_wgpu_on_a_diagonal_edge() {
    let triangle = [
        Vec2::new(64.0, 24.0),
        Vec2::new(104.0, 96.0),
        Vec2::new(24.0, 96.0),
    ];
    let (wgpu, vector) = render_both(
        |queue| {
            queue.path_fill(
                ZIndex::new(0),
                Path::new(Path2D::from_polyline(&triangle, true)),
                FillSolid::new(Color32::RED),
            );
        },
        &[FilledPath::polygon(
            &[
                [0.0, -40.0, 0.0],
                [40.0, 32.0, 0.0],
                [-40.0, 32.0, 0.0],
            ],
            [1.0, 0.0, 0.0, 1.0],
        )],
    );
    let (relative, mae) = assert_within_tolerance(&wgpu, &vector);
    assert!(relative > 0.0 || mae > 0.0, "斜边应体现抗锯齿差异（而非逐位相同）");
}
