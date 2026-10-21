// demo3d.rs — 启动即见的 3D 场景（原住在 app 的 `build_runtime`）。
//
// 内容：白色线框立方体 + 参数曲面（引用内容纹理）+ 3D 相机 + 内容纹理公式。
// 归位到场景层后，app 只负责"装配 / 注册 / 找相机"，不再声明场景内容。

use std::sync::Arc;

use ze_notion::{Rect2D, SurfaceFn};
use ze_scene_core::domain::content_texture_handle;
use ze_scene_core::{
    CONTENT_TEXTURE_HEIGHT, CONTENT_TEXTURE_WIDTH, Camera3D, Camera3DHandle, Path, Projection,
    Surface, SurfaceRenderer, SurfaceTexture, TextureFilter, Transform,
};
use ze_prerender::InMemoryEngine;
use ze_render_ir::StrokeSolid;

use zela_scene_ecs::content;
use zela_scene_ecs::contract::{object_id_of, Parent};
use zela_scene_ecs::domain::CameraMode;
use zela_scene_ecs::scene::Scene;
use zela_scene_ecs::scene::components::Material3D;
use zela_scene_ecs::scene::primitives::{parametric_surface, wireframe_cube};
use zela_scene_ecs::types::Color32;

/// 3D 场景装配产物。
pub struct Demo3D {
    /// 3D 相机句柄（自动轨道绑定由 app 装配）。
    pub camera3d: Camera3DHandle,
}

/// 把 3D demo 与内容纹理公式装配进场景。
pub fn build(scene: &mut Scene, engine: &InMemoryEngine) -> Demo3D {
    // ── 3D 场景：白色线框立方体 ──────────────────────────────
    // 几何以 `Arc` 持有：SceneRecord3DSystem 直接复用引用（Arc::clone），
    // 避免「先深拷贝再包 Arc」的每帧无效开销。
    scene.spawn((
        Arc::new(wireframe_cube(1.0)),
        Material3D::color(Color32::WHITE),
    ));

    // ── 公式接入（内容纹理源）：Typst 内容管线 → 2D 描边路径 ──────────
    // B3 Step 4c-4：内容纹理不再走硬编码常量，改由 Surface(Renderer2D) 表达。
    // 先烘焙公式并求包围盒（Surface.bounds），再 spawn Surface 与内容实体。
    let mut outlines = match content::build_ir(engine, "x^2 + 1") {
        Some(ir) => ze_content::stroke_outlines(&ir, 1.3, ze_notion::Vec2::new(-2.8, 3.0), false),
        None => {
            eprintln!("[zela-scene] 公式编译失败，跳过公式实体");
            Vec::new()
        }
    };
    let bounds = outlines
        .iter_mut()
        .map(|outline| outline.path.bounds())
        .reduce(|acc, b| acc.union(&b))
        .unwrap_or_else(|| Rect2D::new(ze_notion::Vec2::new(-1.0, -1.0), ze_notion::Vec2::new(1.0, 1.0)));

    let surface_entity = scene.spawn((
        Surface::new(bounds, ze_scene_core::SurfaceMapping::World, ze_notion::Vec2::new(0.5, 0.5)),
        SurfaceRenderer::Renderer2D,
        // Fixed 尺寸与 `CONTENT_TEXTURE_*` 同值；fit_margin 0.8 复刻 demo3d 原取景
        // （`camera_for` 直接调用 `Camera2D::fit_bounds`，逐位一致）。
        SurfaceTexture::new(
            [CONTENT_TEXTURE_WIDTH, CONTENT_TEXTURE_HEIGHT],
            TextureFilter::Linear,
            false,
        )
        .with_fit_margin(0.8),
    ));
    let surface_id = object_id_of(surface_entity);
    let content_texture = content_texture_handle(surface_id);

    // ── 3D 场景：参数曲面 z = 0.35·sin(3x)·cos(3y)，域 [-1.5, 1.5]² ──
    // 曲面自带真实法线，经 cube.wgsl 的头顶 Lambert 呈现明暗起伏（非纯色剪影）。
    // 平移到立方体侧下方（部分重叠），以体现深度测试决定的前后遮挡。
    let surface = parametric_surface(
        SurfaceFn::new((-1.5, 1.5), (-1.5, 1.5), |x, y| {
            0.35 * (3.0 * x).sin() * (3.0 * y).cos()
        }),
        48,
        48,
    );
    // S3-3：曲面材质引用内容纹理（同一 TextureHandle）。
    // 着色式为**墨迹 over 已受光材质色**（见 `cube.wgsl` F2）：
    // `rgb = tex.rgb + color.rgb * lit * (1 - tex.a)`。内容纹理透明底 ⇒ 未覆盖像素
    // 退化为 `材质色 × lit`（蓝色保留明暗起伏）；墨迹恒为纯白、不被光照压暗。
    let mut surface_material = Material3D::color(Color32::new(120, 200, 255, 255));
    // 材质引用由 id 链派生（`ObjectId → TextureHandle`，与内容层 LayerId 同一映射）。
    surface_material.texture = Some(content_texture);
    scene.spawn((
        Arc::new(surface),
        surface_material,
        Transform {
            position: ze_notion::Vec3::new(0.0, -1.35, 0.0),
            ..Default::default()
        },
    ));

    // 3D 相机（eye 由 BindingApplySystem 驱动，旧 OrbitAnim 组件已移除）。
    //
    // target（注视点）取"立方体 + 曲面"合并包围盒的中心，而非世界原点：曲面整体
    // 平移到 y = -1.35（见上），其几何 y ∈ [-2.85, 0.15]，与立方体 y ∈ [-1, 1] 合并后
    // 的中心是 y ≈ -0.9。若仍注视原点，视锥会整体上移，曲面下缘（y = -2.85）会贴着
    // 视口最底部。注视合并中心即可让两件物体在竖直方向居中。
    //
    // 注：eye 由 Orbit 绑定直接写入（半径 5、绕原点、eye.y ≡ 0），改变 target 只影响
    // 朝向，不影响该旋转轨道。
    const SCENE_LOOK_AT: ze_notion::Vec3 = ze_notion::Vec3::new(0.0, -0.9, 0.0);
    // K0/K2：3D 相机的交互模式是**组件**；这里取 `Path`（动画驱动）——
    // 与改动前逐位一致（`eye` 由下面的 `Curve::Orbit` 绑定驱动、不接受交互命令）。
    // `CameraDriveSystem` 会在用户把"视口操作"切到 3D 时把它改成 `Orbit` / `Fps`，
    // 并让自动轨道绑定**让位**（否则每帧覆盖 `eye`，交互看不见效果）。
    let camera3d_entity = scene.spawn((
        Camera3D::new(
            ze_notion::Vec3::new(0.0, 0.0, 5.0),
            SCENE_LOOK_AT,
            ze_notion::Vec3::Y,
            Projection::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0),
        ),
        CameraMode::PATH,
    ));
    let camera3d = Camera3DHandle(object_id_of(camera3d_entity));

    // ── 公式实体：挂在 Surface 之下（归属由 Parent 链解析）──────────
    // 零 System 改动：公式作为普通 `(Path, StrokeSolid)` 对象参与既有录制。
    for outline in outlines.drain(..) {
        scene.spawn((
            Path::new(outline.path),
            // 公式描边是世界空间：与字形一同随缩放成比例变化。
            StrokeSolid::new_in(outline.width, outline.color, ze_notion::MeasureSpace::World),
            Parent(surface_id),
        ));
    }

    Demo3D { camera3d }
}
