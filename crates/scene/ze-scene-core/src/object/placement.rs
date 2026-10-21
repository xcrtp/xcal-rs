// placement.rs — 摆放组件（场景对象层）。
//
// **M3-4 归位**：`Transform` 从 `zela-scene/scene/components.rs` 迁入本层。
// 依据（用户已定）：它是**暴露给用户**的摆放语义（位置 / 旋转 / 缩放），
// 而渲染 IR 基本不暴露它 —— 故属场景对象层，不属 IR 层。
//
// 与理念层 [`ze_notion::Transform3D`] 的区别（同名近义，勿混）：
// - `ze_notion::Transform3D`：**数学仿射变换值**（Mat4 包装 + 组合/应用）；
// - 本类型：**ECS 摆放组件**（position / rotation / scale 三个字段），
//   `matrix()` 才把它折算成数学矩阵。

use ze_notion::{Mat4, Quat, Transform2D, Transform3D, Vec3};

/// 3D 实体变换：平移 + 旋转 + 缩放（glam 原生类型）。
///
/// **T1：同时承载 2D 语义**（不引入第二个同名类型），见 [`Transform::to_affine2d`]：
/// - `position.xy` ⇒ 2D 位移，`position.z` 在 2D 下**忽略**；
/// - `rotation` 在 2D 下**只取绕 Z 轴的分量**（其余忽略）；
/// - `scale.xy` ⇒ 2D 缩放，`scale.z` 忽略。
#[derive(Debug, Clone, derive_builder::Builder, serde::Serialize, serde::Deserialize)]
pub struct Transform {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Transform {
    /// 组合模型矩阵：scale → rotation → translation（列主序，右乘约定）。
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }

    /// **2D 仿射**（T1）：把摆放组件折算为理念层仿射值（`world → world` 的本地变换）。
    ///
    /// 组合顺序与 [`Transform::matrix`] **同序**（先缩放 → 再旋转 → 再平移）：
    /// `Transform2D::compose(self, other)` 的语义是"先 `self` 后 `other`"（实测：
    /// 其 doc 与 `zela-scene`/`ze-notion` 的 `test_2d_compose_order` 均指
    /// `a.compose(b) = b.matrix * a.matrix` ⇒ 点先被 `a` 作用），故链式写法
    /// `scale.compose(rotate).compose(translate)` 等价于 `T * R * S` —— 与 `matrix()` 一致。
    ///
    /// 2D 语义（用户决策）：`position.z` / `scale.z` 忽略；`rotation` 只取绕 Z 分量。
    ///
    /// **不做父子变换累乘**（`Parent` 仍零消费方，见计划 §2.3 记账）。
    pub fn to_affine2d(&self) -> Transform2D {
        Transform2D::scale(self.scale.truncate())
            .compose(Transform2D::rotate(self.rotation_about_z()))
            .compose(Transform2D::translate(self.position.truncate()))
    }

    /// **3D 仿射**（S1）：折算为理念层 [`Transform3D`] —— 完整 4×4，
    /// 与 [`Transform::matrix`] **逐位一致**（`Transform3D::from_matrix` 原样包装）。
    /// 2D 消费方从它投影（[`Transform3D::to_affine2d`]），世界变换单一真源（铁律 4）。
    pub fn to_affine3d(&self) -> Transform3D {
        Transform3D::from_matrix(self.matrix())
    }

    /// 从 `Quat` 取**绕 Z 轴**的旋转角（弧度）。
    ///
    /// 公式：`θ = atan2(2(w·z + x·y), 1 − 2(y² + z²))`
    /// —— 即把 `q = (x, y, z, w)` 分解为 ZYX 欧拉角时绕 Z 的分量。
    /// 纯 Z 旋转 `q = (0, 0, sin(θ/2), cos(θ/2))` 代入得
    /// `atan2(sin θ, cos θ) = θ`（可直接验算）；退化 / 一般四元数下取"投影到 Z 的等效角"。
    fn rotation_about_z(&self) -> f32 {
        let q = self.rotation;
        (2.0 * (q.w * q.z + q.x * q.y)).atan2(1.0 - 2.0 * (q.y * q.y + q.z * q.z))
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            scale: Vec3::ONE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ze_notion::Vec2;

    #[test]
    fn default_is_identity() {
        assert_eq!(Transform::default().matrix(), Mat4::IDENTITY);
    }

    #[test]
    fn matrix_composes_scale_then_translate() {
        let t = Transform {
            position: Vec3::new(1.0, 2.0, 3.0),
            rotation: Quat::IDENTITY,
            scale: Vec3::splat(2.0),
        };
        // 原点先按 2 倍缩放（仍是原点）再平移 ⇒ 落到 position。
        assert_eq!(t.matrix().transform_point3(Vec3::ZERO), t.position);
        assert_eq!(
            t.matrix().transform_point3(Vec3::X),
            Vec3::new(3.0, 2.0, 3.0)
        );
    }

    /// **T1**：`to_affine2d` 与 `matrix()` 在同序（先缩放 → 再旋转 → 再平移），
    /// 且 `position.z` / `scale.z` 在 2D 下被忽略。
    #[test]
    fn to_affine2d_matches_matrix_order_and_drops_z() {
        // 缩放 2 → 旋转 90°（绕 Z） → 平移 (1, 0)。
        // 点 (1, 0)：缩放 → (2, 0)；旋转 90° → (0, 2)；平移 → (1, 2)。
        let t = Transform {
            position: Vec3::new(1.0, 0.0, 9.0), // z 应被忽略
            rotation: Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
            scale: Vec3::new(2.0, 2.0, 7.0), // z 应被忽略
        };
        let p = t.to_affine2d().apply_point(Vec2::new(1.0, 0.0));
        assert!((p.x - 1.0).abs() < 1e-5, "x = {}", p.x);
        assert!((p.y - 2.0).abs() < 1e-5, "y = {}", p.y);
    }

    /// 恒等 ⇒ 2D 恒等仿射（缺席 `Transform` 的等价形态）。
    #[test]
    fn identity_transform_is_identity_affine2d() {
        assert_eq!(Transform::default().to_affine2d(), Transform2D::identity());
    }

    // ─── S1：`Transform::to_affine3d` ──────────────────────────────────────

    /// `to_affine3d` 与 `matrix()` **逐位一致**（单一真源：Mat4 是唯一世界变换）。
    #[test]
    fn to_affine3d_matrix_is_bitwise_matrix() {
        let t = Transform {
            position: Vec3::new(1.0, -2.0, 3.0),
            rotation: Quat::from_rotation_z(0.7),
            scale: Vec3::new(2.0, 0.5, 1.0),
        };
        assert_eq!(t.to_affine3d().matrix, t.matrix());
    }

    /// 默认 ⇒ 3D 恒等仿射。
    #[test]
    fn default_to_affine3d_is_identity() {
        assert_eq!(
            Transform::default().to_affine3d(),
            Transform3D::identity()
        );
    }

    /// **S1-0 探针固化**：`to_affine3d` 投影出的 2D 子块 vs `to_affine2d`。
    ///
    /// 实测（2026-10-20，Windows）：θ ∈ {0, 0.1, TAU, 2TAU} 逐位相等；
    /// θ = 2.7 有 2 个元素差 2 ulp；θ = π/2、π 的近零分量差（最大 ulp
    /// 6710331，绝对差 < 1e-7）。此处把 fourier 命中的角度断言为**逐位相等**，
    /// 其余按 ulp 上界断言 —— 不写模糊的"接近"。
    #[test]
    fn to_affine3d_xy_subblock_vs_to_affine2d() {
        let tau = std::f32::consts::TAU;
        for theta in [0.0, 0.1, tau, 2.0 * tau] {
            let t = Transform {
                rotation: Quat::from_rotation_z(theta),
                ..Transform::default()
            };
            assert_eq!(
                t.to_affine3d().to_affine2d(),
                t.to_affine2d(),
                "θ = {theta} 必须逐位相等"
            );
        }

        // θ = 2.7：S1-0 实测最大 2 ulp（f32）。
        let t = Transform {
            rotation: Quat::from_rotation_z(2.7),
            ..Transform::default()
        };
        let a = t.to_affine3d().to_affine2d().matrix.to_cols_array();
        let b = t.to_affine2d().matrix.to_cols_array();
        let max_ulp = [0, 1, 3, 4, 6, 7]
            .iter()
            .map(|&i| ulp_diff(a[i], b[i]))
            .max()
            .unwrap();
        assert!(max_ulp <= 2, "θ=2.7 max_ulp 应 ≤ 2，实际 {max_ulp}");
    }

    fn ulp_diff(x: f32, y: f32) -> u32 {
        if x == y {
            return 0;
        }
        let ix = x.to_bits() as i64;
        let iy = y.to_bits() as i64;
        if (ix < 0) != (iy < 0) {
            return u32::MAX;
        }
        (ix - iy).unsigned_abs() as u32
    }
}
