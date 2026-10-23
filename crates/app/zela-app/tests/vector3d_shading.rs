//! **B10 Step 4 验收**：着色栈第一次出现在**作品**画面上，并且逐项可判读。
//!
//! 判据（计划 §6.2，不得只靠"没报错"）：
//! - tone：AgX ≠ clip（同一曝光、同一 bloom）；
//! - bloom：关掉后整体亮度下降（亮区外溢消失）；
//! - AO：关掉后内角变亮（AO 压暗了环境项）。
//!
//! 同一份脚本产物、同一个帧循环（`zela_app::frame_loop`），只切换 `vector_params` ——
//! 差异因此只能来自着色栈本身。`zela-render` / `zela-verify` 恒传 `None`（后端默认）。

use std::path::PathBuf;

use zela_app::frame_loop::{FrameLoopOptions, LoopBackend, run_frames};
use zela_app::{ScriptLanguage, build_script_runtime};
use zela_render_backends::{HeadlessTarget, RenderContext, StandardBackends};
use zela_scene_bundle::RenderParams;

const SIZE: (u32, u32) = (160, 120);

fn script_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scripts/shaded_meshes.lua")
}

/// 用给定的着色栈参数渲一帧（一帧作品：`duration = 0` ⇒ `frames_for` = 1）。
fn render(
    ctx: &RenderContext,
    params: Option<RenderParams>,
) -> Vec<u8> {
    let source = std::fs::read_to_string(script_path()).expect("读取 shaded_meshes.lua");
    let mut runtime = build_script_runtime(ctx.clone(), source, ScriptLanguage::Lua)
        .expect("3D 作品装配（含 3D 相机与网格）");
    let mut backends = StandardBackends::new(ctx);
    let mut target = HeadlessTarget::new();
    let mut frame: Vec<u8> = Vec::new();
    let _run = run_frames(
        ctx,
        &mut runtime,
        &mut backends,
        &mut target,
        FrameLoopOptions {
            width: SIZE.0,
            height: SIZE.1,
            fps: 30.0,
            backend: LoopBackend::Vector,
            vector_params: params,
        },
        true,
        &mut |_index, view| {
            frame = view.rgba.to_vec();
            Ok(std::time::Duration::ZERO)
        },
    )
    .expect("矢量 3D 帧渲染成功");
    frame
}

fn tone(exposure: f32, curve: ze_render_ir::ToneCurve) -> ze_render_ir::ToneMapping {
    ze_render_ir::ToneMapping { exposure, curve }
}

fn params_with(
    exposure: f32,
    curve: ze_render_ir::ToneCurve,
    bloom: bool,
    ao: bool,
) -> RenderParams {
    let mut params = RenderParams::default();
    params.tone = Some(tone(exposure, curve));
    if bloom {
        params.bloom = Some(ze_render_ir::Bloom::default());
    }
    if ao {
        params.ao = Some(ze_render_ir::AmbientOcclusion::default());
    }
    params
}

fn luma_sum(frame: &[u8]) -> i64 {
    frame
        .chunks_exact(4)
        .map(|p| p[0] as i64 + p[1] as i64 + p[2] as i64)
        .sum()
}

fn changed(a: &[u8], b: &[u8]) -> usize {
    a.chunks_exact(4)
        .zip(b.chunks_exact(4))
        .filter(|(x, y)| x != y)
        .count()
}

#[test]
fn shading_stack_changes_pixels_on_a_mesh_work() {
    let ctx = RenderContext::headless().expect("headless GPU");

    // 0) 后端默认（`zela-render --backend vector` 的用法）：tone/bloom/ao 全开。
    let default = render(&ctx, None);
    let drawn = default
        .chunks_exact(4)
        .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 0)
        .count();
    assert!(drawn > 2000, "3D 网格应真的画出来（非背景像素 {drawn}）");

    // 1) tone：同一曝光 + 同一 bloom，只换曲线 ⇒ 必须不同。
    let agx = render(&ctx, Some(params_with(4.0, ze_render_ir::ToneCurve::AgX, true, true)));
    let clip = render(&ctx, Some(params_with(4.0, ze_render_ir::ToneCurve::Clip, true, true)));
    assert!(
        changed(&agx, &clip) > 2000,
        "AgX 与 clip 必须产生可判读的差异（实际 {} 像素）",
        changed(&agx, &clip)
    );

    // 2) bloom：关掉后整体变暗（亮区外溢消失）。
    let no_bloom = render(&ctx, Some(params_with(4.0, ze_render_ir::ToneCurve::AgX, false, true)));
    assert!(
        luma_sum(&no_bloom) < luma_sum(&agx),
        "有 bloom 的画面应更亮：with = {}，without = {}",
        luma_sum(&agx),
        luma_sum(&no_bloom)
    );

    // 3) AO：关掉后内角变亮（AO 乘的是环境项）。
    let no_ao = render(&ctx, Some(params_with(4.0, ze_render_ir::ToneCurve::AgX, true, false)));
    assert!(
        luma_sum(&no_ao) > luma_sum(&agx),
        "开 AO 后画面应更暗：with = {}，without = {}",
        luma_sum(&agx),
        luma_sum(&no_ao)
    );
}
