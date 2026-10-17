// camera_drive.rs — 「**谁驱动哪台相机**」的开关（K3 + 补：2D 自由相机）
//
// # 问题（同一条实测结论，两台相机都中）
//
// `BindingApplySystem` 遍历绑定表**每帧无条件写入**目标字段（无 active 判断），而
// `ClockNode.mode = Hold` 把局部时间 clamp 进 `[lo, hi]` ⇒ **时间轴的任何时刻都有值**。
// 于是：
// - 3D 相机：`Curve::Orbit` → `CameraEye` 每帧写 `eye`；
// - 2D 相机：分镜的 `Camera2DFraming` → `center` / `scale` 每帧写取景。
// 用户交互（Orbit/Dolly/Look/Move 或 Pan/Zoom）在 `Input` 相位写入，`SceneSync` 相位的
// `BindingApplySystem` **同帧**就把它抹掉 ⇒ 表现是"拖了没反应"。
//
// # 处置：模式互斥 + **显式让位**
//
// 让位 = 把对应绑定**移出 `BindingTable`**（切回时放回）。这是与既有机制一致的唯一做法
// （绑定是每帧重算的纯函数，"交互优先"在求值链上没有落点）。
//
// | `UiConfig` | 2D 相机 | 3D 相机 |
// |---|---|---|
// | `free_2d_camera = false`（默认） | 分镜绑定**在位** + 模式 `Path` | — |
// | `free_2d_camera = true` | 分镜绑定**移出** + 模式 `PanZoom` | — |
// | `viewport_camera = TwoD`（默认） | — | 自动轨道**在位** + 模式 `Path` |
// | `viewport_camera = ThreeDOrbit/Fps` | — | 自动轨道**移出** + 模式 `Orbit` / `Fps` |
//
// # 为什么这两件事在**同一个 System** 里
//
// `Scheduler::add_system` 在 debug 下做静态借用冲突检查：**同相位内不能有两个
// `BindingTable` 写者**（3D 让位最初就是踩这个坑崩的，见 ARCHITECTURE「K 线实测坑」）。
// 两个相机要让位的是同一张表 ⇒ 必须由**一个** System 原子地完成两台相机的让位判定。

use xc_object::{Camera2DHandle, Camera3DHandle};
use xcal_egui::resources::{UiConfig, ViewportCamera};
use xcal_scene::animation::{Binding, BindingTable};
use xcal_scene::contract::{ObjectId, entity_of};
use xcal_scene::domain::CameraMode;
use xcal_schedule::context::ExecutorContext;
use xcal_schedule::system::System;

/// 按 `UiConfig` 决定两台相机"由分镜驱动还是由交互驱动"，并做绑定让位/复位。
pub struct CameraDriveSystem {
    /// 分镜里指向 2D 相机（`Camera2DFraming`）的绑定（`center` + `scale` 各一条）。
    cam2d: Vec<Binding>,
    /// 自动轨道绑定（`Curve::Orbit` → `CameraEye`）。
    orbit: Binding,
    /// 分镜的 2D 相机绑定当前是否在位。
    cam2d_driven: bool,
    /// 自动轨道绑定当前是否在位。
    orbit_active: bool,
}

impl CameraDriveSystem {
    /// `cam2d`：分镜的 2D 相机绑定（装配时已在表内）；
    /// `orbit`：自动轨道绑定（装配时已在表内）。
    pub fn new(cam2d: Vec<Binding>, orbit: Binding) -> Self {
        Self {
            cam2d,
            orbit,
            cam2d_driven: true,
            orbit_active: true,
        }
    }
}

impl System for CameraDriveSystem {
    type Reads = (UiConfig, Camera2DHandle, Camera3DHandle);
    type Writes = (BindingTable,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let cfg = ctx.get::<UiConfig>().cloned().unwrap_or_default();
        let cam2d = ctx.get::<Camera2DHandle>().copied().unwrap_or_default();
        let cam3d = ctx.get::<Camera3DHandle>().copied().unwrap_or_default();
        // 想要的状态（`true` = 绑定在位 / 由动画驱动）。
        let want_cam2d_driven = !cfg.free_2d_camera;
        let want_orbit = cfg.viewport_camera == ViewportCamera::TwoD;
        // 相机模式（组件）：与"谁驱动"一致 —— 动画驱动 ⇒ `Path`（交互被忽略），
        // 自由 ⇒ `PanZoom`（2D）/ `Orbit` / `Fps`（3D）。
        let mode_2d = if want_cam2d_driven {
            CameraMode::PATH
        } else {
            CameraMode::PAN_ZOOM
        };
        let mode_3d = match cfg.viewport_camera {
            ViewportCamera::TwoD => CameraMode::PATH,
            ViewportCamera::ThreeDOrbit => CameraMode::ORBIT,
            ViewportCamera::ThreeDFps => CameraMode::FPS,
        };

        // 1) 相机模式写进**场景对象**（组件即状态；仅在变化时写，避免每帧 churn）。
        write_mode(ctx, cam2d.0, mode_2d);
        write_mode(ctx, cam3d.0, mode_3d);

        // 2) 绑定让位 / 复位（幂等：状态未变则不动表）。
        let cam2d_changed = want_cam2d_driven != self.cam2d_driven;
        let orbit_changed = want_orbit != self.orbit_active;
        if cam2d_changed || orbit_changed {
            let mut table = ctx.get::<BindingTable>().cloned().unwrap_or_default();
            if cam2d_changed {
                if want_cam2d_driven {
                    for b in &self.cam2d {
                        table.add(b.clone());
                    }
                } else {
                    let clocks: Vec<_> = self.cam2d.iter().map(|b| b.clock).collect();
                    table.bindings.retain(|b| !clocks.contains(&b.clock));
                }
                self.cam2d_driven = want_cam2d_driven;
            }
            if orbit_changed {
                if want_orbit {
                    table.add(self.orbit.clone());
                } else {
                    table.bindings.retain(|b| b.clock != self.orbit.clock);
                }
                self.orbit_active = want_orbit;
            }
            if let Some(t) = ctx.get_mut::<BindingTable>() {
                *t = table;
            }
        }
    }
}

/// 把 `mode` 写到 `id` 指向的相机对象上（对象不存在则静默跳过）。
///
/// 只在**变化时**写组件：`xcal-scene` 的相机模式默认是 `PanZoom`（未挂组件时的取值），
/// 但这里显式落组件，让"谁驱动"这件事在场景数据里可查（而不是靠缺省）。
fn write_mode(ctx: &mut ExecutorContext<'_>, id: ObjectId, mode: CameraMode) {
    let Some(entity) = entity_of(id) else {
        return;
    };
    let changed = ctx
        .world
        .get::<&CameraMode>(entity)
        .map(|m| *m != mode)
        .unwrap_or(true);
    if changed {
        let _ = ctx.world.insert_one(entity, mode);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use xc_object::{Camera3D, Framing2D, Projection};
    use xcal_scene::animation::{Camera2DFraming, CameraEye, SeekKind};
    use xcal_scene::{ClockForest, ClockId, ClockNode, Curve};
    use xcal_schedule::resource::Resources;
    use xcal_schedule::scheduler::{Phase, Scheduler};

    /// 测试脚手架：1 台 2D 相机 + 1 台 3D 相机 + 分镜 2D 绑定×2 + 自动轨道×1。
    struct Fixture {
        sched: Scheduler,
        cam2d: hecs::Entity,
        cam3d: hecs::Entity,
    }

    impl Fixture {
        fn new(cfg: UiConfig) -> Self {
            let mut world = hecs::World::new();
            let cam2d = world.spawn((Framing2D::default(), CameraMode::PATH));
            let cam3d = world.spawn((
                Camera3D::new(
                    glam::Vec3::new(0.0, 0.0, 5.0),
                    glam::Vec3::ZERO,
                    glam::Vec3::Y,
                    Projection::perspective(1.0, 1.0, 0.1, 100.0),
                ),
                CameraMode::PATH,
            ));

            let mut forest = ClockForest::new();
            let c_center = forest.insert(ClockNode::new(None)).unwrap();
            let c_scale = forest.insert(ClockNode::new(None)).unwrap();
            let c_orbit = forest.insert(ClockNode::new(None)).unwrap();
            let cam2d_bindings = vec![
                Binding {
                    clock: c_center,
                    curve: Curve::Keyframes {
                        keys: vec![
                            (0.0, xcal_scene::AnimValue::Vec2((0.0, 0.0).into())),
                            (1.0, xcal_scene::AnimValue::Vec2((1.0, 1.0).into())),
                        ],
                        interpolation: xcal_scene::Interp::Linear,
                    },
                    target: Arc::new(Camera2DFraming {
                        id: xcal_scene::contract::object_id_of(cam2d),
                    }),
                    seek_kind: SeekKind::O1,
                },
                Binding {
                    clock: c_scale,
                    curve: Curve::Keyframes {
                        keys: vec![
                            (0.0, xcal_scene::AnimValue::Scalar(40.0)),
                            (1.0, xcal_scene::AnimValue::Scalar(110.0)),
                        ],
                        interpolation: xcal_scene::Interp::Linear,
                    },
                    target: Arc::new(Camera2DFraming {
                        id: xcal_scene::contract::object_id_of(cam2d),
                    }),
                    seek_kind: SeekKind::O1,
                },
            ];
            let orbit = Binding {
                clock: c_orbit,
                curve: Curve::Orbit {
                    radius: 5.0,
                    axis: glam::Vec3::Y,
                    speed: 0.5,
                    initial_angle: 0.0,
                },
                target: Arc::new(CameraEye {
                    id: xcal_scene::contract::object_id_of(cam3d),
                }),
                seek_kind: SeekKind::O1,
            };

            let mut table = BindingTable::new();
            for b in &cam2d_bindings {
                table.add(b.clone());
            }
            table.add(orbit.clone());

            let mut res = Resources::new();
            res.insert(cfg);
            res.insert(Camera2DHandle(xcal_scene::contract::object_id_of(cam2d)));
            res.insert(Camera3DHandle(xcal_scene::contract::object_id_of(cam3d)));
            res.insert(table);

            let mut sched = Scheduler::new(world, res);
            sched.add_system(Phase::Input, CameraDriveSystem::new(cam2d_bindings, orbit));
            sched.init_resources();
            Self {
                sched,
                cam2d,
                cam3d,
            }
        }

        fn run(&mut self) {
            self.sched.run_frame(0.016);
        }

        fn binding_count(&self) -> usize {
            self.sched
                .resources()
                .get::<BindingTable>()
                .map(|t| t.bindings.len())
                .unwrap_or(0)
        }

        fn mode2d(&self) -> CameraMode {
            *self.sched.world().get::<&CameraMode>(self.cam2d).unwrap()
        }

        fn mode3d(&self) -> CameraMode {
            *self.sched.world().get::<&CameraMode>(self.cam3d).unwrap()
        }

        fn set_cfg(&mut self, f: impl FnOnce(&mut UiConfig)) {
            f(self.sched.resources_mut().get_mut::<UiConfig>().unwrap());
        }
    }

    /// 默认（分镜驱动）：3 条绑定全在位；2D 相机 `Path`（不接受交互）。
    #[test]
    fn default_keeps_storyboard_driving_2d_camera() {
        let mut f = Fixture::new(UiConfig::default());
        f.run();
        assert_eq!(f.binding_count(), 3, "分镜 2 条 + 自动轨道 1 条");
        assert_eq!(f.mode2d(), CameraMode::Path);
        assert_eq!(f.mode3d(), CameraMode::Path);
    }

    /// **开关打开（自由 2D）**：分镜的 2D 相机绑定移出（否则每帧覆盖用户操作），
    /// 2D 相机改 `PanZoom`；3D 侧不受影响。
    #[test]
    fn free_2d_switch_releases_storyboard_camera_bindings() {
        let mut f = Fixture::new(UiConfig::default());
        f.set_cfg(|c| c.free_2d_camera = true);
        f.run();
        assert_eq!(f.binding_count(), 1, "只应剩自动轨道绑定");
        assert_eq!(
            f.mode2d(),
            CameraMode::PAN_ZOOM,
            "自由态下 2D 交互必须被接受"
        );
        assert_eq!(f.mode3d(), CameraMode::Path);
    }

    /// 切回分镜：2 条 2D 绑定**放回**且不重复（幂等）。
    #[test]
    fn switching_back_restores_storyboard_bindings_exactly_once() {
        let mut f = Fixture::new(UiConfig::default());
        f.set_cfg(|c| c.free_2d_camera = true);
        f.run();
        assert_eq!(f.binding_count(), 1);

        f.set_cfg(|c| c.free_2d_camera = false);
        f.run();
        assert_eq!(f.binding_count(), 3, "2 条分镜绑定应放回");
        assert_eq!(f.mode2d(), CameraMode::Path);

        f.run();
        f.run();
        assert_eq!(f.binding_count(), 3, "多帧不得重复添加");
    }

    /// 两个开关**互不干扰**：自由 2D + 3D 轨道交互 ⇒ 0 条绑定。
    #[test]
    fn free_2d_and_3d_orbit_are_independent() {
        let mut f = Fixture::new(UiConfig::default());
        f.set_cfg(|c| {
            c.free_2d_camera = true;
            c.viewport_camera = ViewportCamera::ThreeDOrbit;
        });
        f.run();
        assert_eq!(f.binding_count(), 0, "两个让位都生效");
        assert_eq!(f.mode2d(), CameraMode::PAN_ZOOM);
        assert_eq!(f.mode3d(), CameraMode::Orbit);
    }

    /// 3D 交互（Orbit/Fps）⇒ 自动轨道让位；2D 侧保持分镜驱动。
    #[test]
    fn three_d_interaction_suspends_auto_orbit_only() {
        let mut f = Fixture::new(UiConfig::default());
        f.set_cfg(|c| c.viewport_camera = ViewportCamera::ThreeDFps);
        f.run();
        assert_eq!(f.binding_count(), 2, "只让位自动轨道");
        assert_eq!(f.mode3d(), CameraMode::Fps);
        assert_eq!(f.mode2d(), CameraMode::Path, "2D 仍由分镜驱动");
    }

    /// FPS 目标 ⇒ 模式 `Fps`。
    #[test]
    fn fps_target_sets_fps_mode() {
        let mut f = Fixture::new(UiConfig::default());
        f.set_cfg(|c| c.viewport_camera = ViewportCamera::ThreeDFps);
        f.run();
        assert_eq!(f.mode3d(), CameraMode::Fps);
    }

    /// 句柄失效（无相机对象）⇒ 不 panic；绑定让位仍生效。
    #[test]
    fn missing_cameras_are_noop() {
        let camera = Camera3D::new(
            glam::Vec3::ZERO,
            glam::Vec3::new(0.0, 0.0, -1.0),
            glam::Vec3::Y,
            Projection::perspective(1.0, 1.0, 0.1, 100.0),
        );
        let _ = camera;
        let mut res = Resources::new();
        res.insert(UiConfig {
            free_2d_camera: true,
            ..UiConfig::default()
        });
        res.insert(Camera2DHandle::default());
        res.insert(Camera3DHandle::default());
        let orbit = Binding {
            clock: ClockId(0),
            curve: Curve::Phase { omega: 1.0 },
            target: Arc::new(CameraEye {
                id: xcal_scene::contract::ObjectId::DANGLING,
            }),
            seek_kind: SeekKind::O1,
        };
        let mut table = BindingTable::new();
        table.add(orbit.clone());
        res.insert(table);

        let mut sched = Scheduler::new(hecs::World::new(), res);
        sched.add_system(Phase::Input, CameraDriveSystem::new(Vec::new(), orbit));
        sched.init_resources();
        sched.run_frame(0.016);
        assert_eq!(
            sched
                .resources()
                .get::<BindingTable>()
                .unwrap()
                .bindings
                .len(),
            1
        );
    }
}
