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
// 6. 宿主环境 — `install_cjk_font`：按平台候选路径（+ `XCAL_CJK_FONT`）安装 CJK 字体；
// 7. 帧末    — `draw_call_count`（可观测性取数）/ `end_frame`（清理已消费的队列）。
//
// 这是唯一同时接触 egui 与场景两者的模块；将来迁移 UI 只需另写一个 host，
// 场景逻辑一行不动。本文件之外的 xcal-egui 模块不得出现 egui 类型（B 阶段红线）。
//
// 帧内调用顺序（xcal-app::App::ui）：
// `build_scene_view` → `begin_frame` → `Scheduler::run_frame` → `present`。

use std::sync::Arc;

use egui::Panel;

use xc_notion::Rect2D;
use xc_object::{Camera2DHandle, Camera3D};
use xcal_render::{
    ContentTextureQueue, DrawQueue, DrawQueue3D, MorphQueue, PostFilterChain, RevealQueue, ViewRect,
};
use xcal_render_backends::{Recorded2D, SceneCompositorCallback, SceneLayer, snapshot_queue};
use xcal_scene::animation::BindingTable;
use xcal_scene::contract::WorldView;
use xcal_scene::director_state::DirectorState;
use xcal_scene::domain::{
    CameraCommand, ContentHandle, ContentTextureSpec, InputEvents, LayerBlend, LayerDescriptor,
    LayerId, LayerKind, SceneEvent, SceneView, ids,
};
use xcal_scene::{Curve, TimeState};
use xcal_schedule::resource::Resources;

use crate::resources::{UiConfig, ViewportCamera};

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
        // 视口操作目标（K3）：拖拽/滚轮产生哪一类相机命令。
        let target = res
            .get::<UiConfig>()
            .map(|c| c.viewport_camera)
            .unwrap_or_default();
        let mut events = InputEvents::from_events(Self::collect_events(ui, view, target));
        events.extend(Self::draw_control_panel(ui, view, res));
        events.extend(Self::draw_time_panel(ui, res));
        events.extend(Self::draw_param_panel(ui, res));
        res.insert(events);
        Self::update_view_rect(ui, res);
    }

    // ─── 宿主环境（平台相关）──────────────────────────────────────────────

    /// 安装 CJK 字体：按 `XCAL_CJK_FONT` 环境变量 + 各平台常见路径查找，
    /// 找到即注入 `Proportional` / `Monospace` 字体族，返回实际加载的路径。
    ///
    /// 为什么在 UI 层：字体是**宿主平台**的资产（字体文件路径与任何渲染后端无关），
    /// 且注入对象是 `egui::Context` —— 本 crate 的 host 是唯一允许出现 egui 类型的地方。
    ///
    /// 跨平台：候选列表覆盖 Windows / macOS / 常见 Linux 发行版；还可由环境变量指定。
    /// 找不到时返回 `None` 且**不 panic、不打日志**（提示文案由调用方决定）：中文会退化为方框。
    pub fn install_cjk_font(ctx: &egui::Context) -> Option<std::path::PathBuf> {
        let path = cjk_font_candidates().into_iter().find(|p| p.is_file())?;
        let data = std::fs::read(&path).ok()?;

        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("cjk".to_owned(), Arc::new(egui::FontData::from_owned(data)));
        // 比例字体：插到**最前**（默认正文字体）；等宽字体：追加（保留原有 ASCII 等宽字形）。
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "cjk".to_owned());
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk".to_owned());
        ctx.set_fonts(fonts);
        Some(path)
    }

    // ─── 帧末管理 / 可观测性 ───────────────────────────────────────────────

    /// 本帧的**绘制调用数**（2D 命令 + 3D mesh/line 条数）。
    ///
    /// ⚠️ 口径说明（本轮实测）：`xcal-schedule::FrameReport` 只有 System 耗时 / 实体数，
    /// `xcal_render_backends::FrameCacheStats` 只有三角化缓存的请求 / 未命中 —— **两者都没有**
    /// "绘制调用数"字段。故这里保留"查队列求长度"的原口径，只是把取数从 app 收到 host
    /// （app 不该知道队列的内部结构；将来 `FrameReport` 若增字段，可再收敛到这里）。
    pub fn draw_call_count(res: &Resources) -> usize {
        let two_d = res.get::<DrawQueue>().map_or(0, |q| q.len());
        let three_d = res
            .get::<DrawQueue3D>()
            .map_or(0, |q| q.mesh_count() + q.line_count());
        two_d + three_d
    }

    /// 帧末清理：2D 队列内容已被 `present` 消费，清空以便下帧重新录制。
    ///
    /// 为什么在 host 而不是 app：这是**帧管理**（谁消费、谁负责重置），
    /// app 只应表达"这一帧结束了"，不该知道要清哪个队列。
    pub fn end_frame(res: &mut Resources) {
        if let Some(queue) = res.get_mut::<DrawQueue>() {
            queue.clear();
        }
    }

    // ─── 场景 → UI：视图声明 ───────────────────────────────────────────────

    /// 从场景状态构建本帧视图声明（纯数据，无 egui 类型）。
    ///
    /// - 2D 图层：相机来自**场景对象**（`Camera2DHandle` 指认 `Framing2D`），
    ///   视口矩形由 `ViewRect` 绑定（P1-1 / P1-2：UI 不再持有相机状态）；
    ///   受 `UiConfig::show_2d` 控制（关掉即"关掉 2D 相机"）；
    /// - 3D 图层：相机取 World 中第一个 `Camera3D` 组件，受 `UiConfig::show_3d` 控制；
    /// - `ContentHandle` 为 B 阶段占位（S3 统一管线接管后指向 RenderQueue 分片）；
    /// - **混合方式（S3-2）**：两个图层的内容都渲染到**透明底**离屏纹理，
    ///   所以声明为 [`LayerBlend::Alpha`] —— 用 `Opaque`(REPLACE) 会把透明像素
    ///   原样写进主 pass，抹掉背景与下层（`Opaque` 仅作枚举默认值）。
    pub fn build_scene_view(world: &hecs::World, res: &Resources) -> SceneView {
        let rect = res
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);
        let cfg = res.get::<UiConfig>().cloned().unwrap_or_default();

        let mut view = SceneView::default();

        // ⚠️ **总闸优先（N3）**：`UiConfig::show_2d` / `show_3d` 是**图层级总闸**，
        // 关掉即整层不画（连 `Viewport2D` 图层都不声明）——此时**不看**任何对象的
        // 场景级 `Visibility`。总闸开时才轮到录制路径按对象 tag 逐对象判
        // （见 `xcal_render::draw::record_scene_tagged`）。
        if cfg.show_2d {
            if let Some(handle) = res.get::<Camera2DHandle>() {
                if let Some(camera) = handle.resolve(&WorldView::new(world), rect) {
                    view.layers.push(LayerDescriptor {
                        id: LayerId(view.layers.len() as u32),
                        kind: LayerKind::Viewport2D { camera },
                        content: ContentHandle::OPAQUE,
                        opacity: 1.0,
                        blend: LayerBlend::Alpha,
                    });
                }
            }
        }

        // hecs 0.11：单组件查询的 iter 产出 `&Camera3D`（无 Entity）。
        if cfg.show_3d {
            if let Some(camera3d) = world.query::<&Camera3D>().iter().next() {
                view.layers.push(LayerDescriptor {
                    id: LayerId(view.layers.len() as u32),
                    kind: LayerKind::Viewport3D { camera: *camera3d },
                    content: ContentHandle::OPAQUE,
                    opacity: 1.0,
                    blend: LayerBlend::Alpha,
                });
            }
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
    ///
    /// **操作目标由 `target` 决定**（K3，见 [`ViewportCamera`]）——2D / 3D 图层同屏
    /// 占据同一矩形，坐标无法区分：
    /// - [`ViewportCamera::TwoD`]：滚轮 → `Zoom { factor }`（factor = 1 + scroll × 0.001），
    ///   右键/中键拖拽 → `Pan { delta }`（屏幕位移在此换算为**世界坐标**，含 Y 翻转）；
    /// - [`ViewportCamera::ThreeDOrbit`]：拖拽 → `Orbit { delta_yaw, delta_pitch }`、
    ///   滚轮 → `Dolly { delta }`（3D 语义，无需世界换算）；
    /// - [`ViewportCamera::ThreeDFps`]：拖拽 → `Look { .. }`、
    ///   `WASD`/`QE` → `Move { delta }`（x = 右、y = 上、z = 前）。
    ///
    /// 悬停 / 点击（拾取类事件）与操作目标无关，两种目标下都上报。
    pub fn collect_events(
        ui: &egui::Ui,
        view: &SceneView,
        target: ViewportCamera,
    ) -> Vec<SceneEvent> {
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
        // M3-2：`camera.rect` 已是理念层 `Rect2D`（`min`/`max` 是字段）。
        let in_view = |p: egui::Pos2| {
            p.x >= camera.rect.min.x
                && p.x <= camera.rect.max.x
                && p.y >= camera.rect.min.y
                && p.y <= camera.rect.max.y
        };

        // 每帧位移/旋转换算系数（交互手感，纯数值选择）。
        /// 3D 拖拽 → 旋转角（弧度/像素）。
        const ORBIT_RAD_PER_PX: f32 = 0.01;
        /// 滚轮 → Dolly 步长（世界单位/滚轮量）。
        const DOLLY_PER_SCROLL: f32 = 0.02;
        /// FPS 按键 → 每帧位移（世界单位）。
        const MOVE_PER_FRAME: f32 = 0.08;

        let mut events = Vec::new();
        ui.input(|i| {
            match target {
                ViewportCamera::TwoD => {
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

                    // 右键/中键拖拽 → 平移（屏幕位移经对象层的
                    // `Camera2D::screen_delta_to_world` 换算为世界位移：缩放 + Y 翻转都在那里；
                    // 「拖拽方向 → 取景中心反向移动」取负是**控制器语义**，留在本层）。
                    if i.pointer.secondary_down() || i.pointer.middle_down() {
                        let d = i.pointer.delta();
                        if d != egui::Vec2::ZERO {
                            events.push(SceneEvent::Camera {
                                layer: *layer,
                                command: CameraCommand::Pan {
                                    delta: -camera.screen_delta_to_world((d.x, d.y).into()),
                                },
                            });
                        }
                    }
                }
                ViewportCamera::ThreeDOrbit => {
                    // 滚轮 → Dolly（向前滚 = 靠近 target）。
                    let scroll = i.smooth_scroll_delta.y;
                    if scroll != 0.0 {
                        events.push(SceneEvent::Camera {
                            layer: *layer,
                            command: CameraCommand::Dolly {
                                delta: scroll * DOLLY_PER_SCROLL,
                            },
                        });
                    }
                    // 拖拽（左/右/中键任一）→ 绕目标公转：向右拖 = 场景向右转。
                    if i.pointer.primary_down()
                        || i.pointer.secondary_down()
                        || i.pointer.middle_down()
                    {
                        let d = i.pointer.delta();
                        if d != egui::Vec2::ZERO {
                            events.push(SceneEvent::Camera {
                                layer: *layer,
                                command: CameraCommand::Orbit {
                                    delta_yaw: -d.x * ORBIT_RAD_PER_PX,
                                    delta_pitch: -d.y * ORBIT_RAD_PER_PX,
                                },
                            });
                        }
                    }
                }
                ViewportCamera::ThreeDFps => {
                    // 拖拽 → 原地转头（与鼠标移动同向）。
                    if i.pointer.primary_down()
                        || i.pointer.secondary_down()
                        || i.pointer.middle_down()
                    {
                        let d = i.pointer.delta();
                        if d != egui::Vec2::ZERO {
                            events.push(SceneEvent::Camera {
                                layer: *layer,
                                command: CameraCommand::Look {
                                    delta_yaw: d.x * ORBIT_RAD_PER_PX,
                                    delta_pitch: -d.y * ORBIT_RAD_PER_PX,
                                },
                            });
                        }
                    }
                    // WASD → 前后左右；QE → 上下（沿用 3D 建模软件惯例）。
                    let axis = |neg: egui::Key, pos: egui::Key| {
                        (i.key_down(pos) as i32 - i.key_down(neg) as i32) as f32
                    };
                    let delta = glam::Vec3::new(
                        axis(egui::Key::A, egui::Key::D),
                        axis(egui::Key::Q, egui::Key::E),
                        axis(egui::Key::S, egui::Key::W),
                    ) * MOVE_PER_FRAME;
                    if delta != glam::Vec3::ZERO {
                        events.push(SceneEvent::Camera {
                            layer: *layer,
                            command: CameraCommand::Move { delta },
                        });
                    }
                }
            }

            // 悬停 / 点击（视口内；拾取类事件，与操作目标无关）。
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
    /// Camera 读数只展示 —— 值取自**本帧视图声明**（相机已在场景对象里，
    /// P1-1 后 UI 不再持有相机状态）。返回空事件集（预留未来 Toggle 事件）。
    fn draw_control_panel(
        ui: &mut egui::Ui,
        view: &SceneView,
        res: &mut Resources,
    ) -> Vec<SceneEvent> {
        let time = res.get::<TimeState>().map(|t| t.elapsed).unwrap_or(0.0);
        let camera = view.layers.iter().find_map(|l| match &l.kind {
            LayerKind::Viewport2D { camera } => Some(*camera),
            _ => None,
        });
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
                // 图层开关（= 关掉对应的相机）：`build_scene_view` 不再声明该图层。
                ui.checkbox(&mut ui_config.show_2d, "2D 相机（网格/曲线/公式）");
                ui.checkbox(&mut ui_config.show_3d, "3D 相机（立方体/曲面）");

                ui.separator();
                // 视口操作目标（K3）：2D / 3D 同屏 ⇒ 由本开关决定拖拽/滚轮操作谁。
                ui.label("视口操作");
                ui.radio_value(
                    &mut ui_config.viewport_camera,
                    ViewportCamera::TwoD,
                    "2D 相机（平移/缩放）",
                );
                ui.radio_value(
                    &mut ui_config.viewport_camera,
                    ViewportCamera::ThreeDOrbit,
                    "3D 轨道（拖拽/滚轮）",
                );
                ui.radio_value(
                    &mut ui_config.viewport_camera,
                    ViewportCamera::ThreeDFps,
                    "3D 第一人称（拖拽 + WASD/QE）",
                );
                // 2D 相机的驱动方式：分镜（动画推动）还是自由（用户平移缩放）。
                // 「自由」会把分镜的 2D 相机绑定移出绑定表 —— 否则它每帧无条件重写
                // center/scale，用户操作同帧就被抹掉（见 `CameraDriveSystem`）。
                ui.checkbox(
                    &mut ui_config.free_2d_camera,
                    "2D 相机自由（关闭=分镜驱动）",
                );

                ui.separator();
                // S3-2 验收开关：翻转图层叠放顺序（见 `UiConfig::flip_layer_order`）。
                ui.checkbox(&mut ui_config.flip_layer_order, "翻转图层顺序");

                ui.separator();
                // C 线：合成后滤镜（post-composite）。全关 ⇒ 空链 ⇒ 渲染层走直通路径
                // （不建中间纹理、不多一次全屏 pass ⇒ 画面逐位不变）。
                ui.label("合成后滤镜");
                ui.checkbox(&mut ui_config.filter_gain, "亮度（点运算）");
                ui.checkbox(&mut ui_config.filter_distortion, "桶形畸变（几何重映射）");
                ui.checkbox(&mut ui_config.filter_blur, "模糊 3×3（邻域）");
                ui.add(
                    egui::Slider::new(&mut ui_config.filter_param, 0.0..=1.0)
                        .text("滤镜强度")
                        .fixed_decimals(2),
                );

                ui.separator();
                ui.label("Camera");
                match camera {
                    Some(c) => {
                        ui.label(format!("center: ({:.2}, {:.2})", c.center.x, c.center.y));
                        ui.label(format!("scale: {:.1}", c.scale));
                    }
                    None => {
                        ui.label("(无 2D 相机对象)");
                    }
                }
            });

        if let Some(cfg) = res.get_mut::<UiConfig>() {
            *cfg = ui_config.clone();
        }
        // C 线：面板开关 → **合成后滤镜链**（Resource）。全关 ⇒ 空链 ⇒ 渲染层直通。
        res.insert(crate::resources::post_filters(&ui_config));
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
                let label = if director.director.playing {
                    "⏸ 暂停"
                } else {
                    "▶ 播放"
                };
                if ui.button(label).clicked() {
                    events.push(SceneEvent::Button {
                        id: ids::PLAY_PAUSE,
                    });
                }

                // 重新播放：把时间拨回 0 并恢复播放（分镜从头重放）。
                // 同一行并排放，便于"暂停后回看"与"重播"的对比操作。
                if ui
                    .button("⟲ 重新播放")
                    .on_hover_text("把时间拨回 0 并开始播放")
                    .clicked()
                {
                    events.push(SceneEvent::Button { id: ids::REPLAY });
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
    /// **T0-② 后**：`dyn Target` 无法模式匹配，且集中分类清单已删除 ⇒
    /// **滑块集由曲线类型决定**（`Sin` 显示 `base/amp/omega`、`Orbit` 显示
    /// `radius/speed/angle`……），target 只提供**标签文字**（`target.name()`，可选中文化）。
    /// 滑块交互以 `Slider { id }` 事件上报，由场景侧 `ParamPanelSystem` 按 `WidgetId` 写回。
    ///
    /// 行为变化（T0-② 记账）：从 `match (kind, curve)` 改为 `match curve` ⇒
    /// **任何 `Sin` 曲线都会显示三个滑块**（原来只有 `CircleRadius` 的才显示）。
    /// 这是**更通用**的改进；当前场景里 `Curve::Phase` 会被 `RevealRange` 绑定用到
    /// （书写动画的 `omega`），故该滑块现在也会出现 —— 见交付说明。
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
                    match &binding.curve {
                        Curve::Sin { base, amp, omega } => {
                            ui.label(target_label(binding.target.name()));
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
                        Curve::Phase { omega } => {
                            ui.label(target_label(binding.target.name()));
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
                        Curve::Orbit {
                            radius,
                            speed,
                            initial_angle,
                            ..
                        } => {
                            ui.label(target_label(binding.target.name()));
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
        // egui::Rect → 理念层 `Rect2D` 的就地转换（后端转换由使用方负责）。
        let egui_rect = ui.available_rect_before_wrap();
        let rect = Rect2D::new(
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
            egui::pos2(view_rect.min.x, view_rect.min.y),
            egui::pos2(view_rect.max.x, view_rect.max.y),
        );

        let painter = ui.painter_at(egui_rect);

        // ── 背景铺底（图层全部在它之上） ──
        let bg = view.background;
        painter.rect_filled(
            egui_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(bg.r, bg.g, bg.b, bg.a),
        );

        let Some(callback) = Self::build_render_callback(view, res) else {
            return;
        };
        painter.add(egui::epaint::Shape::Callback(
            egui_wgpu::Callback::new_paint_callback(egui_rect, callback),
        ));
    }

    /// 从本帧视图与资源构造**唯一**合成回调（不含任何 egui 调用）。
    ///
    /// GUI（[`Self::present`]）与 headless 导出（`xcal-app` 的图像序列 / ffmpeg 管道）
    /// 共用同一份构造逻辑 —— 导出路径与实时路径的帧体因此不可能分叉。
    /// 返回 `None` 表示本帧没有图层（只画背景）。
    pub fn build_render_callback(
        view: &SceneView,
        res: &Resources,
    ) -> Option<SceneCompositorCallback> {
        let view_rect = res
            .get::<ViewRect>()
            .map(|v| v.rect)
            .unwrap_or_else(|| ViewRect::default().rect);

        // 无图层时不注册回调（与 S4.1 前"无 GPU 图层则不加回调"一致）。
        if view.layers.is_empty() {
            return None;
        }

        // 图层顺序开关（UI 状态）。缺省与 `UiConfig::default()` 一致（不翻转）。
        let flip = res
            .get::<UiConfig>()
            .map(|c| c.flip_layer_order)
            .unwrap_or(false);

        // 按有效序收集全部图层声明。S4.1 后所有 `LayerKind` 都走 GPU，
        // 故不再需要"逐层判断是否 GPU"，回调固定落在有效序起点。
        let order: Vec<usize> = if flip {
            (0..view.layers.len()).rev().collect()
        } else {
            (0..view.layers.len()).collect()
        };
        let gpu_specs: Vec<SceneLayer> = order
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
        let viewport = Rect2D::new(view_rect.min, view_rect.max);

        let (queue_2d, queue_3d) = (res.get::<DrawQueue>()?, res.get::<DrawQueue3D>()?);
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
        // 书写动画项（R0）：同口径的 owned 快照。
        let reveals = res
            .get::<RevealQueue>()
            .map(|q| q.0.clone())
            .unwrap_or_default();
        // 合成后滤镜链（C 线）：空链 ⇒ 渲染层走直通（不建中间纹理、画面逐位不变）。
        let filters = res.get::<PostFilterChain>().cloned().unwrap_or_default();
        Some(SceneCompositorCallback::new(
            viewport, gpu_specs, queue_2d, queue_3d, content, morphs, reveals, filters,
        ))
    }
}

/// 参数面板的**可选**中文标签映射（T0-②）。
///
/// 名字由各 target **自带**（`Target::name()`）；本表只是 UI 侧的增强 ——
/// **未收录的名字 fallback 到原样输出**，故它**不是**"新增 target 必须登记的清单"。
fn target_label(name: &str) -> &str {
    match name {
        "CircleRadius" => "圆半径",
        "Phase" => "函数曲线相位",
        "CameraEye" => "3D 轨道相机",
        "Camera2DFraming" => "2D 相机取景",
        "RevealRange" => "书写动画",
        "OpacityValue" => "不透明度",
        "TransformValue" => "摆放（位移/缩放）",
        _ => name,
    }
}

/// CJK 字体候选路径：环境变量 `XCAL_CJK_FONT` **优先**，其次各平台常见内置路径。
///
/// 只做"列候选"，是否存在由调用方过滤 —— 这样非 Windows 平台也能跑（无 panic / 无方框提示之外的行为）。
/// 注意：下面的 `C:\Windows\Fonts\...` **只在 Windows 上存在**；在 Linux / macOS 上
/// 这些候选会被"文件不存在"过滤掉，不是错误路径。
fn cjk_font_candidates() -> Vec<std::path::PathBuf> {
    let mut paths: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(custom) = std::env::var("XCAL_CJK_FONT") {
        if !custom.trim().is_empty() {
            paths.push(std::path::PathBuf::from(custom));
        }
    }
    paths.extend(
        [
            // Windows：微软雅黑
            r"C:\Windows\Fonts\msyh.ttc",
            // macOS：苹方
            "/System/Library/Fonts/PingFang.ttc",
            // Linux：Noto CJK（Debian/Ubuntu 与 Fedora/Arch 的常见落点）
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            // Linux：文泉驿正黑（轻量发行版常见）
            "/usr/share/fonts/truetype/wqy/wqy-zenhei.ttc",
        ]
        .map(std::path::PathBuf::from),
    );
    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use xc_object::{Camera3D, Framing2D, Projection};
    use xcal_scene::contract::WorldScene;
    use xcal_schedule::resource::Resources;

    /// 建含 3D 相机的世界（3D 图层来源）。
    fn spawn_3d_camera(world: &mut hecs::World) {
        world.spawn((Camera3D::new(
            glam::Vec3::new(0.0, 0.0, 5.0),
            glam::Vec3::ZERO,
            glam::Vec3::Y,
            Projection::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0),
        ),));
    }

    /// 建含 2D 相机对象的场景 + 其句柄 Resource（P1：相机是场景对象）。
    fn spawn_2d_camera(world: &mut hecs::World) -> Camera2DHandle {
        let mut scene = WorldScene::new(world);
        xc_object::spawn_camera2d(&mut scene, Framing2D::default()).expect("Framing2D 已登记")
    }

    /// headless：无任何 egui 上下文即可构建 SceneView。
    #[test]
    fn build_scene_view_declares_2d_and_3d_layers() {
        let mut world = hecs::World::new();
        spawn_3d_camera(&mut world);
        let handle = spawn_2d_camera(&mut world);

        let mut res = Resources::new();
        res.insert(handle);
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
        spawn_3d_camera(&mut world);
        let handle = spawn_2d_camera(&mut world);

        let mut res = Resources::new();
        res.insert(handle);
        res.insert(ViewRect::default());

        let view = EguiSceneHost::build_scene_view(&world, &res);
        assert_eq!(view.layers.len(), 2);
        for (i, a) in view.layers.iter().enumerate() {
            for b in view.layers.iter().skip(i + 1) {
                assert_ne!(a.id, b.id, "图层 LayerId 必须两两不相等");
            }
        }
    }

    /// 无 3D 相机实体时只声明 2D 图层；无 2D 相机句柄（或句柄失效）时无 2D 图层。
    #[test]
    fn build_scene_view_skips_missing_layers() {
        let mut world = hecs::World::new();
        let handle = spawn_2d_camera(&mut world);

        let mut res = Resources::new();
        res.insert(handle);
        let view = EguiSceneHost::build_scene_view(&world, &res);
        assert_eq!(view.layers.len(), 1);
        assert!(matches!(view.layers[0].kind, LayerKind::Viewport2D { .. }));

        // 空资源 ⇒ 无句柄 ⇒ 无图层。
        let res_empty = Resources::new();
        let view = EguiSceneHost::build_scene_view(&world, &res_empty);
        assert!(view.layers.is_empty());

        // 句柄存在但指向的场景对象已消失（空世界）⇒ 同样无 2D 图层。
        let empty_world = hecs::World::new();
        let mut res_stale = Resources::new();
        res_stale.insert(Camera2DHandle::default());
        assert!(
            EguiSceneHost::build_scene_view(&empty_world, &res_stale)
                .layers
                .is_empty()
        );
    }

    /// 2D 图层相机 = 场景里的取景参数（`Framing2D`）绑定 `ViewRect` —— P1-2 的
    /// "相机与渲染目标 rect 解绑"在场景 → UI 方向的落点。
    #[test]
    fn scene_view_2d_camera_binds_framing_to_view_rect() {
        let mut world = hecs::World::new();
        let mut scene = WorldScene::new(&mut world);
        let handle =
            xc_object::spawn_camera2d(&mut scene, Framing2D::new((2.0, -3.0).into(), 55.0))
                .expect("Framing2D 已登记");

        let mut res = Resources::new();
        res.insert(handle);
        res.insert(ViewRect::default());

        let rect = res.get::<ViewRect>().unwrap().rect;
        let view = EguiSceneHost::build_scene_view(&world, &res);
        let LayerKind::Viewport2D { camera } = view.layers[0].kind else {
            panic!("expected 2D layer");
        };
        assert!((camera.center.x - 2.0).abs() < 1e-5);
        assert!((camera.center.y + 3.0).abs() < 1e-5);
        assert!((camera.scale - 55.0).abs() < 1e-5);
        // rect 来自渲染目标（ViewRect），不是相机自带。
        assert_eq!(camera.rect, rect);
    }

    /// 图层开关（= 关掉对应相机）：关掉的图层**不再出现在声明里**。
    #[test]
    fn build_scene_view_respects_layer_toggles() {
        let mut world = hecs::World::new();
        spawn_3d_camera(&mut world);
        let handle = spawn_2d_camera(&mut world);

        let res_with = |show_2d: bool, show_3d: bool| {
            let mut res = Resources::new();
            res.insert(handle);
            res.insert(ViewRect::default());
            res.insert(UiConfig {
                show_2d,
                show_3d,
                ..UiConfig::default()
            });
            res
        };

        // 默认两图都在（`UiConfig::default()` 两个开关为 true）。
        assert_eq!(
            EguiSceneHost::build_scene_view(&world, &res_with(true, true))
                .layers
                .len(),
            2
        );

        // 关 2D ⇒ 只剩 3D。
        let v = EguiSceneHost::build_scene_view(&world, &res_with(false, true));
        assert_eq!(v.layers.len(), 1);
        assert!(matches!(v.layers[0].kind, LayerKind::Viewport3D { .. }));

        // 关 3D ⇒ 只剩 2D。
        let v = EguiSceneHost::build_scene_view(&world, &res_with(true, false));
        assert_eq!(v.layers.len(), 1);
        assert!(matches!(v.layers[0].kind, LayerKind::Viewport2D { .. }));

        // 两个都关 ⇒ 无图层（`present` 只铺背景）。
        assert!(
            EguiSceneHost::build_scene_view(&world, &res_with(false, false))
                .layers
                .is_empty()
        );
    }
}
