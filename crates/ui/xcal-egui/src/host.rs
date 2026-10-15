// host.rs — EguiSceneHost：egui ↔ 场景的唯一适配层（B 阶段，计划 §6.3/§6.4）
//
// 职责：
// 1. `build_scene_view` — 场景 → UI：把场景状态声明为纯数据 [`SceneView`]；
// 2. `collect_events`   — UI → 场景：把 egui 原生输入翻译为 [`SceneEvent`]，
//    视口坐标已换算为场景坐标（经由 `Camera2D::screen_to_world`）；
// 3. 面板 chrome — 控制面板 / 时间控制 / 参数面板：纯 UI 绘制，控件交互以
//    [`SceneEvent`]（按 `WidgetId` 标识）写入 [`InputEvents`]，由场景侧
//    System（CameraInput / TimeControl / ParamPanel）消费；
// 4. `update_view_rect` — 面板布局完成后写入中央视口矩形（布局信息本就属 UI，
//    由 host 承担原 ViewRectUpdateSystem 的职责）；
// 5. `present` — 把 `DrawQueue`(2D) 与 `DrawQueue3D` 的录制内容提交到
//    egui painter（承担原 RenderSubmitSystem / RenderSubmit3DSystem 的职责）。
//
// 这是唯一同时接触 egui 与场景两者的模块；将来迁移 UI 只需另写一个 host，
// 场景逻辑一行不动。本文件之外的 xcal-egui 模块不得出现 egui 类型（B 阶段红线）。
//
// 帧内调用顺序（xcal-app::App::ui）：
// `build_scene_view` → `begin_frame` → `Scheduler::run_frame` → `present`。

use egui::Panel;

use xcal_core::animation::{BindingTable, BindingTarget};
use xcal_core::director_state::DirectorState;
use xcal_core::domain::{
    ids, CameraCommand, ContentHandle, InputEvents, LayerDescriptor, LayerId, LayerKind, SceneEvent,
    SceneView,
};
use xcal_core::resource::Resources;
use xcal_core::scene::components::Camera3D;
use xcal_core::types::Rect;
use xcal_core::{Curve, TimeState};
use xcal_render::{DrawQueue, DrawQueue3D};
use xcal_render_backends::{EguiPainter, Layer2DCallback, Layer3DCallback};
use xc_geometry::Rect2D;

use crate::resources::{AppCamera, UiConfig, ViewRect};

pub struct EguiSceneHost;

impl EguiSceneHost {
    // ─── 帧编排 ────────────────────────────────────────────────────────────

    /// 帧首调用（在 `Scheduler::run_frame` 之前）：
    /// 1. 收集视口原始输入 → 场景事件；
    /// 2. 绘制面板 chrome，控件事件并入同一队列；
    /// 3. 整帧替换写入 [`InputEvents`]（自动清空上一帧的事件）；
    /// 4. 面板布局完成后更新 [`ViewRect`]。
    ///
    /// 注意：ViewRect 必须在 run_frame 之前写入（而非 present 时）——
    /// Prepare/RenderCommit 阶段的 GridBuild/SceneRecord 当帧就要用它，
    /// 否则窗口 resize 时内容会慢一帧。
    pub fn begin_frame(ui: &mut egui::Ui, view: &SceneView, res: &mut Resources) {
        let mut events = InputEvents::from_events(Self::collect_events(ui, view));
        events.extend(Self::draw_control_panel(ui, res));
        events.extend(Self::draw_time_panel(ui, res));
        events.extend(Self::draw_param_panel(ui, res));
        res.insert(events);
        Self::update_view_rect(ui, res);
    }

    // ─── 场景 → UI：视图声明 ───────────────────────────────────────────────

    /// 从场景状态构建本帧视图声明（纯数据，无 egui 类型）。
    ///
    /// - 2D 图层：相机来自 `AppCamera` + `ViewRect`（UI 侧管理的观察参数）；
    /// - 3D 图层：相机取 World 中第一个 `Camera3D` 组件；
    /// - `ContentHandle` 为 B 阶段占位（S3 统一管线接管后指向 RenderQueue 分片）。
    pub fn build_scene_view(world: &hecs::World, res: &Resources) -> SceneView {
        let rect = res
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);

        let mut view = SceneView::default();

        if let Some(cam) = res.get::<AppCamera>() {
            view.layers.push(LayerDescriptor {
                id: LayerId(0),
                kind: LayerKind::Viewport2D {
                    camera: cam.to_camera_2d(rect),
                },
                content: ContentHandle::OPAQUE,
            });
        }

        // hecs 0.11：单组件查询的 iter 产出 `&Camera3D`（无 Entity）。
        if let Some(camera3d) = world.query::<&Camera3D>().iter().next() {
            view.layers.push(LayerDescriptor {
                id: LayerId(1),
                kind: LayerKind::Viewport3D { camera: *camera3d },
                content: ContentHandle::OPAQUE,
            });
        }

        view
    }

    // ─── UI → 场景：原始输入翻译 ───────────────────────────────────────────

    /// 把 egui 原生输入翻译为相机语义命令与视口拾取事件。
    ///
    /// 视口交互路由到 SceneView 中第一个 2D 图层（B 阶段单视口；
    /// 多视口命中路由在 S3 统一管线中实现）。
    /// 相机操作翻译为语义化 [`CameraCommand`]（不引用相机字段名）：
    /// - 滚轮 → `Zoom { factor }`，factor = 1 + scroll × 0.001（不锚定光标，
    ///   与旧行为一致；clamp 由 PanZoom 控制器负责）；
    /// - 右键/中键拖拽 → `Pan { delta }`：屏幕位移在此换算为**世界坐标**
    ///   （含 Y 翻转：dx → -dx/scale，dy → +dy/scale），控制器只做加法。
    pub fn collect_events(ui: &egui::Ui, view: &SceneView) -> Vec<SceneEvent> {
        let Some(LayerDescriptor {
            id: layer,
            kind: LayerKind::Viewport2D { camera },
            ..
        }) = view
            .layers
            .iter()
            .find(|l| matches!(l.kind, LayerKind::Viewport2D { .. }))
        else {
            return Vec::new();
        };

        // 指针是否落在视口矩形内（悬停/点击只在视口内上报）。
        let in_view = |p: egui::Pos2| {
            p.x >= camera.rect.min().x
                && p.x <= camera.rect.max().x
                && p.y >= camera.rect.min().y
                && p.y <= camera.rect.max().y
        };

        let mut events = Vec::new();
        ui.input(|i| {
            // 滚轮缩放（语义命令；与旧行为一致：任意位置滚轮均生效）。
            let scroll = i.smooth_scroll_delta.y;
            if scroll != 0.0 {
                events.push(SceneEvent::Camera {
                    layer: *layer,
                    command: CameraCommand::Zoom {
                        factor: 1.0 + scroll * 0.001,
                    },
                });
            }

            // 右键/中键拖拽 → 平移（屏幕位移在此换算为世界坐标位移量，
            // Y 翻转集中在这一处换算，控制器拿到的是 Y-up 世界量）。
            if i.pointer.secondary_down() || i.pointer.middle_down() {
                let d = i.pointer.delta();
                if d != egui::Vec2::ZERO {
                    events.push(SceneEvent::Camera {
                        layer: *layer,
                        command: CameraCommand::Pan {
                            delta: (-d.x / camera.scale, d.y / camera.scale).into(),
                        },
                    });
                }
            }

            // 悬停 / 点击（视口内；拾取类事件，非相机命令）。
            if let Some(pos) = i.pointer.latest_pos() {
                if in_view(pos) {
                    events.push(SceneEvent::ViewportHover {
                        layer: *layer,
                        scene_pos: camera.screen_to_world(pos.into()),
                    });
                    if i.pointer.primary_clicked() {
                        events.push(SceneEvent::ViewportClick {
                            layer: *layer,
                            scene_pos: camera.screen_to_world(pos.into()),
                        });
                    }
                }
            }
        });
        events
    }

    // ─── 面板 chrome（控件事件由场景侧 System 消费） ───────────────────────

    /// 控制面板（原 ControlPanelSystem 的 UI chrome，移出 ECS）。
    ///
    /// `UiConfig` 是纯 UI 状态（显示开关），由 chrome 直接修改；
    /// Camera 读数只展示。返回空事件集（预留未来 Toggle 事件）。
    fn draw_control_panel(ui: &mut egui::Ui, res: &mut Resources) -> Vec<SceneEvent> {
        let time = res.get::<TimeState>().map(|t| t.elapsed).unwrap_or(0.0);
        let camera = res.get::<AppCamera>().cloned().unwrap_or_default();
        let mut ui_config = res.get::<UiConfig>().cloned().unwrap_or_default();

        Panel::left("controls")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.heading("数学可视化");
                ui.label(format!("t = {:.3}", time));
                ui.separator();

                ui.checkbox(&mut ui_config.show_grid, "显示网格");
                ui.checkbox(&mut ui_config.show_field, "显示向量场");

                ui.separator();
                // S3-1 验收开关：切换 2D 管线（旧 CPU 回放 ↔ 新 GPU 离屏 MSAA），
                // 用于同一画面下逐帧对比两条管线的输出是否一致。
                ui.checkbox(&mut ui_config.gpu_2d, "GPU 2D 管线");

                ui.separator();
                ui.label("Camera");
                ui.label(format!(
                    "center: ({:.2}, {:.2})",
                    camera.center.x, camera.center.y
                ));
                ui.label(format!("scale: {:.1}", camera.scale));
            });

        if let Some(cfg) = res.get_mut::<UiConfig>() {
            *cfg = ui_config;
        }
        Vec::new()
    }

    /// 时间控制面板（原 TimeControlSystem 的 UI chrome）。
    ///
    /// 只负责绘制与采集控件事件（播放/暂停、速度、seek），
    /// DirectorState 的实际变更由场景侧 `TimeControlSystem` 消费事件完成。
    fn draw_time_panel(ui: &mut egui::Ui, res: &mut Resources) -> Vec<SceneEvent> {
        let time = res.get::<TimeState>().map(|t| t.elapsed).unwrap_or(0.0);
        let director = res.get::<DirectorState>().cloned().unwrap_or_default();

        let mut events = Vec::new();
        Panel::left("time_control")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.heading("时间控制");
                ui.label(format!("t = {:.3}", time));
                ui.separator();

                // 播放/暂停切换
                let label = if director.director.playing { "⏸ 暂停" } else { "▶ 播放" };
                if ui.button(label).clicked() {
                    events.push(SceneEvent::Button { id: ids::PLAY_PAUSE });
                }

                // 速度滑块
                ui.separator();
                ui.label("速度");
                let mut speed_f = director.director.speed as f32;
                if ui
                    .add(egui::Slider::new(&mut speed_f, -3.0..=3.0))
                    .changed()
                {
                    events.push(SceneEvent::Slider {
                        id: ids::SPEED,
                        value: speed_f,
                    });
                }

                // 时间轴拖动条（seek）——只在用户真的拖完才发事件，
                // 场景侧 step_by 会把 global_t 对齐到 0.1 网格，速度慢时 knob 卡 0。
                ui.separator();
                ui.label("时间轴（seek）");
                let mut t_f = director.director.global_t as f32;
                let resp = ui.add(egui::Slider::new(&mut t_f, -10.0..=30.0).text("t"));
                if resp.drag_stopped() {
                    events.push(SceneEvent::Slider {
                        id: ids::SEEK,
                        value: t_f,
                    });
                }
            });
        events
    }

    /// 参数面板（原 ParamPanelSystem 的 UI chrome）。
    ///
    /// 按 (curve, target) 类型匹配展示绑定（与 A5 一致，不依赖插入顺序），
    /// 滑块交互以 `Slider { id }` 事件上报，由场景侧 `ParamPanelSystem` 消费。
    fn draw_param_panel(ui: &mut egui::Ui, res: &mut Resources) -> Vec<SceneEvent> {
        let table = res.get::<BindingTable>().cloned().unwrap_or_default();

        let mut events = Vec::new();
        Panel::right("params")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.heading("动画参数");
                ui.separator();

                for binding in table.bindings.iter() {
                    match (&binding.curve, &binding.target) {
                        (Curve::Sin { base, amp, omega }, BindingTarget::CircleRadius(_)) => {
                            ui.label("圆半径");
                            for (id, label, value, range, step) in [
                                (ids::CIRCLE_BASE, "base", *base, 0.0..=3.0, 0.1),
                                (ids::CIRCLE_AMP, "amp", *amp, 0.0..=2.0, 0.05),
                                (ids::CIRCLE_OMEGA, "omega", *omega, 0.0..=8.0, 0.1),
                            ] {
                                let mut v = value;
                                if ui
                                    .add(egui::Slider::new(&mut v, range).text(label).step_by(step))
                                    .changed()
                                {
                                    events.push(SceneEvent::Slider { id, value: v });
                                }
                            }
                            ui.separator();
                        }
                        (Curve::Phase { omega }, BindingTarget::Phase(_)) => {
                            ui.label("函数曲线相位");
                            let mut v = *omega;
                            if ui
                                .add(
                                    egui::Slider::new(&mut v, 0.0..=5.0)
                                        .text("omega")
                                        .step_by(0.1),
                                )
                                .changed()
                            {
                                events.push(SceneEvent::Slider {
                                    id: ids::PHASE_OMEGA,
                                    value: v,
                                });
                            }
                            ui.separator();
                        }
                        (
                            Curve::Orbit {
                                radius,
                                speed,
                                initial_angle,
                                ..
                            },
                            BindingTarget::CameraEye(_),
                        ) => {
                            ui.label("3D 轨道相机");
                            for (id, label, value, range, step) in [
                                (ids::ORBIT_RADIUS, "radius", *radius, 1.0..=20.0, 0.5),
                                (ids::ORBIT_SPEED, "speed", *speed, -2.0..=2.0, 0.1),
                                (ids::ORBIT_ANGLE, "angle", *initial_angle, -3.14..=3.14, 0.1),
                            ] {
                                let mut v = value;
                                if ui
                                    .add(egui::Slider::new(&mut v, range).text(label).step_by(step))
                                    .changed()
                                {
                                    events.push(SceneEvent::Slider { id, value: v });
                                }
                            }
                            ui.separator();
                        }
                        _ => {}
                    }
                }
            });
        events
    }

    // ─── 布局信息 ──────────────────────────────────────────────────────────

    /// 面板布局完成后写入中央视口矩形（原 ViewRectUpdateSystem 的职责）。
    pub fn update_view_rect(ui: &egui::Ui, res: &mut Resources) {
        let rect = Rect::from(ui.available_rect_before_wrap());
        if let Some(vr) = res.get_mut::<ViewRect>() {
            vr.rect = rect;
        }
    }

    // ─── 内容提交 ──────────────────────────────────────────────────────────

    /// 把本帧录制的内容提交到 egui painter（原 RenderSubmitSystem /
    /// RenderSubmit3DSystem 的职责）。在 `Scheduler::run_frame` 之后调用。
    ///
    /// - 2D：`DrawQueue` 经 `EguiPainter` 回放到视口矩形；
    /// - 3D：取 SceneView 声明的 3D 图层相机 + `DrawQueue3D` 快照，
    ///   封装为 egui_wgpu 回调（与 2D 同层、shape 顺序在后 → 绘制于 2D 之上，
    ///   clip 自动限定在中央视口，侧边面板不受影响）。
    pub fn present(ui: &mut egui::Ui, view: &SceneView, res: &mut Resources) {
        let view_rect = res
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let egui_rect = view_rect.into_egui_rect();

        // 2D 内容先画。两条管路由 `UiConfig::gpu_2d` 运行期开关二选一
        // （S3-1：新管线不是替换而是并存，验收靠对比两条管线）。
        let gpu_2d = res.get::<UiConfig>().map(|c| c.gpu_2d).unwrap_or(false);
        if gpu_2d {
            // 新路径：`Layer2DCallback` 在 `prepare` 把队列渲染进离屏 MSAA 纹理，
            // 在 `paint` 合成回主 pass（场景侧 System 的 Reads/Writes 不变，
            // 开关只影响 present 这一处）。
            if let Some(queue) = res.get::<DrawQueue>() {
                if !queue.is_empty() {
                    let viewport = Rect2D::new(*view_rect.min(), *view_rect.max());
                    let callback = Layer2DCallback::new(viewport, queue);
                    ui.painter_at(egui_rect).add(egui::epaint::Shape::Callback(
                        egui_wgpu::Callback::new_paint_callback(egui_rect, callback),
                    ));
                }
            }
        } else if let Some(queue) = res.get_mut::<DrawQueue>() {
            // 旧路径：`DrawQueue` 经 `EguiPainter` 在 CPU 侧回放（特征化基线）。
            let painter = ui.painter_at(egui_rect);
            queue.execute(&EguiPainter::new(&painter));
        }

        // 3D 内容覆盖其上（仅当场景声明了 3D 图层且队列非空）。
        let Some(LayerDescriptor {
            kind: LayerKind::Viewport3D { camera },
            ..
        }) = view
            .layers
            .iter()
            .find(|l| matches!(l.kind, LayerKind::Viewport3D { .. }))
        else {
            return;
        };
        let Some(queue3d) = res.get::<DrawQueue3D>() else {
            return;
        };
        if queue3d.is_empty() {
            return;
        }

        let aspect = egui_rect.width() / egui_rect.height().max(1.0);
        let callback = Layer3DCallback::new(*camera, aspect, queue3d);
        let painter = ui.painter_at(egui_rect);
        painter.add(egui::epaint::Shape::Callback(
            egui_wgpu::Callback::new_paint_callback(egui_rect, callback),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_core::resource::Resources;
    use xcal_core::scene::components::{Camera3D, Projection};

    /// headless：无任何 egui 上下文即可构建 SceneView。
    #[test]
    fn build_scene_view_declares_2d_and_3d_layers() {
        let mut world = hecs::World::new();
        world.spawn((
            Camera3D::new(
                glam::Vec3::new(0.0, 0.0, 5.0),
                glam::Vec3::ZERO,
                glam::Vec3::Y,
                Projection::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0),
            ),
        ));

        let mut res = Resources::new();
        res.insert(AppCamera::default());
        res.insert(ViewRect::default());

        let view = EguiSceneHost::build_scene_view(&world, &res);
        assert_eq!(view.layers.len(), 2);
        assert!(matches!(view.layers[0].kind, LayerKind::Viewport2D { .. }));
        assert!(matches!(view.layers[1].kind, LayerKind::Viewport3D { .. }));
        assert_eq!(view.layers[0].content, ContentHandle::OPAQUE);
    }

    /// 无 3D 相机实体时只声明 2D 图层；无 AppCamera 资源时无 2D 图层。
    #[test]
    fn build_scene_view_skips_missing_layers() {
        let world = hecs::World::new();
        let mut res = Resources::new();
        res.insert(AppCamera::default());

        let view = EguiSceneHost::build_scene_view(&world, &res);
        assert_eq!(view.layers.len(), 1);
        assert!(matches!(view.layers[0].kind, LayerKind::Viewport2D { .. }));

        let res_empty = Resources::new();
        let view = EguiSceneHost::build_scene_view(&world, &res_empty);
        assert!(view.layers.is_empty());
    }

    /// 2D 图层相机与 GridBuild 的构造方式一致：AppCamera → Camera2D。
    #[test]
    fn scene_view_2d_camera_matches_app_camera() {
        let world = hecs::World::new();
        let mut res = Resources::new();
        res.insert(AppCamera::default());
        res.insert(ViewRect::default());

        let rect = res.get::<ViewRect>().unwrap().rect;
        let expected = res.get::<AppCamera>().unwrap().to_camera_2d(rect);

        let view = EguiSceneHost::build_scene_view(&world, &res);
        let Some(LayerKind::Viewport2D { camera }) = Some(view.layers[0].kind) else {
            panic!("expected 2D layer");
        };
        assert!((camera.center.x - expected.center.x).abs() < 1e-5);
        assert!((camera.center.y - expected.center.y).abs() < 1e-5);
        assert!((camera.scale - expected.scale).abs() < 1e-5);
    }
}
