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

use xcal_scene::animation::{BindingTable, BindingTarget};
use xcal_scene::director_state::DirectorState;
use xcal_scene::domain::{
    ids, CameraCommand, ContentHandle, ContentTextureSpec, InputEvents, LayerBlend,
    LayerDescriptor, LayerId, LayerKind, SceneEvent, SceneView,
};
use xcal_schedule::resource::Resources;
use xcal_scene::scene::components::Camera3D;
use xcal_scene::types::Rect;
use xcal_scene::{Curve, TimeState};
use xcal_render::{ContentTextureQueue, DrawQueue, DrawQueue3D, MorphQueue};
use xcal_render_backends::{Recorded2D, SceneCompositorCallback, SceneLayer, snapshot_queue};
use xc_notion::Rect2D;

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
    /// - `ContentHandle` 为 B 阶段占位（S3 统一管线接管后指向 RenderQueue 分片）；
    /// - **混合方式（S3-2）**：两个图层的内容都渲染到**透明底**离屏纹理，
    ///   所以声明为 [`LayerBlend::Alpha`] —— 用 `Opaque`(REPLACE) 会把透明像素
    ///   原样写进主 pass，抹掉背景与下层（`Opaque` 仅作枚举默认值）。
    pub fn build_scene_view(world: &hecs::World, res: &Resources) -> SceneView {
        let rect = res
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);

        let mut view = SceneView::default();

        if let Some(cam) = res.get::<AppCamera>() {
            view.layers.push(LayerDescriptor {
                id: LayerId(view.layers.len() as u32),
                kind: LayerKind::Viewport2D {
                    camera: cam.to_camera_2d(rect),
                },
                content: ContentHandle::OPAQUE,
                opacity: 1.0,
                blend: LayerBlend::Alpha,
            });
        }

        // hecs 0.11：单组件查询的 iter 产出 `&Camera3D`（无 Entity）。
        if let Some(camera3d) = world.query::<&Camera3D>().iter().next() {
            view.layers.push(LayerDescriptor {
                id: LayerId(view.layers.len() as u32),
                kind: LayerKind::Viewport3D { camera: *camera3d },
                content: ContentHandle::OPAQUE,
                opacity: 1.0,
                blend: LayerBlend::Alpha,
            });
        }

        // 内容纹理层（S3-3）：仅当注册了 `ContentTextureSpec` 时声明。
        // 它不是视口层——渲染到独立纹理供 3D 材质采样（默认不参与合成）。
        if let Some(spec) = res.get::<ContentTextureSpec>() {
            view.layers.push(LayerDescriptor {
                id: LayerId(view.layers.len() as u32),
                kind: LayerKind::ContentTexture {
                    camera: spec.camera,
                    size_px: spec.size_px,
                    texture: spec.texture,
                    composite: spec.composite,
                },
                content: ContentHandle::OPAQUE,
                opacity: 1.0,
                blend: LayerBlend::Alpha,
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
                        scene_pos: camera.screen_to_world((pos.x, pos.y).into()),
                    });
                    if i.pointer.primary_clicked() {
                        events.push(SceneEvent::ViewportClick {
                            layer: *layer,
                            scene_pos: camera.screen_to_world((pos.x, pos.y).into()),
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
                // S3-2 验收开关：翻转图层叠放顺序（见 `UiConfig::flip_layer_order`）。
                ui.checkbox(&mut ui_config.flip_layer_order, "翻转图层顺序");

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
        // egui::Rect → xcal_scene::Rect 的就地转换（后端转换由使用方负责）。
        let egui_rect = ui.available_rect_before_wrap();
        let rect = Rect::new(
            (egui_rect.min.x, egui_rect.min.y).into(),
            (egui_rect.max.x, egui_rect.max.y).into(),
        );
        if let Some(vr) = res.get_mut::<ViewRect>() {
            vr.rect = rect;
        }
    }

    // ─── 内容提交 ──────────────────────────────────────────────────────────

    /// 把本帧录制的内容提交到 egui painter（原 RenderSubmitSystem /
    /// RenderSubmit3DSystem / GridBuild 背景矩形的职责）。在 `Scheduler::run_frame`
    /// 之后调用。
    ///
    /// **顺序语义（S3-2）**：`SceneView::layers` 的**索引序 = 合成序**（先 = 下）。
    /// `UiConfig::flip_layer_order` 仅用于验收时翻转一次顺序（不改场景声明）。
    ///
    /// **单一回调（S3-2 修复）**：`egui_wgpu::CallbackResources` 是全局 TypeMap，
    /// 若每个图层各注册一个回调，它们会互相覆盖同一份合成资源
    /// （见 [`xcal_render_backends::scene_callback`] 头部说明）。因此这里
    /// 只注册**一个** [`SceneCompositorCallback`]，由它一次性渲染并合成**全部**
    /// 图层（S4.1 后 2D / 3D / 内容纹理**全部**走 GPU，不再有 CPU 回放分支）。
    ///
    /// - 背景：先铺 `SceneView::background`（原 GridBuild 的 `-1000` 全屏矩形职责）；
    /// - 全部图层：进唯一回调（离屏 + 合成，按有效序）。
    pub fn present(ui: &mut egui::Ui, view: &SceneView, res: &mut Resources) {
        let view_rect = res
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let egui_rect = egui::Rect::from_min_max(
            egui::pos2(view_rect.min().x, view_rect.min().y),
            egui::pos2(view_rect.max().x, view_rect.max().y),
        );

        let painter = ui.painter_at(egui_rect);

        // ── 背景铺底（图层全部在它之上） ──
        let bg = view.background;
        painter.rect_filled(
            egui_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(bg.r, bg.g, bg.b, bg.a),
        );

        // 图层顺序开关（UI 状态）。缺省与 `UiConfig::default()` 一致（不翻转）。
        let flip = res
            .get::<UiConfig>()
            .map(|c| c.flip_layer_order)
            .unwrap_or(false);

        // 无图层时不注册回调（与 S4.1 前"无 GPU 图层则不加回调"一致）。
        if view.layers.is_empty() {
            return;
        }

        // 按有效序收集全部图层声明。S4.1 后所有 `LayerKind` 都走 GPU，
        // 故不再需要"逐层判断是否 GPU"，回调固定落在有效序起点。
        let order: Vec<usize> = if flip {
            (0..view.layers.len()).rev().collect()
        } else {
            (0..view.layers.len()).collect()
        };
        let mut gpu_specs: Vec<SceneLayer> = order
            .iter()
            .map(|&idx| {
                let layer = &view.layers[idx];
                SceneLayer {
                    id: layer.id,
                    kind: layer.kind.clone(),
                    blend: layer.blend,
                    opacity: layer.clamped_opacity(),
                }
            })
            .collect();

        // 视口矩形（egui points）：GPU 回调的渲染区域（离屏 / 合成都在此范围内）。
        let viewport = Rect2D::new(*view_rect.min(), *view_rect.max());

        let (Some(queue_2d), Some(queue_3d)) = (res.get::<DrawQueue>(), res.get::<DrawQueue3D>())
        else {
            return;
        };
        // 内容纹理层快照（S3-3）：单份 `ContentTextureQueue` → 关联到
        // 唯一的 `ContentTexture` 图层 id（单层约束见 `ContentTextureSpec`）。
        let content: Vec<(LayerId, Vec<Recorded2D>)> = res
            .get::<ContentTextureQueue>()
            .map(|q| {
                let snap = snapshot_queue(&q.0);
                gpu_specs
                    .iter()
                    .filter(|l| matches!(l.kind, LayerKind::ContentTexture { .. }))
                    .map(|l| (l.id, snap.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // 形变项（S4.3）：回调在 `prepare` 阶段才消费，故需 owned 快照
        // （与 `SceneLayer` / 内容纹理快照同一口径）。
        let morphs = res
            .get::<MorphQueue>()
            .map(|q| q.0.clone())
            .unwrap_or_default();
        let callback = SceneCompositorCallback::new(
            viewport,
            std::mem::take(&mut gpu_specs),
            queue_2d,
            queue_3d,
            content,
            morphs,
        );
        painter.add(egui::epaint::Shape::Callback(
            egui_wgpu::Callback::new_paint_callback(egui_rect, callback),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_schedule::resource::Resources;
    use xcal_scene::scene::components::{Camera3D, Projection};

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
        // S3-2：两个图层都渲染到透明底离屏纹理，必须声明为 Alpha
        // （用默认的 Opaque/REPLACE 会把透明像素写进主 pass，抹掉背景）。
        for l in &view.layers {
            assert_eq!(l.blend, LayerBlend::Alpha);
            assert_eq!(l.opacity, 1.0);
            assert_eq!(l.clamped_opacity(), 1.0);
        }
        assert_eq!(view.background, xcal_scene::domain::DEFAULT_BACKGROUND);
    }

    /// S3-2 修复：`view.layers` 内 `LayerId` 必须**两两唯一**
    /// （此前是常量式 —— 所有层同一个值会让按层寻址的方案退化成冲突）。
    #[test]
    fn build_scene_view_assigns_unique_layer_ids() {
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
        for (i, a) in view.layers.iter().enumerate() {
            for b in view.layers.iter().skip(i + 1) {
                assert_ne!(a.id, b.id, "图层 LayerId 必须两两不相等");
            }
        }
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
