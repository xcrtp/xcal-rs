// 3d.rs
//
// 3D 核心类型（纯逻辑，无 GPU 依赖）：
// - PrimitiveTopology：图元拓扑
// - MeshGeometry：顶点/法线/UV/索引/拓扑 + 稳定 id/version（几何）
// - Material3D：颜色/纹理/UV 投影（外观）
// - Projection：透视/正交投影
// - Camera3D：视点 + 投影，提供视图矩阵与 VP 矩阵
//
// 几何与外观是两类关注点：`MeshGeometry` 只描述"长什么样"，
// `Material3D` 只描述"看起来什么颜色/贴什么纹理"。二者同层（均在 xcal-scene）。
//
// 数学类型直接使用 glam（与 Transform 组件一致），2D 包装类型（Vec3 等）
// 仅服务于 Transform2D trait 体系。

use crate::types::Color32;
use glam::Mat4;
use glam::camera::rh::proj::directx;
use glam::camera::rh::view::look_at_mat4;

/// 3D 图元拓扑方式。
///
/// 决定索引/顶点在管线中如何解释为图元。
///
/// # Example
/// ```rust
/// use xcal_scene::scene::components::PrimitiveTopology;
///
/// assert_eq!(PrimitiveTopology::Triangles as u8, PrimitiveTopology::Triangles as u8);
/// assert_ne!(PrimitiveTopology::Lines, PrimitiveTopology::Points);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimitiveTopology {
    /// 三角形列表：每 3 个顶点构成一个三角形。
    Triangles,
    /// 线段列表：每 2 个顶点构成一条线段。
    Lines,
    /// 点列表：每个顶点一个点。
    Points,
}

/// 分配进程内唯一的几何 id。
fn next_geometry_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 3D 网格几何组件：顶点位置 + 可选法线/UV + 可选索引 + 拓扑。
///
/// 只描述"长什么样"，不含颜色等外观信息（外观见 [`Material3D`]）。
/// 每个几何体带一个进程内唯一、跨 `clone` 稳定的 `id`，以及可手动递增的
/// `version`；二者的组合（`ResourceKey::Versioned`）作为 GPU 侧缓存的键：
/// 几何内容变化时调用 [`MeshGeometry::bump_version`]，所有引用者的缓存一起失效。
///
/// # Example
/// ```rust
/// use xcal_scene::scene::components::{MeshGeometry, PrimitiveTopology};
///
/// // 一个无索引三角形。
/// let mesh = MeshGeometry::new(
///     vec![
///         glam::Vec3::new(-1.0, -1.0, 0.0),
///         glam::Vec3::new(1.0, -1.0, 0.0),
///         glam::Vec3::new(0.0, 1.0, 0.0),
///     ],
///     None,
///     PrimitiveTopology::Triangles,
/// );
/// assert_eq!(mesh.positions.len(), 3);
/// assert!(mesh.indices.is_none());
/// ```
#[derive(Debug, Clone)]
pub struct MeshGeometry {
    /// 顶点位置（模型空间）。
    pub positions: Vec<glam::Vec3>,
    /// 可选逐顶点法线；长度应与 `positions` 一致。
    pub normals: Option<Vec<glam::Vec3>>,
    /// 可选逐顶点 UV；长度应与 `positions` 一致。
    pub uvs: Option<Vec<glam::Vec2>>,
    /// 可选索引；`None` 表示按顶点顺序连续解释。
    pub indices: Option<Vec<u32>>,
    /// 图元拓扑。
    pub topology: PrimitiveTopology,
    /// 进程内唯一且跨 `clone` 稳定的几何 id。
    id: u64,
    /// 几何内容版本号，由 [`MeshGeometry::bump_version`] 递增。
    version: u64,
}

impl MeshGeometry {
    /// 构造几何：`id` 自动分配，`version` 从 0 开始。
    pub fn new(
        positions: Vec<glam::Vec3>,
        indices: Option<Vec<u32>>,
        topology: PrimitiveTopology,
    ) -> Self {
        Self {
            positions,
            normals: None,
            uvs: None,
            indices,
            topology,
            id: next_geometry_id(),
            version: 0,
        }
    }

    /// 附加逐顶点法线（builder 风格）。
    pub fn with_normals(mut self, normals: Vec<glam::Vec3>) -> Self {
        self.normals = Some(normals);
        self
    }

    /// 附加逐顶点 UV（builder 风格）。
    pub fn with_uvs(mut self, uvs: Vec<glam::Vec2>) -> Self {
        self.uvs = Some(uvs);
        self
    }

    /// 稳定几何 id（跨 `clone` 保持不变）。
    pub fn id(&self) -> u64 {
        self.id
    }

    /// 当前几何版本号。
    pub fn version(&self) -> u64 {
        self.version
    }

    /// 递增几何版本号，使所有引用者的 GPU 缓存失效。
    pub fn bump_version(&mut self) -> u64 {
        self.version += 1;
        self.version
    }
}

/// 纹理句柄：不透明标识符，仅用于在资源层定位纹理。
///
/// 句柄本身不持有 GPU 资源，具体纹理由后端按 id 解析。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextureHandle(u64);

impl TextureHandle {
    /// 以给定 id 构造句柄。
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// 句柄 id。
    pub fn id(&self) -> u64 {
        self.0
    }
}

/// 无 UV 几何的兜底 UV 投影策略。
///
/// 几何自带真实 UV（[`MeshGeometry::uvs`]）时应忽略本策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UvProjection {
    /// 沿主视轴平面投影（默认）。
    #[default]
    Planar,
    /// 球面投影。
    Spherical,
    /// 三平面投影。
    Triplanar,
}

/// 3D 材质组件：颜色 + 可选纹理句柄 + 无 UV 几何的兜底 UV 投影。
///
/// 只描述"看起来什么颜色/贴什么纹理"，不含几何数据（几何见 [`MeshGeometry`]）。
///
/// # Example
/// ```rust
/// use xcal_scene::scene::components::Material3D;
/// use xcal_scene::types::Color32;
///
/// let m = Material3D::color(Color32::new(255, 0, 0, 255));
/// assert_eq!(m.color.r, 255);
/// assert!(m.texture.is_none());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Material3D {
    /// 整体颜色（无纹理时的着色来源，有纹理时作为调制色）。
    pub color: Color32,
    /// 可选纹理句柄。
    pub texture: Option<TextureHandle>,
    /// 无 UV 几何的兜底 UV 投影策略。
    pub uv_projection: UvProjection,
}

impl Material3D {
    /// 以单一颜色构造材质（无纹理、平面投影兜底）。
    pub fn color(color: Color32) -> Self {
        Self {
            color,
            ..Self::default()
        }
    }
}

/// 投影方式。
///
/// 两种变体都自带 `aspect`（宽高比），可直接生成投影矩阵；
/// 视口尺寸变化时可用 [`Projection::set_aspect`] 或
/// [`Projection::matrix_with_aspect`] 覆盖。
///
/// 矩阵使用右手系、wgpu/D3D NDC 约定（深度 [0, 1]、Y-up）。
///
/// # Example
/// ```rust
/// use xcal_scene::scene::components::Projection;
///
/// let p = Projection::perspective(
///     std::f32::consts::FRAC_PI_3, 16.0 / 9.0, 0.1, 100.0,
/// );
/// let m = p.matrix();
/// // 相机空间 z=-5 的点，透视裁剪空间 w == 5。
/// let clip = m * glam::Vec4::new(0.0, 0.0, -5.0, 1.0);
/// assert!((clip.w - 5.0).abs() < 1e-5);
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Projection {
    /// 透视投影。
    Perspective {
        /// 垂直视场角（弧度）。
        fov_y: f32,
        /// 宽高比（width / height）。
        aspect: f32,
        /// 近裁剪面距离（> 0）。
        near: f32,
        /// 远裁剪面距离（> near）。
        far: f32,
    },
    /// 正交投影。
    Orthographic {
        /// 垂直半高（世界单位）；水平半宽 = `extent * aspect`。
        extent: f32,
        /// 宽高比（width / height）。
        aspect: f32,
        /// 近裁剪面距离。
        near: f32,
        /// 远裁剪面距离。
        far: f32,
    },
}

impl Projection {
    /// 构造透视投影。
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Self {
        Self::Perspective {
            fov_y,
            aspect,
            near,
            far,
        }
    }

    /// 构造正交投影；`extent` 为垂直半高。
    pub fn orthographic(extent: f32, aspect: f32, near: f32, far: f32) -> Self {
        Self::Orthographic {
            extent,
            aspect,
            near,
            far,
        }
    }

    /// 当前存储的宽高比。
    pub fn aspect(&self) -> f32 {
        match self {
            Projection::Perspective { aspect, .. } | Projection::Orthographic { aspect, .. } => {
                *aspect
            }
        }
    }

    /// 更新宽高比（视口尺寸变化时调用）。
    pub fn set_aspect(&mut self, new_aspect: f32) {
        match self {
            Projection::Perspective { aspect, .. } | Projection::Orthographic { aspect, .. } => {
                *aspect = new_aspect;
            }
        }
    }

    /// 投影矩阵（使用自身存储的宽高比）。
    pub fn matrix(&self) -> Mat4 {
        self.matrix_with_aspect(self.aspect())
    }

    /// 以给定宽高比生成投影矩阵（不修改自身）。
    ///
    /// 使用右手坐标系、wgpu/D3D NDC 约定（深度 [0, 1]、Y-up）：
    /// - 透视：[`glam::camera::rh::proj::directx::perspective`]
    /// - 正交：[`glam::camera::rh::proj::directx::orthographic`]
    pub fn matrix_with_aspect(&self, aspect: f32) -> Mat4 {
        match self {
            Projection::Perspective {
                fov_y, near, far, ..
            } => directx::perspective(*fov_y, aspect, *near, *far),
            Projection::Orthographic {
                extent, near, far, ..
            } => {
                let half_h = *extent;
                let half_w = half_h * aspect;
                directx::orthographic(-half_w, half_w, -half_h, half_h, *near, *far)
            }
        }
    }
}

/// 3D 相机：视点位置、注视目标、上方向 + 投影参数。
///
/// # Example
/// ```rust
/// use xcal_scene::scene::components::{Camera3D, Projection};
///
/// let cam = Camera3D::new(
///     glam::Vec3::new(0.0, 0.0, 5.0),
///     glam::Vec3::ZERO,
///     glam::Vec3::Y,
///     Projection::perspective(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 100.0),
/// );
/// // 世界原点在相机空间中位于 z=-5（正前方 5 单位）。
/// let origin_view = cam.view_matrix().project_point3(glam::Vec3::ZERO);
/// assert!((origin_view.z + 5.0).abs() < 1e-5);
/// // VP 矩阵 = P * V。
/// let vp = cam.view_projection(1.0);
/// assert!(vp.is_finite());
/// ```
#[derive(Debug, Clone, Copy, derive_new::new)]
pub struct Camera3D {
    /// 相机位置（世界空间）。
    pub eye: glam::Vec3,
    /// 注视目标点（世界空间）。
    pub target: glam::Vec3,
    /// 上方向（通常为 [`glam::Vec3::Y`]）。
    pub up: glam::Vec3,
    /// 投影方式。
    pub projection: Projection,
}

impl Camera3D {
    /// 视图矩阵（世界空间 → 相机空间），右手 look-at。
    pub fn view_matrix(&self) -> Mat4 {
        look_at_mat4(self.eye, self.target, self.up)
    }

    /// 视图-投影矩阵 `P * V`。
    ///
    /// `aspect` 由视口提供（每帧可用实际渲染区域宽高比覆盖投影中的值）。
    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection.matrix_with_aspect(aspect) * self.view_matrix()
    }

    /// 更新投影宽高比。
    pub fn set_aspect(&mut self, aspect: f32) {
        self.projection.set_aspect(aspect);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Color32;

    #[test]
    fn test_mesh_geometry_and_material3d_independent() {
        // 1) 几何与材质可独立构造。
        let mut geometry = MeshGeometry::new(
            vec![glam::Vec3::ZERO, glam::Vec3::X, glam::Vec3::Y],
            Some(vec![0, 1, 2]),
            PrimitiveTopology::Triangles,
        );
        let material = Material3D::color(Color32::WHITE);
        let id_before = geometry.id();
        assert_eq!(geometry.version(), 0);

        // 2) 改材质不影响几何 identity/version：拆分后二者关注点天然隔离，
        //    不再需要旧 `hash_mesh` 里手工排除 color 的特例。
        let mut edited_material = material;
        edited_material.color = Color32::RED;
        edited_material.uv_projection = UvProjection::Spherical;
        assert_eq!(geometry.id(), id_before);
        assert_eq!(geometry.version(), 0, "改材质不应改变几何版本");
        assert_ne!(material, edited_material);

        // 3) 只有几何自身内容变更才递增 version，且 id 不变。
        assert_eq!(geometry.bump_version(), 1);
        assert_eq!(geometry.version(), 1);
        assert_eq!(geometry.id(), id_before, "bump_version 不改 id");
    }

    #[test]
    fn test_parametric_surface_uv_boundaries_exact() {
        // 临时构造 1×1 参数网格（u 沿 x 参数、v 沿 y 参数），验证 UV 被
        // 精确保留：边界值恰好 0.0 / 1.0，无浮点漂移。
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        for j in 0..=1 {
            let v = j as f32;
            for i in 0..=1 {
                let u = i as f32;
                positions.push(glam::Vec3::new(u * 2.0 - 1.0, v * 2.0 - 1.0, 0.0));
                uvs.push(glam::Vec2::new(u, v));
            }
        }
        let mesh = MeshGeometry::new(positions, None, PrimitiveTopology::Triangles).with_uvs(uvs);

        let uvs = mesh.uvs.as_ref().expect("uvs 应存在");
        assert_eq!(uvs[0], glam::Vec2::new(0.0, 0.0));
        assert_eq!(uvs[1], glam::Vec2::new(1.0, 0.0));
        assert_eq!(uvs[2], glam::Vec2::new(0.0, 1.0));
        assert_eq!(uvs[3], glam::Vec2::new(1.0, 1.0));
    }

    #[test]
    fn test_camera3d_view_matrix_eye_on_z_axis() {
        // eye=(0,0,5) 注视原点：视图矩阵应把世界原点变换到相机空间 (0,0,-5)。
        let cam = Camera3D::new(
            glam::Vec3::new(0.0, 0.0, 5.0),
            glam::Vec3::ZERO,
            glam::Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        );
        let view = cam.view_matrix();

        let origin_in_view = view.project_point3(glam::Vec3::ZERO);
        assert!(
            (origin_in_view.z - (-5.0)).abs() < 1e-5,
            "origin should be at z=-5 in view space, got {origin_in_view:?}"
        );

        // eye 自身在相机空间位于原点。
        let eye_in_view = view * glam::Vec4::new(0.0, 0.0, 5.0, 1.0);
        assert!(
            eye_in_view.z.abs() < 1e-5,
            "eye should be at z=0, got {eye_in_view:?}"
        );
    }

    #[test]
    fn test_perspective_matrix_w_component_is_view_depth() {
        // 透视投影下，相机空间 z=-d 的点裁剪空间 w 应等于 d。
        let proj = Projection::perspective(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 100.0);
        let m = proj.matrix_with_aspect(1.0);
        let clip = m * glam::Vec4::new(0.0, 0.0, -5.0, 1.0);
        assert!(
            (clip.w - 5.0).abs() < 1e-5,
            "perspective w should equal view depth 5.0, got {}",
            clip.w
        );
    }

    #[test]
    fn test_projection_set_aspect() {
        let mut p = Projection::perspective(1.0, 1.0, 0.1, 100.0);
        assert_eq!(p.aspect(), 1.0);
        p.set_aspect(16.0 / 9.0);
        assert!((p.aspect() - 16.0 / 9.0).abs() < 1e-6);
    }

    #[test]
    fn test_camera3d_view_projection_uses_aspect() {
        let cam = Camera3D::new(
            glam::Vec3::new(0.0, 0.0, 5.0),
            glam::Vec3::ZERO,
            glam::Vec3::Y,
            Projection::perspective(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 100.0),
        );
        let vp = cam.view_projection(2.0);
        let expected = cam.projection.matrix_with_aspect(2.0) * cam.view_matrix();
        assert_eq!(vp.to_cols_array(), expected.to_cols_array());
    }
}
