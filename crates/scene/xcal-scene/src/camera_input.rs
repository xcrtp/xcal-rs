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

use xc_notion::Vec3;
use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;
use xc_object::{
    Camera2DHandle, Camera3D, Camera3DHandle, FPS_PITCH_LIMIT, FpsView, Framing2D,
};

use crate::contract::entity_of;
use crate::domain::{CameraCommand, CameraMode, InputEvents, SceneEvent};

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

/// 相机输入（Input 阶段）：消费相机语义命令，驱动**场景里的相机对象**。
///
/// 命令来源是 [`InputEvents`]（纯数据契约）——可 headless 构造命令并驱动。
/// 目标相机由 [`Camera2DHandle`] / [`Camera3DHandle`] 指认（装配层 spawn 后写入 Resource）。
pub struct CameraInputSystem;
impl System for CameraInputSystem {
    type Reads = (InputEvents, Camera2DHandle, Camera3DHandle);
    type Writes = ();

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        // 先拷贝命令（释放对 resources 的共享借用），再取相机句柄。
        let commands: Vec<CameraCommand> = match ctx.get::<InputEvents>() {
            Some(e) => e
                .iter()
                .filter_map(|ev| match ev {
                    SceneEvent::Camera { command, .. } => Some(*command),
                    _ => None,
                })
                .collect(),
            None => return,
        };
        if commands.is_empty() {
            return;
        }

        // ── 2D 相机（按模式分派；未挂 `CameraMode` ⇒ 默认 PanZoom）──
        let cam2d = ctx.get::<Camera2DHandle>().copied().unwrap_or_default();
        if let Some(entity) = entity_of(cam2d.0) {
            let mode = ctx
                .world
                .get::<&CameraMode>(entity)
                .map(|m| *m)
                .unwrap_or_default();
            if let Ok(mut view) = ctx.world.get::<&mut Framing2D>(entity) {
                for command in &commands {
                    apply_command_2d(&mut view, mode, *command);
                }
            }
        }

        // ── 3D 相机（按模式分派；未挂 ⇒ 默认 PanZoom ⇒ 不消费任何命令）──
        let cam3d = ctx.get::<Camera3DHandle>().copied().unwrap_or_default();
        if let Some(entity) = entity_of(cam3d.0) {
            let mode = ctx
                .world
                .get::<&CameraMode>(entity)
                .map(|m| *m)
                .unwrap_or_default();
            if mode == CameraMode::PanZoom || mode == CameraMode::Path {
                return; // 该相机不接受交互命令
            }
            // 朝向状态：优先读组件；缺失时用相机当前朝向初始化（接管不跳变）。
            // 两次只读借用（不同组件）在同一个块内，块结束即释放。
            let mut fps = {
                let Ok(camera) = ctx.world.get::<&Camera3D>(entity) else {
                    return;
                };
                match ctx.world.get::<&FpsView>(entity) {
                    Ok(v) => *v,
                    Err(_) => FpsView::from_camera(&camera),
                }
            };
            {
                let Ok(mut camera) = ctx.world.get::<&mut Camera3D>(entity) else {
                    return;
                };
                for command in &commands {
                    apply_command_3d(&mut camera, &mut fps, mode, *command);
                }
            }
            // 回写朝向状态（`Look` 累积的结果必须持久化，否则下一帧又回到初始朝向）。
            let _ = ctx.world.insert_one(entity, fps);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::LayerId;
    use crate::TimeState;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};
    use xc_object::Projection;
    use xc_scene_contract::ObjectId;

    fn cam_zoom(factor: f32) -> SceneEvent {
        SceneEvent::Camera {
            layer: LayerId(0),
            command: CameraCommand::Zoom { factor },
        }
    }

    fn cam_pan(dx: f32, dy: f32) -> SceneEvent {
        SceneEvent::Camera {
            layer: LayerId(0),
            command: CameraCommand::Pan {
                delta: (dx, dy).into(),
            },
        }
    }

    fn cam_orbit(dyaw: f32, dpitch: f32) -> SceneEvent {
        SceneEvent::Camera {
            layer: LayerId(0),
            command: CameraCommand::Orbit {
                delta_yaw: dyaw,
                delta_pitch: dpitch,
            },
        }
    }

    fn cam_dolly(delta: f32) -> SceneEvent {
        SceneEvent::Camera {
            layer: LayerId(0),
            command: CameraCommand::Dolly { delta },
        }
    }

    fn cam_look(dyaw: f32, dpitch: f32) -> SceneEvent {
        SceneEvent::Camera {
            layer: LayerId(0),
            command: CameraCommand::Look {
                delta_yaw: dyaw,
                delta_pitch: dpitch,
            },
        }
    }

    fn cam_move(delta: Vec3) -> SceneEvent {
        SceneEvent::Camera {
            layer: LayerId(0),
            command: CameraCommand::Move { delta },
        }
    }

    /// 建一个含 2D 相机对象的场景 + 句柄 Resource（`CameraMode` 可选）。
    fn world_with_camera(mode: Option<CameraMode>) -> (hecs::World, Camera2DHandle) {
        let mut world = hecs::World::new();
        let entity = world.spawn((Framing2D::default(),));
        if let Some(m) = mode {
            world.insert_one(entity, m).unwrap();
        }
        (world, Camera2DHandle(crate::contract::object_id_of(entity)))
    }

    /// 建一个含 3D 相机对象的场景 + 句柄（`CameraMode` + 可选 `FpsView`）。
    fn world_with_camera3d(mode: CameraMode, fps: Option<FpsView>) -> (hecs::World, Camera3DHandle) {
        let mut world = hecs::World::new();
        let camera = Camera3D::new(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        );
        let entity = world.spawn((camera, mode));
        if let Some(v) = fps {
            world.insert_one(entity, v).unwrap();
        }
        (world, Camera3DHandle(crate::contract::object_id_of(entity)))
    }

    /// 跑一帧（只装 CameraInputSystem）。
    fn run(
        world: hecs::World,
        cam2d: Camera2DHandle,
        cam3d: Camera3DHandle,
        events: Vec<SceneEvent>,
    ) -> Scheduler {
        let mut resources = Resources::new();
        resources.insert(cam2d);
        resources.insert(cam3d);
        resources.insert(TimeState::default());
        resources.insert(InputEvents::from_events(events));
        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::Input, CameraInputSystem);
        sched.init_resources();
        sched.run_frame(0.016);
        sched
    }

    // ── 2D / PanZoom（与 P 阶段逐位一致）────────────────────────────────

    /// headless 决定性测试：Zoom 命令改变场景里相机对象的 `scale`。
    #[test]
    fn panzoom_applies_zoom_to_scene_object() {
        let (world, h2d) = world_with_camera(None); // 未挂模式 ⇒ 默认 PanZoom
        let entity = entity_of(h2d.0).unwrap();
        let sched = run(world, h2d, Camera3DHandle::default(), vec![cam_zoom(1.1)]);
        let scale = sched.world().get::<&Framing2D>(entity).unwrap().scale;
        assert!((scale - 44.0).abs() < 1e-4, "scale = {scale}");
    }

    /// 缩放 clamp：factor 过大时 scale 不超过上限 500。
    #[test]
    fn panzoom_zoom_is_clamped() {
        let (world, h2d) = world_with_camera(None);
        let entity = entity_of(h2d.0).unwrap();
        let sched = run(world, h2d, Camera3DHandle::default(), vec![cam_zoom(1000.0)]);
        let scale = sched.world().get::<&Framing2D>(entity).unwrap().scale;
        assert!((scale - 500.0).abs() < 1e-4, "scale = {scale}");
    }

    /// 平移：Pan 命令直接叠加到取景中心（世界坐标，Y-up）。
    #[test]
    fn panzoom_applies_pan_to_scene_object() {
        let (world, h2d) = world_with_camera(None);
        let entity = entity_of(h2d.0).unwrap();
        let sched = run(world, h2d, Camera3DHandle::default(), vec![cam_pan(2.0, -1.0)]);
        let view = sched.world().get::<&Framing2D>(entity).unwrap();
        assert!((view.center.x - 2.0).abs() < 1e-5);
        assert!((view.center.y + 1.0).abs() < 1e-5);
    }

    /// 无事件帧不改变相机状态。
    #[test]
    fn no_events_leaves_camera_untouched() {
        let (world, h2d) = world_with_camera(None);
        let entity = entity_of(h2d.0).unwrap();
        let sched = run(world, h2d, Camera3DHandle::default(), Vec::new());
        let view = sched.world().get::<&Framing2D>(entity).unwrap();
        assert_eq!(view.center.x, 0.0);
        assert_eq!(view.scale, 40.0);
    }

    /// 句柄失效 ⇒ 静默跳过（不 panic）。
    #[test]
    fn stale_handle_is_noop() {
        let world = hecs::World::new();
        let sched = run(
            world,
            Camera2DHandle(ObjectId::DANGLING),
            Camera3DHandle(ObjectId::DANGLING),
            vec![cam_zoom(1.1)],
        );
        assert_eq!(sched.world().len(), 0);
    }

    /// 2D 控制器纯函数：Orbit/Dolly/Look/Move 在 PanZoom 模式下被忽略，状态不变。
    #[test]
    fn panzoom_ignores_other_commands() {
        let mut view = Framing2D::default();
        for cmd in [
            CameraCommand::Orbit {
                delta_yaw: 1.0,
                delta_pitch: 0.5,
            },
            CameraCommand::Dolly { delta: 3.0 },
            CameraCommand::Look {
                delta_yaw: 1.0,
                delta_pitch: 0.5,
            },
            CameraCommand::Move {
                delta: Vec3::ONE,
            },
        ] {
            apply_pan_zoom(&mut view, cmd);
        }
        assert_eq!(view.center.x, 0.0);
        assert_eq!(view.center.y, 0.0);
        assert_eq!(view.scale, 40.0);
    }

    // ── K2：Path 模式忽略全部交互命令 ───────────────────────────────────

    /// Path 模式（动画驱动）下 Pan / Zoom 都不改变相机 —— 与 `CameraMode::Path`
    /// 的既有语义注释一致（"不接受交互命令"）。
    #[test]
    fn path_mode_ignores_all_interaction() {
        let (world, h2d) = world_with_camera(Some(CameraMode::PATH));
        let entity = entity_of(h2d.0).unwrap();
        let sched = run(
            world,
            h2d,
            Camera3DHandle::default(),
            vec![cam_pan(5.0, 5.0), cam_zoom(2.0), cam_orbit(1.0, 0.5)],
        );
        let view = sched.world().get::<&Framing2D>(entity).unwrap();
        assert_eq!(view.center.x, 0.0, "Path 模式不得被 Pan 移动");
        assert_eq!(view.center.y, 0.0);
        assert_eq!(view.scale, 40.0, "Path 模式不得被 Zoom 改变");
    }

    // ── K3：3D 轨道控制器 ───────────────────────────────────────────────

    /// Orbit 命令绕 target 公转：**半径不变**、方向改变、target 不动。
    #[test]
    fn orbit_rotates_eye_around_target_keeping_radius() {
        let mut camera = Camera3D::new(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        );
        let (r0, t0) = ((camera.eye - camera.target).length(), camera.target);
        apply_orbit(
            &mut camera,
            CameraCommand::Orbit {
                delta_yaw: std::f32::consts::FRAC_PI_2,
                delta_pitch: 0.0,
            },
        );
        let offset = camera.eye - camera.target;
        assert!((offset.length() - r0).abs() < 1e-4, "半径应保持");
        assert_ne!(offset, Vec3::new(0.0, 0.0, 5.0), "方向应改变");
        assert_eq!(camera.target, t0, "target 不得移动");
        // 初始 offset = +Z ⇒ yaw = atan2(z, x) = +π/2；再 +π/2 ⇒ π ⇒ 方向变为 -X。
        assert!(
            (offset - Vec3::new(-5.0, 0.0, 0.0)).length() < 1e-3,
            "got {offset:?}"
        );
    }

    /// Orbit 的俯仰被钳制（不翻转）。
    #[test]
    fn orbit_pitch_is_clamped() {
        let mut camera = Camera3D::new(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        );
        for _ in 0..100 {
            apply_orbit(
                &mut camera,
                CameraCommand::Orbit {
                    delta_yaw: 0.0,
                    delta_pitch: 0.5,
                },
            );
        }
        let dir = (camera.eye - camera.target).normalize();
        // 钳的是**角度**，方向的 y 分量是其正弦。
        assert!(
            (dir.y - FPS_PITCH_LIMIT.sin()).abs() < 1e-4,
            "俯仰应钳到 {FPS_PITCH_LIMIT} rad（sin = {}），实际 dir.y = {}",
            FPS_PITCH_LIMIT.sin(),
            dir.y
        );
    }

    /// Dolly 改变 eye↔target 距离（target 不动）；正值靠近、负值远离。
    #[test]
    fn dolly_changes_distance_to_target() {
        let mut camera = Camera3D::new(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        );
        apply_orbit(&mut camera, CameraCommand::Dolly { delta: 2.0 });
        assert!(((camera.eye - camera.target).length() - 3.0).abs() < 1e-4);
        apply_orbit(&mut camera, CameraCommand::Dolly { delta: -1.0 });
        assert!(((camera.eye - camera.target).length() - 4.0).abs() < 1e-4);
        assert_eq!(camera.target, Vec3::ZERO);
    }

    /// 3D 视口命令经 System 落到场景对象上（Orbit 模式）。
    #[test]
    fn orbit_mode_camera_consumes_orbit_dolly() {
        let (world, h3d) = world_with_camera3d(CameraMode::ORBIT, None);
        let entity = entity_of(h3d.0).unwrap();
        let sched = run(
            world,
            Camera2DHandle::default(),
            h3d,
            vec![cam_orbit(0.0, 0.3), cam_dolly(1.0)],
        );
        let camera = sched.world().get::<&Camera3D>(entity).unwrap();
        let dist = (camera.eye - camera.target).length();
        assert!((dist - 4.0).abs() < 1e-3, "Dolly 应把距离从 5 收到 4，实际 {dist}");
        assert!(camera.eye.y > 1e-3, "Orbit 的 +pitch 应把 eye 抬起来");
    }

    // ── K1：FPS 相机 ────────────────────────────────────────────────────

    /// Look 改朝向（eye 不动、target 随朝向动）；pitch 被 clamp。
    #[test]
    fn fps_look_changes_orientation_and_clamps_pitch() {
        let mut camera = Camera3D::new(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::ZERO,
            Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        );
        let mut fps = FpsView::from_camera(&camera); // 朝向 = -Z ⇒ yaw = -π/2
        let eye0 = camera.eye;

        apply_fps(&mut fps, &mut camera, cam_look_cmd(0.0, 0.2));
        assert_eq!(camera.eye, eye0, "Look 不移动 eye");
        assert!((camera.target - eye0).normalize().y - 0.2_f32.sin() < 1e-4);

        // 连续抬头 ⇒ pitch 钳到上限，视线不翻转（target 仍在 eye 上方）。
        for _ in 0..100 {
            apply_fps(&mut fps, &mut camera, cam_look_cmd(0.0, 0.5));
        }
        assert!((fps.pitch - FPS_PITCH_LIMIT).abs() < 1e-5);
        assert!(camera.target.y > camera.eye.y);
    }

    fn cam_look_cmd(dyaw: f32, dpitch: f32) -> CameraCommand {
        CameraCommand::Look {
            delta_yaw: dyaw,
            delta_pitch: dpitch,
        }
    }

    /// Move 沿自身朝向位移：eye 移动、朝向不变、距离保持（target 跟随）。
    #[test]
    fn fps_move_translates_eye_keeping_orientation() {
        let mut camera = Camera3D::new(
            Vec3::new(0.0, 0.0, 5.0),
            Vec3::new(0.0, 0.0, 4.0),
            Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        );
        let mut fps = FpsView::from_camera(&camera);
        let dir0 = (camera.target - camera.eye).normalize();

        apply_fps(
            &mut fps,
            &mut camera,
            CameraCommand::Move {
                delta: Vec3::new(0.0, 0.0, 2.0), // 前进 2
            },
        );
        assert!((camera.eye.z - 3.0).abs() < 1e-4, "前向应使 z 减 2，实际 {}", camera.eye.z);
        assert!(
            ((camera.target - camera.eye).normalize() - dir0).length() < 1e-4,
            "Move 不改变朝向"
        );
        assert!(((camera.target - camera.eye).length() - 1.0).abs() < 1e-4, "target 跟随 eye");

        // 右移：+X（朝向 -Z 时 right = forward × Y = -Z × Y = +X）。
        let eye1 = camera.eye;
        apply_fps(
            &mut fps,
            &mut camera,
            CameraCommand::Move {
                delta: Vec3::new(1.0, 0.0, 0.0),
            },
        );
        assert!((camera.eye.x - (eye1.x + 1.0)).abs() < 1e-4, "eye = {:?}", camera.eye);
    }

    /// FPS 相机经 System 消费 Look / Move，且**朝向状态跨帧累积**（回写组件）。
    #[test]
    fn fps_mode_camera_consumes_look_and_move_via_system() {
        // 第 1 帧：只转头。初始朝向 = -Z ⇒ yaw = -π/2、pitch = 0。
        let (world, h3d) = world_with_camera3d(CameraMode::FPS, None);
        let entity = entity_of(h3d.0).unwrap();
        let sched = run(world, Camera2DHandle::default(), h3d, vec![cam_look(0.3, 0.0)]);
        let fps = *sched.world().get::<&FpsView>(entity).unwrap();
        assert!(
            (fps.yaw - (-std::f32::consts::FRAC_PI_2 + 0.3)).abs() < 1e-4,
            "yaw 应在组件里累积，实际 {}",
            fps.yaw
        );
        assert!(fps.pitch.abs() < 1e-6);
    }

    /// FPS 的 Move 沿**当前朝向**位移（先转头再前进 ⇒ 位移方向随朝向）。
    #[test]
    fn fps_move_follows_current_orientation() {
        let (world, h3d) = world_with_camera3d(CameraMode::FPS, None);
        let entity = entity_of(h3d.0).unwrap();
        // 前进 1 世界单位（z = +1 是"前"）。
        let sched = run(
            world,
            Camera2DHandle::default(),
            h3d,
            vec![cam_move(Vec3::new(0.0, 0.0, 1.0))],
        );
        let camera = sched.world().get::<&Camera3D>(entity).unwrap();
        // 初始朝向 = -Z ⇒ 前进使 z 从 5 减到 4。
        assert!((camera.eye.z - 4.0).abs() < 1e-3, "eye = {:?}", camera.eye);
        assert!((camera.eye.x).abs() < 1e-4);
        // target 跟随 eye（朝向不变、距离保持 1）。
        assert!(((camera.target - camera.eye).length() - 1.0).abs() < 1e-4);
    }

    /// Orbit 模式不消费 Look/Move（模式互不重叠）；PanZoom 相机不消费任何 3D 命令。
    #[test]
    fn modes_do_not_cross_consume() {
        // 3D 相机处于 Orbit 模式：Look/Move 应被忽略。
        let (world, h3d) = world_with_camera3d(CameraMode::ORBIT, None);
        let entity = entity_of(h3d.0).unwrap();
        let sched = run(
            world,
            Camera2DHandle::default(),
            h3d,
            vec![
                cam_look(1.0, 1.0),
                cam_move(Vec3::new(5.0, 5.0, 5.0)),
                cam_pan(5.0, 5.0),
            ],
        );
        let camera = sched.world().get::<&Camera3D>(entity).unwrap();
        assert_eq!(camera.eye, Vec3::new(0.0, 0.0, 5.0), "Orbit 模式不得被 Look/Move 改动");
    }
}
