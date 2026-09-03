// 3d.rs
//
// 3D 核心类型（纯逻辑，无 GPU 依赖）：
// - PrimitiveTopology：图元拓扑
// - Mesh3D：顶点/索引/拓扑/颜色
// - Projection：透视/正交投影
// - Camera3D：视点 + 投影，提供视图矩阵与 VP 矩阵
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
/// use xcal_core::scene::components::PrimitiveTopology;
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

/// 3D 网格组件：顶点缓冲 + 可选索引缓冲 + 拓扑 + 整网格颜色。
///
/// MVE 阶段不支持逐顶点色与法线；颜色由 uniform 传入。
///
/// # Example
/// ```rust
/// use xcal_core::scene::components::{Mesh3D, PrimitiveTopology};
/// use xcal_core::types::Color32;
///
/// // 一个无索引三角形。
/// let mesh = Mesh3D::new(
///     vec![
///         glam::Vec3::new(-1.0, -1.0, 0.0),
///         glam::Vec3::new(1.0, -1.0, 0.0),
///         glam::Vec3::new(0.0, 1.0, 0.0),
///     ],
///     None,
///     PrimitiveTopology::Triangles,
///     Color32::new(255, 255, 255, 255),
/// );
/// assert_eq!(mesh.vertices.len(), 3);
/// assert!(mesh.indices.is_none());
/// ```
#[derive(Debug, Clone, derive_new::new)]
pub struct Mesh3D {
    /// 顶点位置（模型空间）。
    pub vertices: Vec<glam::Vec3>,
    /// 可选索引；`None` 表示按顶点顺序连续解释。
    pub indices: Option<Vec<u32>>,
    /// 图元拓扑。
    pub topology: PrimitiveTopology,
    /// 整网格颜色。
    pub color: Color32,
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
/// use xcal_core::scene::components::Projection;
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
/// use xcal_core::scene::components::{Camera3D, Projection};
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
