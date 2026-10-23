//! B10 **3D 场景 → 矢量后端**的桥：`Camera3D` → [`View3`]、`LightQueue` → 灯、
//! `DrawQueue::meshes` → [`MeshRecord`]。
//!
//! # 为什么桥在本 crate（B11 Step 5 从 `zela-app` 移入）
//! `zela-render-vector` 是**移植层**，只依赖 kernel（`ze-notion` / `ze-render-ir`），
//! 不认识 `zela-render` 的队列与场景组件（见它的 Cargo.toml 与 B4 验收 §1.3）。
//! 桥接需要两边同时在场；而 native（`zela-app::frame_loop`）与浏览器（`zela-web`）
//! 都要用它 ⇒ 放在两边唯一的公共层 `zela-playback`，避免两份实现漂移。
//!
//! # 口径（全部来自代码事实，不猜）
//! - 相机：右手系，`focal = |eye − target|`，`frame = 2·focal·tan(fov_y/2)`；
//!   矢量 `View3` 只有这种 **focal 透视**形式 ⇒ 正交相机**显式报错**，不近似。
//! - 灯：`LightQueue.direction` 是**传播方向**，矢量 `Light::Sun.direction` 是
//!   "指向光源" ⇒ **取反**。环境项与 wgpu 兜底同值（`DEFAULT_AMBIENT = 0.2`）。
//! - 空灯队列 ⇒ 与 wgpu 的 `default_light_bytes` 同参：方向 `+Z` 传播（=> toward
//!   `−Z`）+ 环境 0.2。
//! - 网格：`Material3D` 没有 reflectance 字段 ⇒ 取 `0.5`（电介质常用值），
//!   该取舍记在 GAPS X1-03。

use ze_notion::{Color32, Mat4};
use ze_render_ir::{AmbientOcclusion, MeshForm, RenderParams, ToneMapping};
use ze_scene_core::components::{Material3D, MeshGeometry, PrimitiveTopology};
use ze_scene_core::object::{Camera3D, Projection};
use zela_render::draw::DrawQueue;
use zela_render::light_queue::LightQueue;
use zela_render_vector::{Light, MeshRecord, PathRecord, View3};

/// 与 wgpu 兜底一致的环境项（`wgpu_3d.rs` 的 `DEFAULT_AMBIENT`）。
const DEFAULT_AMBIENT: f32 = 0.2;

/// `Material3D` 没有 reflectance 字段时取的常数（电介质 F0 ≈ 0.04）。
const DEFAULT_REFLECTANCE: f32 = 0.5;

/// 把 `Camera3D` 分解成矢量后端的 `View3`（透视）。
///
/// 失败条件（都显式，不静默近似）：
/// - 正交投影（`View3` 无正交形式）；
/// - `eye == target`（无视线）或 `up` 与视线平行（look-at 退化）。
pub fn view3(
    camera: &Camera3D,
    size: (u32, u32),
    background: [f32; 4],
    params: &RenderParams,
) -> Result<View3, String> {
    let Projection::Perspective { fov_y, .. } = camera.projection else {
        return Err(
            "矢量后端的 3D 视图只有 focal 透视形式：本帧相机是正交投影（不静默近似）"
                .to_string(),
        );
    };
    let to_eye = camera.eye - camera.target;
    let focal = to_eye.length();
    if focal < 1e-6 {
        return Err(format!(
            "3D 相机退化：eye 与 target 重合（focal = {focal}）"
        ));
    }
    let toward_eye = to_eye / focal;
    let forward = -toward_eye;
    let right = forward.cross(camera.up);
    let right_len = right.length();
    if right_len < 1e-6 {
        return Err("3D 相机退化：up 与视线平行（look-at 无解）".to_string());
    }
    let right = right / right_len;
    let up = right.cross(forward);

    let aspect = size.0 as f32 / size.1.max(1) as f32;
    let frame_h = 2.0 * focal * (fov_y * 0.5).tan();
    let frame_w = frame_h * aspect;

    let (exposure, tone) = match params.tone {
        Some(ToneMapping { exposure, curve }) => (exposure, Some(curve)),
        None => (1.0, None),
    };
    let bloom = params.bloom.map(|b| b.intensity).unwrap_or(0.0);
    let ao = params
        .ao
        .map(|AmbientOcclusion { radius, intensity }| (intensity, radius));

    Ok(View3 {
        frame: (frame_w, frame_h),
        center: [camera.target.x, camera.target.y, camera.target.z],
        axes: [
            [right.x, right.y, right.z],
            [up.x, up.y, up.z],
            [toward_eye.x, toward_eye.y, toward_eye.z],
        ],
        zoom: 1.0,
        focal,
        size,
        background,
        // 只在没有显式灯时被上游用于记录；这里给一个稳定的世界方向。
        light_source: [0.0, -1.0, 0.0],
        exposure,
        tone,
        bloom,
        ao,
        lights: Vec::new(),
    })
}

/// `LightQueue` → 矢量灯列表（方向取反 + 环境项与 wgpu 兜底同值）。
pub fn lights(queue: Option<&LightQueue>) -> Vec<Light> {
    let mut out = Vec::new();
    if let Some(queue) = queue {
        out.extend(queue.directional.iter().map(|light| Light::Sun {
            // `direction` 是传播方向；矢量 Sun 要"指向光源" ⇒ 取反。
            direction: [-light.direction.x, -light.direction.y, -light.direction.z],
            color: [light.color[0], light.color[1], light.color[2]],
            intensity: light.intensity,
            shadows: false,
        }));
        out.extend(queue.points.iter().map(|light| Light::Point {
            at: [light.position.x, light.position.y, light.position.z],
            color: [light.color[0], light.color[1], light.color[2]],
            intensity: light.intensity,
            reach: light.range,
        }));
    }
    if out.is_empty() {
        // 与 wgpu 的 `default_light_bytes` 同参：传播 +Z（⇒ 指向光源 −Z）。
        out.push(Light::Sun {
            direction: [0.0, 0.0, -1.0],
            color: [1.0, 1.0, 1.0],
            intensity: 1.0,
            shadows: false,
        });
    }
    // 环境项在 wgpu 侧**恒**为 `DEFAULT_AMBIENT`（空队列也一样）⇒ 这里保持一致；
    // 它同时是 AO 生效的前提（`occlusion_of` 要求 kind 0 或 4）。
    out.push(Light::Ambient {
        color: [1.0, 1.0, 1.0],
        intensity: DEFAULT_AMBIENT,
    });
    out
}

/// 本帧的全部 3D 网格 → 矢量记录（顺序 = 队列顺序）。
pub fn mesh_records(queue: &DrawQueue) -> Result<Vec<PathRecord>, String> {
    queue
        .meshes()
        .enumerate()
        .map(|(index, draw)| {
            mesh_record(&draw.geometry, draw.transform, &draw.material)
                .map_err(|error| format!("3D 网格 #{index}：{error}"))
        })
        .collect()
}

/// 一条网格：几何 + 模型矩阵 + 材质。
fn mesh_record(
    geometry: &MeshGeometry,
    transform: Mat4,
    material: &Material3D,
) -> Result<PathRecord, String> {
    if geometry.topology != PrimitiveTopology::Triangles {
        return Err(format!(
            "矢量后端只接受三角形面（MeshForm::{:?}），本网格是 {:?}",
            MeshForm::Tri,
            geometry.topology
        ));
    }
    let positions: Vec<[f64; 3]> = geometry
        .positions
        .iter()
        .map(|p| [p.x as f64, p.y as f64, p.z as f64])
        .collect();
    if positions.is_empty() {
        return Err("几何没有顶点".to_string());
    }
    let uvs = geometry.uvs.as_ref().map(|uvs| {
        uvs.iter()
            .map(|uv| [uv.x as f64, uv.y as f64])
            .collect::<Vec<_>>()
    });
    let normals = geometry.normals.as_ref().map(|normals| {
        normals
            .iter()
            .map(|n| [n.x as f64, n.y as f64, n.z as f64])
            .collect::<Vec<_>>()
    });
    let triangles = match &geometry.indices {
        Some(indices) => indices.clone(),
        None => (0..positions.len() as u32).collect(),
    };
    Ok(PathRecord::Mesh(MeshRecord {
        positions,
        uvs,
        normals,
        triangles,
        fill: rgba(material.color),
        model: mat34(transform),
        material: Some([
            material.metallic,
            material.roughness,
            DEFAULT_REFLECTANCE,
        ]),
    }))
}

fn rgba(color: Color32) -> [f32; 4] {
    [
        color.r as f32 / 255.0,
        color.g as f32 / 255.0,
        color.b as f32 / 255.0,
        color.a as f32 / 255.0,
    ]
}

/// glam `Mat4`（列主序）→ 上游 `Mat34`（3×4 行主序）。
fn mat34(m: Mat4) -> [[f32; 4]; 3] {
    let cols = m.to_cols_array_2d();
    [
        [cols[0][0], cols[1][0], cols[2][0], cols[3][0]],
        [cols[0][1], cols[1][1], cols[2][1], cols[3][1]],
        [cols[0][2], cols[1][2], cols[2][2], cols[3][2]],
    ]
}

/// 供调用方断言"本帧是否真的有 3D 内容"（空网格 ⇒ 不是 3D 绘制）。
pub fn has_meshes(queue: &DrawQueue) -> bool {
    queue.meshes().next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_notion::Vec3;
    use ze_render_ir::Bloom;
    use ze_scene_core::components::PrimitiveTopology;

    fn camera(projection: Projection) -> Camera3D {
        Camera3D {
            eye: Vec3::new(0.0, 0.0, 8.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            projection,
        }
    }

    fn params(tone: bool) -> RenderParams {
        let mut params = RenderParams::default();
        if tone {
            params.tone = Some(ToneMapping {
                exposure: 2.0,
                curve: ze_render_ir::ToneCurve::AgX,
            });
            params.bloom = Some(Bloom::default());
            params.ao = Some(AmbientOcclusion::default());
        }
        params
    }

    /// 透视相机分解：眼在 +Z ⇒ 朝外轴 +Z、右 +X、上 +Y，focal = 8。
    #[test]
    fn perspective_camera_decomposes_into_view3() {
        let view = view3(
            &camera(Projection::perspective(0.7, 4.0 / 3.0, 0.1, 100.0)),
            (64, 48),
            [0.0, 0.0, 0.0, 1.0],
            &params(false),
        )
        .expect("透视相机可分解");
        assert_eq!(view.focal, 8.0);
        assert_eq!(view.center, [0.0, 0.0, 0.0]);
        assert_eq!(view.axes[0], [1.0, 0.0, 0.0], "right = +X");
        assert_eq!(view.axes[1], [0.0, 1.0, 0.0], "up = +Y");
        assert_eq!(view.axes[2], [0.0, 0.0, 1.0], "toward eye = +Z");
        let expect_h = 2.0 * 8.0 * (0.7f32 * 0.5).tan();
        assert!((view.frame.1 - expect_h).abs() < 1e-4);
        assert!((view.frame.0 - expect_h * (64.0 / 48.0)).abs() < 1e-4);
        assert_eq!(view.tone, None, "默认不开 tone");
        assert_eq!(view.bloom, 0.0);
    }

    /// 着色栈参数从 `RenderParams` 透传（tone 曲线 + 曝光 / bloom 强度 / AO）。
    #[test]
    fn shading_params_pass_through() {
        let view = view3(
            &camera(Projection::perspective(0.7, 1.0, 0.1, 100.0)),
            (64, 64),
            [0.0; 4],
            &params(true),
        )
        .unwrap();
        assert_eq!(view.tone, Some(ze_render_ir::ToneCurve::AgX));
        assert_eq!(view.exposure, 2.0);
        assert!(view.bloom > 0.0);
        assert!(view.ao.is_some());
    }

    /// 正交相机与退化 look-at **显式报错**（不静默近似）。
    #[test]
    fn unsupported_cameras_fail_explicitly() {
        let ortho = camera(Projection::orthographic(2.0, 1.0, 0.1, 100.0));
        let error = view3(&ortho, (64, 64), [0.0; 4], &params(false)).unwrap_err();
        assert!(error.contains("正交"), "{error}");

        let mut degenerate = camera(Projection::perspective(0.7, 1.0, 0.1, 100.0));
        degenerate.target = degenerate.eye;
        let error = view3(&degenerate, (64, 64), [0.0; 4], &params(false)).unwrap_err();
        assert!(error.contains("退化"), "{error}");

        let mut parallel_up = camera(Projection::perspective(0.7, 1.0, 0.1, 100.0));
        parallel_up.up = Vec3::Z;
        let error = view3(&parallel_up, (64, 64), [0.0; 4], &params(false)).unwrap_err();
        assert!(error.contains("平行"), "{error}");
    }

    /// 灯：方向取反（传播 → 指向光源），并恒定带 0.2 环境项；空队列给 wgpu 同款兜底。
    #[test]
    fn lights_negate_direction_and_keep_ambient() {
        let mut queue = LightQueue::new();
        queue.directional.push(
            zela_render::light_queue::DirectionalLightRecord::new(
                Vec3::new(0.0, 0.0, 1.0),
                Color32::WHITE,
                2.0,
            ),
        );
        queue.points.push(zela_render::light_queue::PointLightRecord::new(
            Vec3::new(1.0, 2.0, 3.0),
            Color32::WHITE,
            1.0,
            5.0,
        ));
        let mapped = lights(Some(&queue));
        assert_eq!(mapped.len(), 3, "方向光 + 点光 + 环境项");
        match mapped[0] {
            Light::Sun { direction, intensity, .. } => {
                assert_eq!(direction, [0.0, 0.0, -1.0], "传播 +Z ⇒ 指向光源 −Z");
                assert_eq!(intensity, 2.0);
            }
            other => panic!("第一盏应是 Sun：{other:?}"),
        }
        match mapped[2] {
            Light::Ambient { intensity, .. } => assert_eq!(intensity, DEFAULT_AMBIENT),
            other => panic!("最后一盏应是环境项：{other:?}"),
        }

        let empty = lights(None);
        assert_eq!(empty.len(), 2, "空队列 = 默认方向光 + 环境项");
        match empty[0] {
            Light::Sun { direction, .. } => assert_eq!(direction, [0.0, 0.0, -1.0]),
            other => panic!("{other:?}"),
        }
    }

    /// 网格：无索引 ⇒ 顺序三角形；无 UV / 法线 ⇒ 留 `None`（上游补零 / 求法线）。
    #[test]
    fn mesh_record_maps_geometry_and_material() {
        let geometry = MeshGeometry::new(
            vec![Vec3::ZERO, Vec3::X, Vec3::Y],
            None,
            PrimitiveTopology::Triangles,
        );
        let mut material = Material3D::color(Color32::RED);
        material.metallic = 0.25;
        material.roughness = 0.75;
        let record = mesh_record(&geometry, Mat4::IDENTITY, &material).expect("可映射");
        let PathRecord::Mesh(mesh) = record else {
            panic!("应是网格记录");
        };
        assert_eq!(mesh.positions.len(), 3);
        assert_eq!(mesh.triangles, vec![0, 1, 2], "无索引 ⇒ 顺序");
        assert!(mesh.uvs.is_none() && mesh.normals.is_none());
        assert_eq!(mesh.fill, [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(mesh.material, Some([0.25, 0.75, DEFAULT_REFLECTANCE]));
        mesh.validate().expect("映射结果必须自洽");
    }

    /// 非三角形拓扑 ⇒ 显式报错（`MeshForm::Line` 目前没有后端接受）。
    #[test]
    fn non_triangle_topology_fails_explicitly() {
        let geometry =
            MeshGeometry::new(vec![Vec3::ZERO, Vec3::X], None, PrimitiveTopology::Lines);
        let error = mesh_record(&geometry, Mat4::IDENTITY, &Material3D::color(Color32::WHITE))
            .unwrap_err();
        assert!(error.contains("三角形"), "{error}");
    }
}
