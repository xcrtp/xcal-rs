// demo3d.rs — 启动即见的 3D 场景（原住在 app 的 `build_runtime`）。
//
// 内容：白色线框立方体 + 参数曲面（引用内容纹理）+ 3D 相机 + 内容纹理公式。
// 归位到场景层后，app 只负责"装配 / 注册 / 找相机"，不再声明场景内容。

use std::sync::Arc;

use ze_notion::{Rect2D, SurfaceFn};
use ze_object::{
    CONTENT_TEXTURE_HEIGHT, CONTENT_TEXTURE_ID, CONTENT_TEXTURE_WIDTH, Camera2D, Camera3D,
    Camera3DHandle, Path, Projection, Transform,
};
use ze_prerender::InMemoryEngine;
use ze_render_ir::StrokeSolid;

use crate::content;
use crate::contract::object_id_of;
use crate::domain::CameraMode;
use crate::scene::Scene;
use crate::scene::components::{ContentTextureItem, Material3D, TextureHandle};
use crate::scene::primitives::{parametric_surface, wireframe_cube};
use crate::types::Color32;

/// 3D 场景装配产物。
pub struct Demo3D {
    /// 3D 相机句柄（自动轨道绑定由 app 装配）。
    pub camera3d: Camera3DHandle,
    /// 内容纹理相机（公式编译失败时为 `None`，由调用方回退）。
    pub content_camera: Option<Camera2D>,
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
    surface_material.texture = Some(TextureHandle::new(CONTENT_TEXTURE_ID));
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

    // ── 公式接入（内容纹理源）：Typst 内容管线 → 2D 描边路径 ──────────
    // 静态一条公式，启动时烘焙一次并 spawn 实体；它同时是 3D 曲面材质的纹理源
    // （带 `ContentTextureItem` 标记）。零 System 改动：公式作为普通
    // `(Path, StrokeSolid)` 对象参与既有录制。
    // 内容纹理相机（S3-3）：由公式的世界包围盒导出——`rect` 即纹理矩形
    // （0,0–512,512），让包围盒适配它（margin 0.8 ⇒ 公式宽度 ≈ 纹理宽 × 0.8）。
    let texture_rect = Rect2D::new(
        ze_notion::Vec2::ZERO,
        ze_notion::Vec2::new(CONTENT_TEXTURE_WIDTH as f32, CONTENT_TEXTURE_HEIGHT as f32),
    );
    let mut content_camera: Option<Camera2D> = None;
    match content::build_ir(engine, "x^2 + 1") {
        Some(ir) => {
            // 包围盒由理念层 `Path2D::bounds`（懒计算）给出 —— 不再遍历命令。
            let mut bounds: Option<Rect2D> = None;
            for outline in ze_content::stroke_outlines(&ir, 1.3, ze_notion::Vec2::new(-2.8, 3.0), false)
            {
                let mut path = outline.path;
                bounds = Some(match bounds {
                    Some(acc) => acc.union(&path.bounds()),
                    None => path.bounds(),
                });
                scene.spawn((
                    Path::new(path),
                    // 公式描边是世界空间：与字形一同随缩放成比例变化。
                    StrokeSolid::new_in(
                        outline.width,
                        outline.color,
                        ze_notion::MeasureSpace::World,
                    ),
                    // S3-3：标记该实体进内容纹理（D5：只录带标记的实体）。
                    ContentTextureItem,
                ));
            }
            if let Some(bbox) = bounds {
                // `fit_bounds` 自带退化保护（空盒 / 零宽 ⇒ 保持基底 scale）；
                // 基底取 40，与原实现里退化分支的兜底值同值。
                let base = Camera2D::new(ze_notion::Vec2::ZERO, 40.0, texture_rect);
                content_camera = Some(base.fit_bounds(bbox, 0.8));
            }
        }
        None => eprintln!("[zela-scene] 公式编译失败，跳过公式实体"),
    }

    Demo3D {
        camera3d,
        content_camera,
    }
}
