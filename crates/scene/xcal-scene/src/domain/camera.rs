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

use xc_notion::{Vec2, Vec3};

/// 相机交互模式（可插拔控制器选择）。
///
/// 同一个 [`CameraCommand`] 在不同模式下作用于相机状态的方式不同：
/// - [`PanZoom`](CameraMode::PanZoom)（2D）：`Pan` 改取景中心 xy、`Zoom` 改缩放；
///   `Orbit`/`Dolly`/`Look`/`Move` 忽略。
/// - [`Orbit`](CameraMode::Orbit)（3D 轨道，K3 实现）：`Orbit` 绕 `target` 转（改 eye、
///   保持半径）、`Dolly` 沿视线推拉（改 eye↔target 距离）；其余忽略。
/// - [`Fps`](CameraMode::Fps)（3D 第一人称，K1）：`Look` 改朝向、`Move` 沿自身朝向位移；
///   其余忽略。
/// - [`Path`](CameraMode::Path)（路径动画，K2）：eye/target 各自由动画曲线驱动
///   （见 `xcal_scene::animation::CameraEye` / `Camera2DFraming`），**不接受交互命令**。
///
/// **模式是相机对象的组件**（K0）：支持多相机各自不同模式；未挂该组件时取
/// [`Default`] = `PanZoom`（不 panic）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum CameraMode {
    /// 2D 平移/缩放（默认）。
    #[default]
    PanZoom,
    /// 3D 轨道环绕（Orbit/Dolly）。
    Orbit,
    /// 3D 第一人称（Look/Move）。
    Fps,
    /// 路径驱动，交互命令被忽略。
    Path,
}

impl CameraMode {
    /// 供**外部 crate** 取值的关联常量。
    ///
    /// 为什么需要：本枚举是 `#[non_exhaustive]`，下游不能直接写 `CameraMode::Orbit`
    /// （非定义 crate 无法构造其变体）；这些常量给出一条稳定的构造路径。
    pub const PAN_ZOOM: Self = Self::PanZoom;
    pub const ORBIT: Self = Self::Orbit;
    pub const FPS: Self = Self::Fps;
    pub const PATH: Self = Self::Path;
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
    /// 仅 `Orbit` 模式消费。
    Orbit { delta_yaw: f32, delta_pitch: f32 },
    /// 平移取景：世界坐标（Y-up）下的目标位移量。仅 `PanZoom` 消费。
    Pan { delta: Vec2 },
    /// 缩放：`factor > 1` 放大、`factor < 1` 缩小。仅 `PanZoom` 消费
    /// （3D 的"远近"用 [`CameraCommand::Dolly`]）。
    Zoom { factor: f32 },
    /// 推拉：沿视线方向前后移动（`delta` 为世界单位步长，正值朝 `target`）。
    /// 仅 `Orbit` 模式消费。
    Dolly { delta: f32 },
    /// **原地转头**（K1，仅 `Fps` 模式消费）：`delta_yaw` 绕世界 Y 轴、
    /// `delta_pitch` 绕自身右轴（弧度）。
    ///
    /// 与 [`CameraCommand::Orbit`] 的区别：`Orbit` 是**绕目标公转**（eye 动、target 不动），
    /// `Look` 是**自身朝向变化**（eye 不动、target 随朝向动）。
    Look { delta_yaw: f32, delta_pitch: f32 },
    /// **沿自身朝向位移**（K1，仅 `Fps` 模式消费）。
    ///
    /// `delta` 的分量含义：`x` = 右、`y` = 上、`z` = 前（世界单位）。
    Move { delta: Vec3 },
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
        // K1：FPS 的两个命令同样可在无 egui / 无窗口下构造。
        let _ = CameraCommand::Look {
            delta_yaw: 0.05,
            delta_pitch: -0.02,
        };
        let _ = CameraCommand::Move {
            delta: Vec3::new(1.0, 0.0, -0.5),
        };
        // 模式默认 PanZoom（未挂 `CameraMode` 组件时的行为）。
        assert_eq!(CameraMode::default(), CameraMode::PanZoom);
        assert_eq!(CameraMode::ORBIT, CameraMode::Orbit);
        assert_eq!(CameraMode::FPS, CameraMode::Fps);
        assert_eq!(CameraMode::PATH, CameraMode::Path);
    }
}
