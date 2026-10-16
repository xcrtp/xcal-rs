// camera.rs — 相机语义命令与控制器模式（向上契约的一部分：UI → 场景）
//
// 设计要点（B1/B2 补充约束）：
// - 相机**状态**是纯数据；控制器是「消费命令 → 改状态」的纯函数，不接触 egui。
//   egui 原生输入（滚轮、pointer.delta）的读取只在 xcal-egui::host，
//   翻译成本模块的 [`CameraCommand`] 后进入场景。
// - 命令是**语义化**的（"用户做了什么相机操作"），不引用任何相机字段名
//   （center/scale/eye/target…），也不含 egui 类型。
// - 同一个命令在不同 [`CameraMode`] 下语义不同——这就是可插拔控制器的价值：
//   将来加 FPS 模式只需在 [`CameraMode`] 加一个变体并实现其控制器，
//   事件契约（SceneEvent / CameraCommand）零改动。
//
// 本模块是纯数据：不得出现任何 `egui::` 类型（B 阶段验收红线）。

use xc_notion::Vec2;

/// 相机交互模式（可插拔控制器选择）。
///
/// 同一个 [`CameraCommand`] 在不同模式下作用于相机状态的方式不同：
/// - [`PanZoom`](CameraMode::PanZoom)（2D，当前实现）：
///   `Pan` 改取景中心 xy，`Zoom` 改缩放（对应未来正交投影 extent）；
///   `Orbit`/`Dolly` 忽略。
/// - [`Orbit`](CameraMode::Orbit)（3D 轨道，契约预留）：
///   `Orbit` 改 eye（绕 target 的球面角），`Pan` 改 target，
///   `Zoom`/`Dolly` 改 eye↔target 距离。
/// - [`Path`](CameraMode::Path)（路径动画，契约预留）：
///   eye/target 各自由动画曲线驱动（见 BindingTarget::CameraEye），
///   不接受交互命令。
///
/// 新增模式（如 FPS）只需在此加变体 + 实现对应控制器，事件契约不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CameraMode {
    /// 2D 平移/缩放。
    PanZoom,
    /// 3D 轨道环绕（控制器待 3D 交互接入时实现）。
    #[allow(dead_code)]
    Orbit,
    /// 路径驱动，交互命令被忽略（控制器待路径动画接入时启用）。
    #[allow(dead_code)]
    Path,
}

/// 语义化相机命令。
///
/// 由 UI 适配器（host）从原生输入翻译产生，经 [`crate::domain::SceneEvent::Camera`]
/// 投递；相机控制器按当前 [`CameraMode`] 决定命令如何改变相机状态。
///
/// 坐标约定：[`CameraCommand::Pan`] 的 `delta` 为**世界坐标**（Y-up）下的
/// 取景位移量——屏幕→世界的换算（含 Y 翻转）由 host 借助
/// `Camera2D::screen_to_world` 完成，控制器拿到的已是世界空间量。
#[derive(Debug, Clone, Copy)]
pub enum CameraCommand {
    /// 环绕观察：绕取景目标旋转（delta_yaw 水平角、delta_pitch 垂直角，弧度）。
    /// PanZoom 模式下忽略。
    Orbit { delta_yaw: f32, delta_pitch: f32 },
    /// 平移取景：世界坐标（Y-up）下的目标位移量。
    Pan { delta: Vec2 },
    /// 缩放：`factor > 1` 放大、`factor < 1` 缩小。
    /// 作用对象由模式决定（PanZoom → 2D scale；Orbit → 轨道半径）。
    Zoom { factor: f32 },
    /// 推拉：沿视线方向前后移动（`delta` 为无量纲步长，正负方向由控制器约定）。
    /// PanZoom 模式下忽略。
    Dolly { delta: f32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// headless：命令可在无 egui、无窗口的情况下构造。
    #[test]
    fn camera_commands_construct_headless() {
        let _ = CameraCommand::Orbit {
            delta_yaw: 0.05,
            delta_pitch: -0.02,
        };
        let _ = CameraCommand::Pan {
            delta: (1.0, -2.0).into(),
        };
        let _ = CameraCommand::Zoom { factor: 1.1 };
        let _ = CameraCommand::Dolly { delta: 0.5 };
    }
}
