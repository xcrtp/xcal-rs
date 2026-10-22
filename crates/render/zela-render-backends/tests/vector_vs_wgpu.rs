// SPDX-FileCopyrightText: 2026 Academa, Inc.
// SPDX-License-Identifier: MIT
// Modified by Zela: B4 Step 6 —— 双后端回归：同一填充形状分别走 wgpu 三角化后端与
// 矢量后端，按**容差**比较（解析 AA 与 MSAA 本来就不同，不追逐位相同）。

//! 双后端回归：wgpu（三角化 + MSAA 4x）对 矢量（解析 AA）。
//!
//! 判据（B4 计划 §6.1，**不断言像素相同**）：
//! 1. 两条路都产出非空帧；
//! 2. 覆盖率（非背景像素占比）相对差 ≤ 10%；
//! 3. 逐像素 RGB 平均绝对误差 ≤ 8/255（全图；实测 0.09，阈值放宽 2 个量级已足够），
//!    且**内部区** MAE ≤ 2/255（内部区只该受色彩管线影响，不该受 AA 影响）。
//!
//! 两个用例：轴对齐矩形（边缘落在像素边界上，两边应逐位一致）与斜边三角形
//! （边缘落在像素内部，解析 AA 与 MSAA 必然不同 —— 容差在这里才有意义）。
//!
//! **本对拍不经过 `BackendRegistry`**（`BackendKind::Vector` 目前还不可用）：
//! 两个后端只是**在同一进程**内各渲一帧。注册 / 协商机制由 `registry.rs` 的单测
//! 与 `BackendRegistry` 的测试覆盖，不由本文件覆盖。

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

    let vector = zela_render_vector::render_2d(
        SIZE,
        SIZE,
        1,
        [0.0, 0.0, 0.0, 1.0],
        &vector_paths
            .iter()
            .cloned()
            .map(zela_render_vector::PathRecord::from)
            .collect::<Vec<_>>(),
    )
    .expect("矢量后端渲染失败");
    (wgpu, vector)
}

/// 三条判据；返回（覆盖率相对差，全图 MAE）。
///
/// **P1-1 诊断**：MAE 同时拆成「内部区」与「边缘带」两组并打印。
/// 内部区 MAE ≈ 0 才说明差异**只**来自抗锯齿方式；若内部区也显著 > 0，
/// 说明存在系统性色彩管线差异（gamma / 预乘 / sRGB），那是另一种 bug。
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

    // ③ 逐像素 RGB 平均绝对误差 ≤ 8/255。
    let mae_at = |index: usize| -> f64 {
        let p = &wgpu[index * 4..index * 4 + 4];
        let q = &vector[index * 4..index * 4 + 4];
        (0..3).map(|k| (p[k] as f64 - q[k] as f64).abs()).sum::<f64>() / 3.0
    };
    let total = (SIZE * SIZE) as usize;
    let mae: f64 = (0..total).map(mae_at).sum::<f64>() / total as f64;
    assert!(mae <= 8.0, "逐像素 MAE 超阈值：{mae:.2}/255");

    // ── 拆分：以**矢量帧**的形状掩码定性（红 > 128 视为被覆盖）──
    let covered = |frame: &[u8], index: usize| frame[index * 4] > 128;
    let index_of = |x: u32, y: u32| (y * SIZE + x) as usize;
    let mut interior = (0usize, 0.0f64);
    let mut edge = (0usize, 0.0f64);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let i = index_of(x, y);
            if !covered(vector, i) {
                continue;
            }
            // 4 邻域里只要有一个未被覆盖 ⇒ 边缘带；否则算内部区。
            let is_edge = [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)]
                .into_iter()
                .filter(|&(nx, ny)| nx < SIZE && ny < SIZE)
                .any(|(nx, ny)| !covered(vector, index_of(nx, ny)));
            let bucket = if is_edge { &mut edge } else { &mut interior };
            bucket.0 += 1;
            bucket.1 += mae_at(i);
        }
    }
    let mean = |(count, sum): (usize, f64)| if count == 0 { 0.0 } else { sum / count as f64 };
    let (interior_mae, edge_mae) = (mean(interior), mean(edge));
    eprintln!(
        "双后端对拍：wgpu={a} 像素 / vector={b} 像素（相对差 {relative:.3}），\
         MAE={mae:.2}/255（内部区 {interior_mae:.2}/255 / {} px，边缘带 {edge_mae:.2}/255 / {} px）",
        interior.0, edge.0
    );

    // P1-1 判据：内部区必须**基本一致**。若这里变大，先查色彩管线（不是 AA）。
    assert!(
        interior_mae <= 2.0,
        "内部区 MAE 超阈值（说明存在系统性色彩管线差异，不是抗锯齿）：{interior_mae:.2}/255，{} px",
        interior.0
    );
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

/// **场景适配回归（V2-06）**：真实 `DrawQueue`（矩形 + 圆，两种图元）走
/// `snapshot_queue → SceneItem → fill_paths` 的映射路径，与 wgpu 同一帧按容差比较。
/// 这比手搓 `FilledPath` 更接近"作品级别"的每帧对拍。
#[test]
fn scene_mapping_matches_wgpu_on_a_filled_frame() {
    let mut queue = DrawQueue::new();
    queue.rect_fill(
        ZIndex::new(0),
        Rect2D::new(Vec2::new(32.0, 32.0), Vec2::new(96.0, 96.0)),
        FillSolid::new(Color32::RED),
    );
    queue.circle_fill(
        ZIndex::new(1),
        ze_scene_core::Circle::new(Vec2::new(64.0, 16.0), 12.0),
        FillSolid::new(Color32::RED),
    );
    let snapshot = zela_render_backends::snapshot_queue(&queue);
    let items: Vec<zela_render_vector::SceneItem<'_>> = snapshot
        .iter()
        .map(|recorded| zela_render_vector::SceneItem {
            shape: &recorded.shape,
            transform: recorded.transform,
            style: recorded.style,
        })
        .collect();

    let Some((_gpu, ctx)) = zela_render_backends::gpu_test::require_gpu() else {
        panic!("本机起不来 GPU（对拍需要真实设备）");
    };
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
    let vector = zela_render_vector::render_scene_2d(SIZE, SIZE, 1, [0.0, 0.0, 0.0, 1.0], &items)
        .expect("场景映射 / 矢量渲染失败");

    let (relative, mae) = assert_within_tolerance(&wgpu, &vector);
    assert!(relative < 0.02, "覆盖率相对差应很小：{relative:.4}");
    assert!(mae < 2.0, "整帧 MAE 应很小：{mae:.2}/255");
}

/// **描边容差回归（B6 Step 2.1 / 4）**：同一闭合路径两侧都只描边（宽 4px，
/// butt + miter），按内部区 / 边缘带 / 覆盖率判定。两侧描边的细分方式不同
/// （lyon 三角化 vs 解析覆盖率），差异必须存在但很小。
#[test]
fn stroke_matches_wgpu_on_a_closed_polyline() {
    // 非对称、非轴对齐（P1-1 教训：对称形状会掩盖方向 / AA 差异）。
    let points = [
        Vec2::new(40.0, 30.0),
        Vec2::new(96.0, 70.0),
        Vec2::new(36.0, 96.0),
    ];
    let mut queue = DrawQueue::new();
    queue.path_stroke(
        ZIndex::new(0),
        Path::new(Path2D::from_polyline(&points, true)),
        ze_render_ir::StrokeSolid::new(4.0, Color32::RED),
    );
    let snapshot = zela_render_backends::snapshot_queue(&queue);
    let items: Vec<zela_render_vector::SceneItem<'_>> = snapshot
        .iter()
        .map(|recorded| zela_render_vector::SceneItem {
            shape: &recorded.shape,
            transform: recorded.transform,
            style: recorded.style,
        })
        .collect();

    let Some((_gpu, ctx)) = zela_render_backends::gpu_test::require_gpu() else {
        panic!("本机起不来 GPU（对拍需要真实设备）");
    };
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
    let vector = zela_render_vector::render_scene_2d(SIZE, SIZE, 1, [0.0, 0.0, 0.0, 1.0], &items)
        .expect("描边映射 / 矢量渲染失败");

    let (relative, mae) = assert_within_tolerance(&wgpu, &vector);
    // 差异必须存在（两侧不是同一实现），但必须在容差内。
    assert!(
        relative > 0.0 || mae > 0.0,
        "描边两侧不应逐位相同（否则测不出 AA 差异）"
    );
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
        // 注意坐标系：`DrawQueue` 用**屏幕坐标（y 向下）**，矢量后端的 2D 视图
        // 用**世界坐标（y 向上）** ⇒ 这里必须把 y 取负，否则比的是镜像形状
        // （P1-1 拆分出的"内部区 MAE"会立刻暴露这一点）。
        &[FilledPath::polygon(
            &[
                [0.0, 40.0, 0.0],
                [40.0, -32.0, 0.0],
                [-40.0, -32.0, 0.0],
            ],
            [1.0, 0.0, 0.0, 1.0],
        )],
    );
    let (relative, mae) = assert_within_tolerance(&wgpu, &vector);
    assert!(relative > 0.0 || mae > 0.0, "斜边应体现抗锯齿差异（而非逐位相同）");
}
