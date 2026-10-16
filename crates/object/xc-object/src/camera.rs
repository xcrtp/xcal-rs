// camera.rs — 相机对象（场景对象层）。
//
// **M3-2 归位**：`Camera2D`（原 `xcal-scene/src/types.rs`）与
// `Camera3D` / `Projection`（原 `xcal-scene/src/scene/components/3d.rs`）迁入本层。
// 依据：相机是**用户可见的场景对象**（可被脚本/UI 声明与驱动），
// 且两个相机同居一层才不割裂。
//
// 顺带（计划 §1.5）：`Camera2D.rect` 直接用理念层 [`Rect2D`]，
// `xcal_scene::types::Rect`（视口版的私有字段 + 访问器包装）**已删除** ——
// 它与 `Rect2D` 字段完全相同，是可删的重复定义。

use xc_notion::proj::{directx, look_at_mat4};
use xc_notion::{Mat4, Rect2D, Transform2D, Vec2, Vec3};

// ─── Camera2D ─────────────────────────────────────────────────────────────

/// 2D 相机参数：用于在屏幕坐标系和逻辑坐标系之间转换。
///
/// - 屏幕坐标系：以视口左上角为原点，y 向下（Y-down），单位为像素。
/// - 逻辑坐标系：以 `center` 为相机中心，y 向上（Y-up，数学惯例），单位为世界单位。
///
/// `scale` 方向约定（易踩坑）：**1 世界单位 = `scale` 像素**，scale 越大
/// 物体越大；这与正交投影 `extent`（垂直半高，越大看到越多、物体**越小**）
/// 方向**相反**——薄壳层（Camera2D = Camera3D + Orthographic）负责两者换算。
/// 屏幕 Y-down ↔ 世界 Y-up 的翻转只在 [`Camera2D::to_transform2d`] 的矩阵里
/// 存在一份，其它地方不得再写翻转。
#[derive(Debug, Clone, Copy, derive_new::new)]
pub struct Camera2D {
    /// 逻辑坐标系中相机中心位置
    pub center: Vec2,
    /// 缩放系数：1 世界单位 = `scale` 像素（越大物体越大）
    pub scale: f32,
    /// 屏幕视口矩形（像素）
    pub rect: Rect2D,
}

impl Camera2D {
    /// 屏幕坐标 → 场景（逻辑）坐标。UI 适配器把原生指针事件换算为
    /// 场景语义事件时使用。
    pub fn screen_to_world(&self, p: Vec2) -> Vec2 {
        self.affine_inverse().apply_point(p)
    }

    /// 场景（逻辑）坐标 → 屏幕坐标。
    pub fn world_to_screen(&self, p: Vec2) -> Vec2 {
        self.to_transform2d().apply_point(p)
    }

    /// 屏幕空间的**位移增量**（像素，Y-down）→ 世界空间位移（Y-up）。
    ///
    /// 与 [`Camera2D::screen_to_world`] 同源，只是"点"与"增量"的区别：增量不含视口平移，
    /// 只做缩放与 Y 翻转。供 UI 适配器把指针拖拽的像素位移换算为世界位移
    /// （M4 收尾：换算归对象层，UI 只做事件转发）。
    pub fn screen_delta_to_world(&self, delta: Vec2) -> Vec2 {
        Vec2::new(delta.x / self.scale, -delta.y / self.scale)
    }

    /// 把相机**降级为「世界 → 屏幕」的仿射矩阵**（M2-3）。
    ///
    /// 这是渲染 IR 层唯一被允许看到的相机形态：`ToShape2D` 只吃
    /// [`Transform2D`]，不吃 `Camera2D`（否则 IR 层要依赖场景对象层，成环）。
    ///
    /// 精确形式（与 [`Camera2D::world_to_screen`] 同式）：
    ///
    /// ```text
    /// screen.x = rect.center.x + (world.x - center.x) * scale
    /// screen.y = rect.center.y - (world.y - center.y) * scale      // Y 翻转
    /// ```
    ///
    /// 作为 `Mat3`（列主序）：
    ///
    /// ```text
    /// col0 = [ s,  0, 0 ]
    /// col1 = [ 0, -s, 0 ]
    /// col2 = [ rc.x - c.x·s,  rc.y + c.y·s,  1 ]
    /// ```
    pub fn to_transform2d(&self) -> Transform2D {
        let rc = self.rect.center();
        let s = self.scale;
        let c = self.center;
        Transform2D {
            matrix: xc_notion::Mat3::from_cols(
                Vec3::new(s, 0.0, 0.0),
                Vec3::new(0.0, -s, 0.0),
                Vec3::new(rc.x - c.x * s, rc.y + c.y * s, 1.0),
            ),
        }
    }

    /// 反向（屏幕 → 世界）仿射：正向矩阵的逆。
    fn affine_inverse(&self) -> Transform2D {
        Transform2D {
            matrix: self.to_transform2d().matrix.inverse(),
        }
    }
}

// ─── Projection / Camera3D ────────────────────────────────────────────────

/// 投影方式。
///
/// 两种变体都自带 `aspect`（宽高比），可直接生成投影矩阵；
/// 视口尺寸变化时可用 [`Projection::set_aspect`] 或
/// [`Projection::matrix_with_aspect`] 覆盖。
///
/// 矩阵使用右手系、wgpu/D3D NDC 约定（深度 [0, 1]、Y-up）。
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
    /// - 透视：[`directx::perspective`]
    /// - 正交：[`directx::orthographic`]
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
#[derive(Debug, Clone, Copy, derive_new::new)]
pub struct Camera3D {
    /// 相机位置（世界空间）。
    pub eye: Vec3,
    /// 注视目标点（世界空间）。
    pub target: Vec3,
    /// 上方向（通常为 [`Vec3::Y`]）。
    pub up: Vec3,
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

    fn cam2d() -> Camera2D {
        Camera2D::new(
            Vec2::new(-3.5, 7.25),
            12.0,
            Rect2D::new(Vec2::ZERO, Vec2::new(800.0, 600.0)),
        )
    }

    /// 坐标换算往返：screen_to_world(to_screen(p)) == p（误差 < 1e-5）。
    #[test]
    fn screen_world_roundtrip() {
        let camera = cam2d();
        for wp in [
            Vec2::ZERO,
            Vec2::new(-100.0, 42.0),
            Vec2::new(3.7, -9.9),
        ] {
            let sp = camera.world_to_screen(wp);
            let back = camera.screen_to_world(sp);
            assert!(
                (back.x - wp.x).abs() < 1e-5 && (back.y - wp.y).abs() < 1e-5,
                "world roundtrip failed: {wp:?} -> {sp:?} -> {back:?}"
            );
        }
    }

    /// 屏幕位移增量 → 世界位移：只做缩放与 Y 翻转，不含视口平移（M4 收尾）。
    ///
    /// 与"屏幕两点之差"口径一致：`screen_delta_to_world(b - a) == screen_to_world(b) - screen_to_world(a)`。
    #[test]
    fn screen_delta_to_world_is_scale_and_yflip_only() {
        let camera = cam2d();
        let d = camera.screen_delta_to_world(Vec2::new(24.0, -36.0));
        assert_eq!(d, Vec2::new(24.0 / 12.0, 36.0 / 12.0));

        let a = Vec2::new(100.0, 200.0);
        let b = Vec2::new(124.0, 164.0);
        let by_points = camera.screen_to_world(b) - camera.screen_to_world(a);
        let by_delta = camera.screen_delta_to_world(b - a);
        assert!(
            (by_points - by_delta).length() < 1e-5,
            "增量换算与两点之差必须一致：{by_points:?} vs {by_delta:?}"
        );
    }

    /// 降级矩阵与 `world_to_screen` 一致（容差 1e-2 像素：两式只差浮点结合律）。
    #[test]
    fn transform2d_matches_world_to_screen() {
        let camera = cam2d();
        let t = camera.to_transform2d();
        for wp in [
            Vec2::ZERO,
            Vec2::new(-100.0, 42.0),
            Vec2::new(800.0, -600.0),
        ] {
            let a = camera.world_to_screen(wp);
            let b = t.apply_point(wp);
            assert!(
                (a.x - b.x).abs() < 1e-2 && (a.y - b.y).abs() < 1e-2,
                "仿射与 world_to_screen 不一致: {wp:?} -> {a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn camera3d_view_and_projection() {
        let cam = Camera3D::new(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            Projection::perspective(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 100.0),
        );
        // 世界原点在相机空间中位于 z=-5。
        let origin_view = cam.view_matrix().project_point3(Vec3::ZERO);
        assert!((origin_view.z + 5.0).abs() < 1e-5);
        assert!(cam.view_projection(2.0).is_finite());
    }

    #[test]
    fn projection_aspect_and_matrix() {
        let mut p = Projection::perspective(1.0, 1.0, 0.1, 100.0);
        assert_eq!(p.aspect(), 1.0);
        p.set_aspect(16.0 / 9.0);
        assert!((p.aspect() - 16.0 / 9.0).abs() < 1e-6);

        // 透视下，相机空间 z=-5 的点裁剪空间 w 应等于 5。
        let m = Projection::perspective(std::f32::consts::FRAC_PI_3, 1.0, 0.1, 100.0)
            .matrix_with_aspect(1.0);
        let clip = m * xc_notion::Vec4::new(0.0, 0.0, -5.0, 1.0);
        assert!((clip.w - 5.0).abs() < 1e-5, "w = {}", clip.w);
    }
}
