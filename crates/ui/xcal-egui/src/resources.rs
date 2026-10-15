// resources.rs: egui 层的全局 Resource 类型
//
// B 阶段（UI 解耦）后本文件不得出现任何 egui 类型：
// - EguiFrameContext 已删除——egui 访问收敛到 host.rs::EguiSceneHost；
// - AppCamera 是**纯状态**（center/scale），不含任何输入处理；
//   相机控制器是纯函数 [`AppCamera::apply_command`]（消费 CameraCommand → 改状态），
//   egui 原生输入（滚轮/拖拽）的读取与翻译全部在 host。

use xcal_core::domain::CameraCommand;
use xcal_core::types::{Camera2D, Rect, Vec2};

// ─── AppCamera ─────────────────────────────────────────────────────────────

/// 2D 相机缩放范围（单位：像素/世界单位）。
const MIN_SCALE: f32 = 5.0;
const MAX_SCALE: f32 = 500.0;

/// 应用层 2D 相机（纯状态；UI 侧管理的观察参数，场景侧经 `to_camera_2d` 消费）。
///
/// 状态与控制器分离（B1 补充约束）：
/// - 本结构只持有数据（取景中心 + 缩放），不读任何输入；
/// - 控制器是纯函数 [`apply_command`](Self::apply_command)，
///   消费语义化的 [`CameraCommand`]（PanZoom 模式语义），可 headless 测试；
/// - egui 原生输入 → CameraCommand 的翻译在 `EguiSceneHost`。
///
/// 相机统一预留：将来 2D 相机收敛为 `Camera3D + Orthographic` 时，本结构
/// （或 [`Camera2D`]）成为薄壳，对外仍暴露取景中心与缩放，控制器命令不变。
#[derive(Debug, Clone)]
pub struct AppCamera {
    pub center: Vec2,
    pub scale: f32,
}

impl Default for AppCamera {
    fn default() -> Self {
        Self {
            center: (0.0, 0.0).into(),
            scale: 40.0,
        }
    }
}

impl AppCamera {
    /// 状态 → 本帧 [`Camera2D`] 视图参数（绑定视口矩形）。
    ///
    /// 这是相机统一的接缝点：将来 Camera2D 成为 `Camera3D + Orthographic`
    /// 的薄壳时，2D 相机的**唯一构造处**就是这里，LayerKind / 渲染层无需改动。
    pub fn to_camera_2d(&self, rect: Rect) -> Camera2D {
        Camera2D::new((self.center.x, self.center.y).into(), self.scale, rect)
    }

    /// 相机控制器（PanZoom 模式）：把语义命令纯函数地应用到相机状态。
    ///
    /// 不接触任何 UI 类型；屏幕→世界的位移换算（含 Y 翻转）已由 host 完成，
    /// `Pan.delta` 是世界坐标（Y-up）下的取景位移量。
    ///
    /// 语义（与迁移前 `handle_input` 逐参一致，视觉无回归）：
    /// - [`CameraCommand::Pan`]：取景中心 += delta；
    /// - [`CameraCommand::Zoom`]：`scale *= factor`，clamp 到
    ///   [`MIN_SCALE`, `MAX_SCALE`]。注意 **scale 方向**：1 世界单位 = `scale` 像素，
    ///   scale 越大物体越大；这与未来正交投影 `extent`（垂直半高，越大物体越小）
    ///   方向**相反**，控制器在薄壳层负责换算；
    /// - [`CameraCommand::Orbit`] / [`CameraCommand::Dolly`]：2D 模式忽略
    ///   （3D Orbit 控制器接入时消费）。
    pub fn apply_command(&mut self, command: CameraCommand) {
        match command {
            CameraCommand::Pan { delta } => {
                self.center.x += delta.x;
                self.center.y += delta.y;
            }
            CameraCommand::Zoom { factor } => {
                self.scale = (self.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
            }
            CameraCommand::Orbit { .. } | CameraCommand::Dolly { .. } => {}
        }
    }
}

// ─── TimeState ─────────────────────────────────────────────────────────────

pub use xcal_core::TimeState;

// ─── UiConfig ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UiConfig {
    pub show_grid: bool,
    pub show_field: bool,
    /// S3-1：2D 管线开关（运行期 checkbox，不是编译期 feature）。
    ///
    /// - `false`（默认）：旧路径——`DrawQueue` 经 `EguiPainter` 在 CPU 侧回放；
    /// - `true`：新路径——`Layer2DCallback` 走 wGPU 离屏纹理（MSAA 4x）+ 合成。
    ///
    /// 两条管线并存的意义就是逐帧对比验收（计划 §5.5/§8）；旧管线退役属 S3-4。
    pub gpu_2d: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_grid: true,
            show_field: false,
            gpu_2d: false,
        }
    }
}

// ─── ViewRect ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ViewRect {
    pub rect: Rect,
}

impl Default for ViewRect {
    fn default() -> Self {
        Self {
            rect: Rect::new((0.0, 0.0).into(), (800.0, 600.0).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_core::resource::Resources;

    #[test]
    fn test_app_camera_default() {
        let c = AppCamera::default();
        assert_eq!(c.center.x, 0.0);
        assert_eq!(c.center.y, 0.0);
        assert_eq!(c.scale, 40.0);
    }

    #[test]
    fn test_ui_config_default() {
        let cfg = UiConfig::default();
        assert!(cfg.show_grid);
        assert!(!cfg.show_field);
        // S3-1：默认走旧 2D 管线（开关关闭），保证不开关时画面与迁移前一致。
        assert!(!cfg.gpu_2d);
    }

    #[test]
    fn test_resources_remove() {
        let mut r = Resources::new();
        r.insert(42_i32);
        assert!(r.get::<i32>().is_some());
        let removed = r.remove::<i32>();
        assert_eq!(removed, Some(42));
        assert!(r.get::<i32>().is_none());
    }
}
