// camera_input.rs — 相机输入（Input 阶段）：消费语义命令，驱动 scene 里的 2D 相机对象。
//
// # P1-1（相机归位）
//
// 相机**状态**原本住在 UI 层（`xcal-egui::AppCamera` Resource）——"相机状态归 UI 持有"
// 是 UI 与场景的耦合点。归位后：相机是**场景对象**（持 [`Framing2D`]），
// UI 只发 [`CameraCommand`]，本模块把命令纯函数地应用到场景对象上。
//
// 与 `xcal-egui::host` 的分工不变（状态与控制器分离，B1 补充约束）：
// - host 把 egui 原生输入（滚轮 / 拖拽）翻译成 [`CameraCommand`]
//   （屏幕→世界的换算、Y 翻转都在那里完成）；
// - 控制器是纯函数 [`apply_pan_zoom`]（消费命令 → 改取景），**可 headless 测试**。

use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;
use xc_object::{Camera2DHandle, Framing2D};

use crate::contract::entity_of;
use crate::domain::{CameraCommand, InputEvents, SceneEvent};

/// 2D 相机缩放范围（单位：像素/世界单位）。
pub const MIN_SCALE: f32 = 5.0;
pub const MAX_SCALE: f32 = 500.0;

/// 相机控制器（PanZoom 模式）：把语义命令**纯函数地**应用到取景参数。
///
/// 不接触任何 UI 类型；屏幕→世界的位移换算（含 Y 翻转）已由 host 完成，
/// `Pan.delta` 是世界坐标（Y-up）下的取景位移量。
///
/// 语义（与迁移前逐参一致，视觉无回归）：
/// - [`CameraCommand::Pan`]：取景中心 += delta；
/// - [`CameraCommand::Zoom`]：`scale *= factor`，clamp 到 [`MIN_SCALE`, `MAX_SCALE`]。
///   注意 **scale 方向**：1 世界单位 = `scale` 像素，scale 越大物体越大；
/// - [`CameraCommand::Orbit`] / [`CameraCommand::Dolly`]：2D 模式忽略（3D 控制器接入时消费）。
pub fn apply_pan_zoom(view: &mut Framing2D, command: CameraCommand) {
    match command {
        CameraCommand::Pan { delta } => {
            view.center.x += delta.x;
            view.center.y += delta.y;
        }
        CameraCommand::Zoom { factor } => {
            view.scale = (view.scale * factor).clamp(MIN_SCALE, MAX_SCALE);
        }
        CameraCommand::Orbit { .. } | CameraCommand::Dolly { .. } => {}
    }
}

/// 相机输入（Input 阶段）：消费相机语义命令，驱动**场景里的 2D 相机对象**。
///
/// 命令来源是 [`InputEvents`]（纯数据契约）——可 headless 构造命令并驱动。
/// 目标相机由 [`Camera2DHandle`] 给出（脚本/装配层 spawn 相机后写入该 Resource）。
/// 当前只有 2D 视口产生相机命令，故全部路由到同一个句柄；
/// 3D Orbit 控制器接入后按 `layer` 分发。
pub struct CameraInputSystem;
impl System for CameraInputSystem {
    type Reads = (InputEvents, Camera2DHandle);
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

        let Some(handle) = ctx.get::<Camera2DHandle>().copied() else {
            return;
        };
        let Some(entity) = entity_of(handle.0) else {
            return;
        };
        let Ok(mut view) = ctx.world.get::<&mut Framing2D>(entity) else {
            return;
        };
        for command in commands {
            apply_pan_zoom(&mut view, command);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::LayerId;
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};
    use xc_scene_contract::ObjectId;

    fn zoom_event(factor: f32) -> SceneEvent {
        SceneEvent::Camera {
            layer: LayerId(0),
            command: CameraCommand::Zoom { factor },
        }
    }

    fn pan_event(dx: f32, dy: f32) -> SceneEvent {
        SceneEvent::Camera {
            layer: LayerId(0),
            command: CameraCommand::Pan {
                delta: (dx, dy).into(),
            },
        }
    }

    /// 建一个含 2D 相机对象的场景 + 句柄 Resource。
    fn world_with_camera() -> (hecs::World, Camera2DHandle) {
        let mut world = hecs::World::new();
        let entity = world.spawn((Framing2D::default(),));
        (world, Camera2DHandle(crate::contract::object_id_of(entity)))
    }

    /// headless 决定性测试：构造含 Zoom 命令的 InputEvents，直接跑本 System，
    /// 断言场景里相机对象的 `scale` 按预期变化（状态在 **scene**，不在 UI）。
    #[test]
    fn camera_input_applies_zoom_to_scene_object() {
        let (world, handle) = world_with_camera();
        let entity = entity_of(handle.0).unwrap();

        let mut resources = Resources::new();
        resources.insert(handle);
        resources.insert(InputEvents::from_events(vec![zoom_event(1.1)]));

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::Input, CameraInputSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let scale = sched.world().get::<&Framing2D>(entity).unwrap().scale;
        assert!((scale - 44.0).abs() < 1e-4, "scale = {scale}");
    }

    /// 缩放 clamp：factor 过大时 scale 不超过上限 500。
    #[test]
    fn camera_input_zoom_is_clamped() {
        let (world, handle) = world_with_camera();
        let entity = entity_of(handle.0).unwrap();

        let mut resources = Resources::new();
        resources.insert(handle);
        resources.insert(InputEvents::from_events(vec![zoom_event(1000.0)]));

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::Input, CameraInputSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let scale = sched.world().get::<&Framing2D>(entity).unwrap().scale;
        assert!((scale - 500.0).abs() < 1e-4, "scale = {scale}");
    }

    /// 平移：Pan 命令直接叠加到取景中心（世界坐标，Y-up）。
    #[test]
    fn camera_input_applies_drag_to_scene_object() {
        let (world, handle) = world_with_camera();
        let entity = entity_of(handle.0).unwrap();

        let mut resources = Resources::new();
        resources.insert(handle);
        resources.insert(InputEvents::from_events(vec![pan_event(2.0, -1.0)]));

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::Input, CameraInputSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let view = sched.world().get::<&Framing2D>(entity).unwrap();
        assert!((view.center.x - 2.0).abs() < 1e-5, "x = {}", view.center.x);
        assert!((view.center.y - (-1.0)).abs() < 1e-5, "y = {}", view.center.y);
    }

    /// 无事件帧不改变相机状态。
    #[test]
    fn camera_input_noop_without_events() {
        let (world, handle) = world_with_camera();
        let entity = entity_of(handle.0).unwrap();

        let mut resources = Resources::new();
        resources.insert(handle);
        resources.insert(InputEvents::default());

        let mut sched = Scheduler::new(world, resources);
        sched.add_system(Phase::Input, CameraInputSystem);
        sched.init_resources();
        sched.run_frame(0.016);

        let view = sched.world().get::<&Framing2D>(entity).unwrap();
        assert_eq!(view.center.x, 0.0);
        assert_eq!(view.scale, 40.0);
    }

    /// 句柄指向已销毁对象 ⇒ 静默跳过（不 panic）。
    #[test]
    fn stale_handle_is_noop() {
        let mut resources = Resources::new();
        resources.insert(Camera2DHandle(ObjectId::DANGLING));
        resources.insert(InputEvents::from_events(vec![zoom_event(1.1)]));

        let mut sched = Scheduler::new(hecs::World::new(), resources);
        sched.add_system(Phase::Input, CameraInputSystem);
        sched.init_resources();
        sched.run_frame(0.016);
    }

    /// 控制器纯函数：Orbit/Dolly 在 PanZoom 模式下被忽略，状态不变。
    #[test]
    fn panzoom_controller_ignores_orbit_and_dolly() {
        let mut view = Framing2D::default();
        apply_pan_zoom(
            &mut view,
            CameraCommand::Orbit {
                delta_yaw: 1.0,
                delta_pitch: 0.5,
            },
        );
        apply_pan_zoom(&mut view, CameraCommand::Dolly { delta: 3.0 });
        assert_eq!(view.center.x, 0.0);
        assert_eq!(view.center.y, 0.0);
        assert_eq!(view.scale, 40.0);
    }
}
