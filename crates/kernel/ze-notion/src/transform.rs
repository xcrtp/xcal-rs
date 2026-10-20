// transform.rs
//
// 仿射变换包装：Transform2D（Mat3）/ Transform3D（Mat4）。
// 纯数据 + 组合/应用函数；具体后端如何消费矩阵由渲染层决定。

use crate::mat::{Mat3, Mat4};
use crate::vec::{Vec2, Vec3};

/// 2D 仿射变换（3x3 齐次矩阵）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform2D {
    pub matrix: Mat3,
}

impl Default for Transform2D {
    fn default() -> Self {
        Self::identity()
    }
}

impl Transform2D {
    pub fn identity() -> Self {
        Self {
            matrix: Mat3::IDENTITY,
        }
    }

    pub fn translate(t: Vec2) -> Self {
        Self {
            matrix: Mat3::from_translation(t),
        }
    }

    pub fn rotate(angle: f32) -> Self {
        Self {
            matrix: Mat3::from_rotation_z(angle),
        }
    }

    pub fn scale(s: Vec2) -> Self {
        Self {
            matrix: Mat3::from_scale_angle_translation(s, 0.0, Vec2::ZERO),
        }
    }

    /// 组合变换：`self` 之后再应用 `other`（即 `other.matrix * self.matrix`）。
    pub fn compose(self, other: Self) -> Self {
        Self {
            matrix: other.matrix * self.matrix,
        }
    }

    /// 对点应用变换（齐次坐标 w=1）。
    pub fn apply_point(&self, p: Vec2) -> Vec2 {
        self.matrix.transform_point2(p)
    }
}

/// 3D 仿射变换（4x4 齐次矩阵）。
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform3D {
    pub matrix: Mat4,
}

impl Default for Transform3D {
    fn default() -> Self {
        Self::identity()
    }
}

impl Transform3D {
    pub fn identity() -> Self {
        Self {
            matrix: Mat4::IDENTITY,
        }
    }

    pub fn from_matrix(matrix: Mat4) -> Self {
        Self { matrix }
    }

    pub fn translate(t: Vec3) -> Self {
        Self {
            matrix: Mat4::from_translation(t),
        }
    }

    pub fn rotate(axis: Vec3, angle: f32) -> Self {
        Self {
            matrix: Mat4::from_axis_angle(axis.normalize_or_zero(), angle),
        }
    }

    pub fn scale(s: Vec3) -> Self {
        Self {
            matrix: Mat4::from_scale(s),
        }
    }

    /// 组合变换：`self` 之后再应用 `other`（即 `other.matrix * self.matrix`）。
    pub fn compose(self, other: Self) -> Self {
        Self {
            matrix: other.matrix * self.matrix,
        }
    }

    /// 对点应用变换（齐次坐标 w=1）。
    pub fn apply_point(&self, p: Vec3) -> Vec3 {
        self.matrix.transform_point3(p)
    }
}

impl Transform3D {
    /// **2D 投影视图**（S1 / D4）：取左上 2×2 线性子块 + xy 平移 ⇒ [`Transform2D`]。
    ///
    /// ⚠️ **这是投影，不是同构**：`z` 平移与 xy↔z 耦合被**丢弃**（不报错，
    /// 铁律 3"不静默"在此以 doc + `GAPS.md` 记账满足）。"2D 对象挂 3D 父下"
    /// 是 S6 的目标形态，本轮若报错会提前堵死该路径。
    ///
    /// 组合顺序与 [`Transform2D`] 的约定一致（scale → rotation → translation），
    /// 3×3 齐次第三行恒为 `(0, 0, 1)`。
    pub fn to_affine2d(&self) -> Transform2D {
        let m = self.matrix;
        Transform2D {
            matrix: Mat3::from_cols(
                Vec3::new(m.x_axis.x, m.x_axis.y, 0.0),
                Vec3::new(m.y_axis.x, m.y_axis.y, 0.0),
                Vec3::new(m.w_axis.x, m.w_axis.y, 1.0),
            ),
        }
    }
}

impl From<Mat4> for Transform3D {
    fn from(matrix: Mat4) -> Self {
        Self::from_matrix(matrix)
    }
}

impl From<Transform3D> for Mat4 {
    fn from(t: Transform3D) -> Self {
        t.matrix
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_2d_identity_applies_point() {
        let t = Transform2D::identity();
        let p = Vec2::new(3.0, 4.0);
        assert_eq!(t.apply_point(p), p);
    }

    #[test]
    fn test_2d_translate() {
        let t = Transform2D::translate(Vec2::new(10.0, -5.0));
        assert_eq!(t.apply_point(Vec2::new(1.0, 1.0)), Vec2::new(11.0, -4.0));
    }

    #[test]
    fn test_2d_compose_order() {
        // 先平移 (10,0)，再缩放 2x：点 (1,0) → 平移 → (11,0) → 缩放 → (22,0)。
        let t = Transform2D::translate(Vec2::new(10.0, 0.0))
            .compose(Transform2D::scale(Vec2::new(2.0, 2.0)));
        assert_eq!(t.apply_point(Vec2::new(1.0, 0.0)), Vec2::new(22.0, 0.0));
    }

    #[test]
    fn test_3d_translate_and_compose() {
        let t = Transform3D::translate(Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(t.apply_point(Vec3::ZERO), Vec3::new(1.0, 2.0, 3.0));
        let t2 = t.compose(Transform3D::scale(Vec3::new(2.0, 2.0, 2.0)));
        assert_eq!(
            t2.apply_point(Vec3::new(0.0, 0.0, 0.0)),
            Vec3::new(2.0, 4.0, 6.0)
        );
    }

    #[test]
    fn test_3d_rotate_around_y() {
        let t = Transform3D::rotate(Vec3::Y, std::f32::consts::FRAC_PI_2);
        let p = t.apply_point(Vec3::new(1.0, 0.0, 0.0));
        // 绕 +Y 旋转 90°：+X → -Z（右手系）。
        assert!((p.z + 1.0).abs() < 1e-5, "got {p:?}");
        assert!(p.x.abs() < 1e-5);
    }

    // ─── S1：`Transform3D::to_affine2d`（2D 投影视图）───────────────────────

    #[test]
    fn test_3d_to_affine2d_identity() {
        assert_eq!(
            Transform3D::identity().to_affine2d(),
            Transform2D::identity()
        );
    }

    #[test]
    fn test_3d_to_affine2d_xy_translation_kept() {
        let t = Transform3D::translate(Vec3::new(10.0, -5.0, 0.0));
        assert_eq!(
            t.to_affine2d(),
            Transform2D::translate(Vec2::new(10.0, -5.0))
        );
    }

    /// 绕 Z 旋转：投影出的 2D 矩阵与 `Transform2D::rotate` 逐位一致
    /// （glam 的 `Mat3::from_rotation_z` / `Mat4::from_rotation_z` 共用 `sin_cos`）。
    #[test]
    fn test_3d_to_affine2d_rotation_z_matches_2d_rotate() {
        for theta in [0.0, 0.1, 2.7, std::f32::consts::FRAC_PI_2, std::f32::consts::PI] {
            let t3 = Transform3D::from_matrix(Mat4::from_rotation_z(theta));
            assert_eq!(
                t3.to_affine2d(),
                Transform2D::rotate(theta),
                "θ = {theta}"
            );
        }
    }

    /// **D4 铁证**：`z` 平移被丢弃 —— xy 部分保持不变，z 不出现在 2D 矩阵里。
    #[test]
    fn test_3d_to_affine2d_drops_z_translation() {
        let t = Transform3D::translate(Vec3::new(1.0, 2.0, 5.0));
        assert_eq!(
            t.to_affine2d(),
            Transform2D::translate(Vec2::new(1.0, 2.0)),
            "z 平移必须被投影丢弃"
        );
    }
}
