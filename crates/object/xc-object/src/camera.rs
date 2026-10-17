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
use xc_scene_contract::{ObjectId, Scene, SceneRead};

/// [`Camera2D::fit_bounds`] 的退化阈值：包围盒某方向尺寸 ≤ 该值即视为退化
/// （保持基底 `scale`）。与 `xcal-app` 原手写式的 `size.x > 1e-3` 同值。
const FIT_MIN_EXTENT: f32 = 1e-3;

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
#[derive(Debug, Clone, Copy, derive_new::new, serde::Serialize, serde::Deserialize)]
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

    /// 让给定**世界空间包围盒**适配本相机的视口矩形（`self.rect`），返回**新相机**。
    ///
    /// 语义：
    /// - `center` = 包围盒中心；
    /// - `scale` = `margin × min(视口宽 / 盒宽, 视口高 / 盒高)`
    ///   —— 按"刚好装下"的方向定比例（另一方向留白），再乘 `margin`；
    /// - `rect` 沿用 `self.rect`（本方法只负责取景，不改视口）；
    /// - **退化保护**：盒宽 / 盒高非有限或 ≤ [`FIT_MIN_EXTENT`] 时**保持 `self.scale`**
    ///   —— 调用方可用一个带"兜底 scale"的相机作基底。
    ///
    /// 为什么 `margin` 是显式参数：它是**视觉选择**（0.8 = 内容占视口 80%），
    /// 不是几何常量，必须由调用方声明。
    ///
    /// 与 `xcal-app` 原手写式的关系：原式 `scale = 视口宽 × 0.8 / 盒宽` 只按宽适配；
    /// 在**盒宽 ≥ 盒高**时 `min` 恰取宽度方向 ⇒ 两者逐位一致（内容纹理相机的实际情形）。
    pub fn fit_bounds(&self, bbox: Rect2D, margin: f32) -> Camera2D {
        let box_size = bbox.size();
        let view_size = self.rect.size();
        let fits = box_size.x.is_finite()
            && box_size.y.is_finite()
            && box_size.x > FIT_MIN_EXTENT
            && box_size.y > FIT_MIN_EXTENT
            && view_size.x.is_finite()
            && view_size.y.is_finite();
        let scale = if fits {
            margin * (view_size.x / box_size.x).min(view_size.y / box_size.y)
        } else {
            self.scale
        };
        Camera2D::new(bbox.center(), scale, self.rect)
    }
}

// ─── Framing2D / Camera2DHandle（P1-2 + P2）────────────────────────────────

/// 2D 相机**取景参数**（不含视口矩形）——相机作为**场景对象**的形态。
///
/// 为什么与 [`Camera2D`] 拆开（P1-2 方案 a）：`Camera2D` 里绑着 `rect`（屏幕视口
/// 矩形）⇒ "同一个相机渲染到 800×600 与 1920×1080"会变成"改相机"，句柄就不是纯引用。
/// 取景（`center` / `scale`）属于**场景对象**，视口矩形属于**渲染目标**：
/// 前者进 scene（可被 handle 引用、可被动画驱动），后者在录制时由渲染层绑定。
#[derive(Debug, Clone, Copy, PartialEq, derive_new::new, serde::Serialize, serde::Deserialize)]
pub struct Framing2D {
    /// 逻辑坐标系中的取景中心。
    pub center: Vec2,
    /// 缩放：1 世界单位 = `scale` 像素（方向约定同 [`Camera2D::scale`]）。
    pub scale: f32,
}

impl Default for Framing2D {
    /// 与 UI 侧原默认一致：世界原点居中、scale = 40（像素/世界单位）。
    fn default() -> Self {
        Self {
            center: Vec2::ZERO,
            scale: 40.0,
        }
    }
}

impl Framing2D {
    /// 绑定**渲染目标矩形** → 本帧 [`Camera2D`]（唯一取景 → 相机的绑定点）。
    pub fn to_camera_2d(&self, rect: Rect2D) -> Camera2D {
        Camera2D::new(self.center, self.scale, rect)
    }
}

/// 2D 相机句柄（P2，**引用型**）：指向 scene 里的相机对象（持有 [`Framing2D`]）。
///
/// 句柄是"隔离**时间**与**渲染目标**的一层膜"：它只携带身份，取景参数从 scene 读、
/// 视口矩形由渲染目标传入 ⇒ 时间进不了渲染层，同一句柄换目标只是画面大小不同。
///
/// 与 `ObjectId` 的关系：句柄**存 `ObjectId` 而非 `hecs::Entity`**（对象层不依赖
/// hecs），失效检测可接 [`ObjectId::is_stale`](xc_scene_contract::ObjectId::is_stale)。
///
/// `Default` = `ObjectId::DANGLING`（永不匹配任何对象的哨兵）：调用方在"尚未装配相机"
/// 时可先用它占位，解析必然失败并跳过本帧渲染（不 panic）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Camera2DHandle(pub ObjectId);

impl Camera2DHandle {
    /// 解析为本帧渲染用 [`Camera2D`]：scene 里的取景参数（[`Framing2D`]）绑定目标 `rect`。
    ///
    /// 对象不存在 / 未挂 `Framing2D` ⇒ `None`（句柄失效，调用方跳过本帧渲染，不 panic）。
    pub fn resolve<S: SceneRead>(&self, scene: &S, rect: Rect2D) -> Option<Camera2D> {
        scene.with::<Framing2D, _>(self.0, |f| f.to_camera_2d(rect))
    }
}

/// 在场景里创建 2D 相机对象，返回其句柄（脚本取景入口）。
///
/// 返回 `None` = `Framing2D` 未在实现的组件登记表中（见 `xcal-scene::contract`）。
pub fn spawn_camera2d<S: Scene>(scene: &mut S, framing: Framing2D) -> Option<Camera2DHandle> {
    scene.spawn_dyn(vec![Box::new(framing)]).map(Camera2DHandle)
}

// ─── FpsView / Camera3DHandle（K1 / K3）────────────────────────────────────

/// FPS 相机的**朝向状态**（K1，组件）：显式存 `yaw` / `pitch`（弧度）。
///
/// 为什么不直接增量旋转 `eye - target`：增量应用到方向向量会**累积漂移**，
/// 且 `pitch` 无法 clamp（会翻转）——只有显式存 yaw / pitch 才能正确限制俯仰范围。
///
/// 与 [`Camera3D`] 的分工：本组件是"朝向的权威来源"，`Camera3D.eye/target` 是它的
/// 派生产物（控制器每次改完朝向就重写 `target = eye + forward`）。
#[derive(Debug, Clone, Copy, PartialEq, derive_new::new, serde::Serialize, serde::Deserialize)]
pub struct FpsView {
    /// 绕世界 Y 轴的偏航角（弧度）。
    pub yaw: f32,
    /// 俯仰角（弧度），限制在 ±[`FPS_PITCH_LIMIT`]。
    pub pitch: f32,
}

/// FPS 俯仰角上限（弧度）：`π/2 − ε`，避免视线与世界上方向共线（翻转/奇异）。
pub const FPS_PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

impl FpsView {
    /// 从相机的当前朝向导出 `yaw` / `pitch`（保证接管时画面不跳变）。
    ///
    /// 朝向约定（与 [`FpsView::forward`] 同式）：`yaw = atan2(dir.z, dir.x)`、
    /// `pitch = asin(dir.y)`。
    pub fn from_camera(camera: &Camera3D) -> Self {
        let dir = (camera.target - camera.eye).normalize_or_zero();
        Self {
            yaw: dir.z.atan2(dir.x),
            pitch: dir.y.asin().clamp(-FPS_PITCH_LIMIT, FPS_PITCH_LIMIT),
        }
    }

    /// 朝向单位向量：`(cos p · cos y, sin p, cos p · sin y)`。
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(cp * cy, sp, cp * sy)
    }

    /// 把 `pitch` 钳到 `±`[`FPS_PITCH_LIMIT`]（转头时调用）。
    pub fn clamp_pitch(&mut self) {
        self.pitch = self.pitch.clamp(-FPS_PITCH_LIMIT, FPS_PITCH_LIMIT);
    }
}

/// 3D 相机句柄（K3，**引用型**）：指向 scene 里的 3D 相机对象（持 [`Camera3D`]）。
///
/// 与 [`Camera2DHandle`] 对称：只携带身份，不使用 `hecs::Entity`
/// （`entity_of` 转换在 `xcal-scene::contract`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Camera3DHandle(pub ObjectId);

/// 在场景里创建 3D 相机对象，返回其句柄。
///
/// 可选挂 [`FpsView`]（FPS 模式的朝向状态）与 `CameraMode`（由调用方另行写入组件）。
pub fn spawn_camera3d<S: Scene>(scene: &mut S, camera: Camera3D) -> Option<Camera3DHandle> {
    scene.spawn_dyn(vec![Box::new(camera)]).map(Camera3DHandle)
}

// ─── Projection / Camera3D ────────────────────────────────────────────────

/// 投影方式。
///
/// 两种变体都自带 `aspect`（宽高比），可直接生成投影矩阵；
/// 视口尺寸变化时可用 [`Projection::set_aspect`] 或
/// [`Projection::matrix_with_aspect`] 覆盖。
///
/// 矩阵使用右手系、wgpu/D3D NDC 约定（深度 [0, 1]、Y-up）。
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, Copy, derive_new::new, serde::Serialize, serde::Deserialize)]
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
        for wp in [Vec2::ZERO, Vec2::new(-100.0, 42.0), Vec2::new(3.7, -9.9)] {
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

    /// `fit_bounds`：`center` = 盒中心；`scale` = `margin × min(视口/盒)`。
    ///
    /// **等价性判据（本轮唯一视觉风险点）**：盒宽 ≥ 盒高 时 `min` 取宽度方向，
    /// 与 `xcal-app` 原手写式 `视口宽 × 0.8 / 盒宽` 逐位一致。
    #[test]
    fn fit_bounds_matches_app_previous_width_fit() {
        let viewport = Rect2D::new(Vec2::ZERO, Vec2::new(512.0, 512.0));
        let base = Camera2D::new(Vec2::ZERO, 40.0, viewport);
        // 盒 100 × 40（宽 ≥ 高，公式包围盒的实际情形）。
        let bbox = Rect2D::new(Vec2::new(10.0, 20.0), Vec2::new(110.0, 60.0));
        let cam = base.fit_bounds(bbox, 0.8);

        assert!(
            (cam.scale - 512.0 * 0.8 / 100.0).abs() < 1e-5,
            "scale = {}",
            cam.scale
        );
        assert!((cam.center.x - 60.0).abs() < 1e-5 && (cam.center.y - 40.0).abs() < 1e-5);
        // 视口沿用基底相机的 rect。
        assert_eq!(cam.rect, viewport);

        // 盒高 > 盒宽 ⇒ 取高度方向（保证完整可见，另一方向留白）。
        let tall = Rect2D::new(Vec2::ZERO, Vec2::new(40.0, 100.0));
        assert!((base.fit_bounds(tall, 0.8).scale - 512.0 * 0.8 / 100.0).abs() < 1e-5);
    }

    /// `fit_bounds` 的退化保护：空盒 / 零宽 / 非有限 ⇒ 保持基底 `scale`。
    ///
    /// 与 `xcal-app` 原手写式的退化兜底（基底 scale = 40）同口径。
    #[test]
    fn fit_bounds_keeps_base_scale_for_degenerate_bbox() {
        let viewport = Rect2D::new(Vec2::ZERO, Vec2::new(512.0, 512.0));
        let base = Camera2D::new(Vec2::ZERO, 40.0, viewport);
        for bbox in [
            Rect2D::default(),                                               // 空盒
            Rect2D::new(Vec2::new(1.0, 1.0), Vec2::new(1.0, 5.0)),           // 零宽
            Rect2D::new(Vec2::new(0.0, 0.0), Vec2::new(f32::INFINITY, 1.0)), // 非有限
        ] {
            let cam = base.fit_bounds(bbox, 0.8);
            assert_eq!(cam.scale, 40.0, "退化盒应保持基底 scale：{bbox:?}");
            assert_eq!(cam.center, bbox.center(), "退化盒仍以盒中心取景");
        }
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
