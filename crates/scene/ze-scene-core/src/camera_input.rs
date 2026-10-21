// camera_input.rs — 相机输入（Input 阶段）：消费语义命令，按**模式**驱动场景里的相机对象。
//
// # P1（相机归位）
//
// 相机**状态**是**场景对象**（2D → [`Framing2D`]、3D → [`Camera3D`]），UI 只发
// [`CameraCommand`]；控制器是纯函数，可 headless 测试。
//
// # K0（模式接线）
//
// [`CameraMode`] 从"零消费方的预留变体"变成**真正被消费**：
// 模式作为**相机对象的组件**（支持多相机各自不同模式；未挂则默认 PanZoom），
// [`CameraInputSystem`] 读出模式 → 选对应控制器函数。
//
// # 命令路由（K0 定，实测后收敛的结论）
//
// app 的中央视口里 2D / 3D 图层**占据同一矩形**（同屏合成），"在哪层上拖拽"无法从
// 坐标区分 ⇒ 路由不能靠几何。本实现按**模式能力**路由：把每条命令交给
// "其模式接受该命令"的相机对象；各模式对不属于自己的命令是 **no-op**：
//
// | 模式 | 消费 | 忽略 |
// |---|---|---|
// | `PanZoom`（2D） | `Pan` / `Zoom` | 其余 |
// | `Orbit`（3D） | `Orbit` / `Dolly` | 其余 |
// | `Fps`（3D） | `Look` / `Move` | 其余 |
// | `Path` | —（动画驱动） | 全部 |
//
// `SceneEvent::Camera` 的 `layer` 字段因此**未被消费**（当前只有一个中央视口，
// 且各模式互不重叠）；将来出现"多个 2D 视口各自一台相机"时才需要 layer→相机 映射。

use ze_notion::Vec3;
use crate::object::{Camera3D, FPS_PITCH_LIMIT, FpsView, Framing2D};
use crate::domain::{CameraCommand, CameraMode};

/// 2D 相机缩放范围（单位：像素/世界单位）。
pub const MIN_SCALE: f32 = 5.0;
pub const MAX_SCALE: f32 = 500.0;

/// 2D 控制器（PanZoom 模式）：把语义命令**纯函数地**应用到取景参数。
///
/// 不接触任何 UI 类型；屏幕→世界的位移换算（含 Y 翻转）已由 host 完成，
/// `Pan.delta` 是世界坐标（Y-up）下的取景位移量。
///
/// 语义（与迁移前逐参一致，视觉无回归）：
/// - [`CameraCommand::Pan`]：取景中心 += delta；
/// - [`CameraCommand::Zoom`]：`scale *= factor`，clamp 到 [`MIN_SCALE`, `MAX_SCALE`]。
///   注意 **scale 方向**：1 世界单位 = `scale` 像素，scale 越大物体越大；
/// - 其余命令：2D 模式忽略（3D 控制器消费）。
pub fn apply_pan_zoom(view: &mut Framing2D, command: CameraCommand) {
    match command {
        CameraCommand::Pan { delta } => {
            view.center.x += delta.x;
            view.center.y += delta.y;
        }
        CameraCommand::Zoom { factor } => {
            view.scale = (view.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        }
        _ => {}
    }
}

/// 3D 轨道控制器（K3，`Orbit` 模式）：
/// - [`CameraCommand::Orbit`]：绕 `target` 公转（改 `eye`、**保持半径**），
///   俯仰钳到 `±(π/2 − ε)` 避免翻转；
/// - [`CameraCommand::Dolly`]：沿视线推拉（改 `eye`↔`target` 距离，`target` 不动）；
/// - 其余命令忽略。
pub fn apply_orbit(camera: &mut Camera3D, command: CameraCommand) {
    // 先把 `eye - target` 转成球面角（yaw 绕 Y、pitch 为仰角）。
    // 约定与 `FpsView::forward` 同式：dir = (cos p · cos y, sin p, cos p · sin y)。
    match command {
        CameraCommand::Orbit {
            delta_yaw,
            delta_pitch,
        } => {
            let offset = camera.eye - camera.target;
            let radius = offset.length();
            if radius <= f32::EPSILON {
                return; // 退化：eye 与 target 重合，无轨道可言
            }
            let dir = offset / radius;
            let yaw = dir.z.atan2(dir.x) + delta_yaw;
            let pitch = (dir.y.asin() + delta_pitch).clamp(-FPS_PITCH_LIMIT, FPS_PITCH_LIMIT);
            let (sy, cy) = yaw.sin_cos();
            let (sp, cp) = pitch.sin_cos();
            camera.eye = camera.target + Vec3::new(cp * cy, sp, cp * sy) * radius;
        }
        CameraCommand::Dolly { delta } => {
            let to_target = camera.target - camera.eye;
            let len = to_target.length();
            if len <= 1e-2 {
                return; // 退化/贴身：无推拉空间（也避免 clamp 边界反转）
            }
            // 正值朝目标靠近；夹住步长，避免越过 target（穿到背面）。
            let step = delta.clamp(-len + 1e-3, len - 1e-3);
            camera.eye += to_target / len * step;
        }
        _ => {}
    }
}

/// FPS 控制器（K1，`Fps` 模式）：
/// - [`CameraCommand::Look`]：改朝向（`yaw` 无限制、`pitch` 钳到
///   `±`[`FPS_PITCH_LIMIT`]），随后重写 `target = eye + forward`（`eye` 不动）；
/// - [`CameraCommand::Move`]：沿自身基（右 / 上 / 前）位移，`target` 随 `eye` 平移；
/// - 其余命令忽略。
pub fn apply_fps(view: &mut FpsView, camera: &mut Camera3D, command: CameraCommand) {
    match command {
        CameraCommand::Look {
            delta_yaw,
            delta_pitch,
        } => {
            view.yaw += delta_yaw;
            view.pitch += delta_pitch;
            view.clamp_pitch();
        }
        CameraCommand::Move { delta } => {
            let forward = view.forward();
            // 右 = 前 × 世界上方向（与 forward 不平行由 pitch clamp 保证）。
            let right = forward.cross(Vec3::Y).normalize_or_zero();
            let up = right.cross(forward).normalize_or_zero();
            camera.eye += right * delta.x + up * delta.y + forward * delta.z;
        }
        _ => {}
    }
    // 朝向的权威来源是 `FpsView`：每次都把 `target` 重算为 `eye + forward`。
    camera.target = camera.eye + view.forward();
}

/// 按模式把命令分派给 2D 相机（`PanZoom`；其余模式 no-op）。
pub fn apply_command_2d(view: &mut Framing2D, mode: CameraMode, command: CameraCommand) {
    match mode {
        CameraMode::PanZoom => apply_pan_zoom(view, command),
        // `Orbit` / `Fps` 是 3D 模式；`Path` 由动画驱动 ⇒ 都不消费交互命令。
        _ => {}
    }
}

/// 按模式把命令分派给 3D 相机（`Orbit` / `Fps`；其余模式 no-op）。
pub fn apply_command_3d(
    camera: &mut Camera3D,
    fps: &mut FpsView,
    mode: CameraMode,
    command: CameraCommand,
) {
    match mode {
        CameraMode::Orbit => apply_orbit(camera, command),
        CameraMode::Fps => apply_fps(fps, camera, command),
        _ => {}
    }
}
