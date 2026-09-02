// orbit.rs
//
// 3D 相机轨道动画：让 Camera3D 的 eye 绕 target 沿指定轴做圆周运动。

use glam::{Quat, Vec3};

use crate::scene::components::Camera3D;

/// 轨道相机参数。
///
/// 每帧由动画系统调用 [`OrbitAnim::apply`]：
/// `eye = target + rotate(axis, initial_angle + t * speed) * (0, 0, radius)`。
#[derive(Debug, Clone, Copy, derive_new::new)]
pub struct OrbitAnim {
    /// 角速度（弧度/秒）。
    pub speed: f32,
    /// 轨道半径（eye 到 target 的距离）。
    pub radius: f32,
    /// 旋转轴（世界空间，使用前自动归一化；零向量回退为 Y 轴）。
    pub axis: Vec3,
    /// 初始角度（弧度）。
    pub initial_angle: f32,
}

impl OrbitAnim {
    /// 按全局时间 `t`（秒）更新相机位置。
    pub fn apply(&self, t: f32, camera: &mut Camera3D) {
        let angle = self.initial_angle + t * self.speed;
        let axis = if self.axis.length_squared() > f32::EPSILON {
            self.axis.normalize()
        } else {
            Vec3::Y
        };
        let rotation = Quat::from_axis_angle(axis, angle);
        let offset = rotation * Vec3::new(0.0, 0.0, self.radius);
        camera.eye = camera.target + offset;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::components::Projection;

    fn make_camera() -> Camera3D {
        Camera3D::new(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        )
    }

    #[test]
    fn test_orbit_initial_angle_zero_places_eye_on_z_axis() {
        let mut cam = make_camera();
        let orbit = OrbitAnim::new(0.0, 5.0, Vec3::Y, 0.0);
        orbit.apply(0.0, &mut cam);
        // 角度 0：eye = target + (0,0,radius)
        assert!((cam.eye - Vec3::new(0.0, 0.0, 5.0)).length() < 1e-5);
    }

    #[test]
    fn test_orbit_half_turn_around_y_moves_eye_to_negative_z() {
        let mut cam = make_camera();
        // speed=0.5, initial_angle=0：t=2PI 时角度为 PI，绕 Y 半圈。
        let orbit = OrbitAnim::new(0.5, 5.0, Vec3::Y, 0.0);
        orbit.apply(2.0 * std::f32::consts::PI, &mut cam);
        assert!(
            cam.eye.z < -4.9,
            "eye should be near -z after half turn, got {:?}",
            cam.eye
        );
    }

    #[test]
    fn test_orbit_keeps_radius_constant() {
        let mut cam = make_camera();
        let orbit = OrbitAnim::new(1.0, 7.0, Vec3::Y, 0.3);
        for i in 0..16 {
            orbit.apply(i as f32 * 0.37, &mut cam);
            let r = (cam.eye - cam.target).length();
            assert!((r - 7.0).abs() < 1e-4, "radius should stay 7.0, got {r}");
        }
    }
}
