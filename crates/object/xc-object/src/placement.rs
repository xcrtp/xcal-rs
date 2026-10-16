// placement.rs — 摆放组件（场景对象层）。
//
// **M3-4 归位**：`Transform` 从 `xcal-scene/scene/components.rs` 迁入本层。
// 依据（用户已定）：它是**暴露给用户**的摆放语义（位置 / 旋转 / 缩放），
// 而渲染 IR 基本不暴露它 —— 故属场景对象层，不属 IR 层。
//
// 与理念层 [`xc_notion::Transform3D`] 的区别（同名近义，勿混）：
// - `xc_notion::Transform3D`：**数学仿射变换值**（Mat4 包装 + 组合/应用）；
// - 本类型：**ECS 摆放组件**（position / rotation / scale 三个字段），
//   `matrix()` 才把它折算成数学矩阵。

use xc_notion::{Mat4, Quat, Vec3};

/// 3D 实体变换：平移 + 旋转 + 缩放（glam 原生类型）。
#[derive(Debug, Clone, derive_builder::Builder)]
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
}
