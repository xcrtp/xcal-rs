// xcal-app: 组装 + 启动层
//
// App::new 负责：
//   - 构建 Scene、初始化 World 图元
//   - 构建 Resources、挂载各个 egui Resource 类型
//   - 构建 Scheduler、按 Phase 注册 System（迁移到新阶段分配以避开冲突）
//   - 实现 eframe::App trait（ui 每帧调度）

use std::sync::Arc;
use std::time::Instant;

pub use xcal_core::Result;
use xcal_core::animation::{
    Binding, BindingApplySystem, BindingTable, BindingTarget, MorphApplySystem, SeekKind,
};
use xcal_core::director_state::{DirectorAdvanceSystem, DirectorState};
use xcal_core::errors::Error;
use xcal_core::resource::Resources;
use xcal_core::domain::InputEvents;
use xcal_core::scene::components::{Camera3D, MorphShape, Projection};
use xcal_core::scene::primitives::wireframe_cube;
use xcal_core::scheduler::{Phase, Scheduler};
use xcal_core::TimeState;
use xcal_core::types::{Color32, Vec3};
use xcal_core::{ClockForest, ClockNode, Curve, PlayMode};
use xcal_egui::resources::{AppCamera, UiConfig, ViewRect};
use xcal_egui::systems::{CameraInputSystem, GridBuildSystem, SceneRecordSystem};
use xcal_egui::{EguiSceneHost, TimeControlSystem};
use xcal_render::components::{Circle, FunctionX2Y, Path, PhaseAnim, Sampler1D, StrokeSolid};
use xcal_render::{DrawQueue, DrawQueue3D, MeasureSpace, scene::Scene};
use xcal_render_backends::{Compositor, RenderContext, WgpuBackend2D, WgpuBackend3D};
use xc_geometry::Path2D;

mod formula;
mod health;
mod param_panel;
mod systems_3d;
use health::HealthOverlay;
use param_panel::ParamPanelSystem;
use systems_3d::SceneRecord3DSystem;

pub struct App {
    scheduler: Scheduler,
    render_ctx: RenderContext,
    health: HealthOverlay,
    /// 墙钟时间基准：每帧用 Instant::now() - last_frame 算 dt，
    /// 不依赖渲染帧间隔（帧率波动、卡顿都不影响 Director 时间流速）。
    last_frame: Instant,
}

impl App {
    pub fn new(ctx: &eframe::CreationContext) -> Result<Self> {
        Self::setup_fonts(&ctx.egui_ctx)?;
        Self::setup_visuals(&ctx.egui_ctx);

        let render_ctx = {
            let rs = ctx
                .wgpu_render_state
                .clone()
                .ok_or(Error::InitializationError(
                    "Wgpu render state not available".to_string(),
                ))?;

            // 3D 后端：pipeline 等 GPU 资源存入 egui-wgpu Renderer 的
            // callback_resources（TypeMap），SceneCompositorCallback 在 prepare/paint
            // 阶段按需取用（回调本身为 &self，不能持有重建中的 GPU 资源）。
            let backend_3d =
                WgpuBackend3D::with_default_shader(&rs.device, &rs.queue, rs.target_format);
            rs.renderer.write().callback_resources.insert(backend_3d);

            // 2D 后端（S3-1）：同一 TypeMap 里再放一份 WgpuBackend2D，
            // 由 `UiConfig::gpu_2d` 开关决定 2D 是否走 GPU 离屏路径。
            // 后端常驻（pipeline/缓存/离屏目标随视口尺寸惰性重建），
            // 关掉开关时不产生任何 GPU 工作。
            let backend_2d = WgpuBackend2D::new(&rs.device, &rs.queue);
            rs.renderer.write().callback_resources.insert(backend_2d);

            // 合成器（S3-2）：2D / 3D 图层各画到自己的离屏纹理后，
            // 由它按 `SceneView::layers` 的声明顺序混合进主 pass。
            let compositor = Compositor::new(&rs.device, rs.target_format);
            rs.renderer.write().callback_resources.insert(compositor);

            RenderContext::from_render_state(&rs)
        };

        let mut scene = Scene::new();

        // ── 3D 场景：白色线框立方体 ──────────────────────────────
        scene.spawn((wireframe_cube(1.0, Color32::new(255, 255, 255, 255)),));

        // 3D 相机（eye 由 BindingApplySystem 驱动，旧 OrbitAnim 组件已移除）。
        let camera_entity = scene.spawn((
            Camera3D::new(
                glam::Vec3::new(0.0, 0.0, 5.0),
                glam::Vec3::ZERO,
                glam::Vec3::Y,
                Projection::perspective(60.0f32.to_radians(), 16.0 / 9.0, 0.1, 100.0),
            ),
        ));

        let circle_entity = scene.spawn((
            Circle::new((0.0, 0.0), 1.0),
            // 屏幕空间：圆粗细是"观测工具"，不随缩放变化。
            StrokeSolid::new_in(2.0, (100, 255, 0, 255), MeasureSpace::Screen),
            // 半径振荡由 BindingApplySystem 驱动（Binding 表：base=1.0, amp=0.4, omega=2.0）
        ));

        let func_entity = scene.spawn((
            FunctionX2Y::new(|x: f32| x.sin()),
            Sampler1D::new(-10.0f32, 10.0, 0.1),
            StrokeSolid::new_in(2.0, (0, 200, 200, 255), MeasureSpace::Screen),
            // 相位滑动由 BindingApplySystem 驱动（Binding 表：omega=1.5）
            PhaseAnim::new(0.0, 1.5),
        ));

        // ── 公式接入：Typst 内容管线 → 旧 2D 描边路径（空心字）──────────
        // 静态一条公式，启动时烘焙一次并 spawn 实体；rebuild() 已留口，
        // 将来接交互只需加 UI 工作量。零 System 改动：公式作为普通
        // (Path, StrokeSolid) 实体参与现有 record_scene 录制。
        // 位置选在上方空白区域，避开 sin 曲线 / 圆 / 网格轴线；
        // 字号按"窄窗口也能完整看到"取值（默认视口 x∈[-10,10]，但面板会压缩视口）。
        let engine = formula::build_engine();
        let spec = formula::FormulaSpec::new("x^2 + 1", 1.3, glam::Vec2::new(-2.8, 3.0));
        match formula::rebuild(&engine, &spec) {
            Some(ir) => {
                for (vertices, color, width, closed, space) in
                    formula::compile_to_stroke_paths(&ir, &spec)
                {
                    scene.spawn((
                        Path::new(vertices, closed),
                        // 公式描边是世界空间：与字形一同随缩放成比例变化。
                        StrokeSolid::new_in(width, color, space),
                    ));
                }
            }
            None => eprintln!("[xcal-app] 公式编译失败，跳过公式实体"),
        }

        // ── S1 形变 demo：圆 → 方（弧长对齐的 Path 形变）────────────────
        // MorphShape 只持有 from/to（System 只写 t）；MorphApplySystem 每帧把
        // 插值结果写进同实体的 Path，由现有 record_scene 消费（零录制改动）。
        let morph_center = glam::Vec2::new(-6.0, -2.5);
        let morph_radius = 1.2;
        // 起点对齐（原型约定，见 S1 计划已知限制"旋转对齐"）：圆的起笔方向与
        // 方的首个顶点方向一致（均指向 -45° 的右下角），避免形变过程中"绕圈"旋转。
        let start_angle = -std::f32::consts::FRAC_PI_4;
        let circle_pts: Vec<glam::Vec2> = (0..64)
            .map(|i| {
                let a = start_angle + std::f32::consts::TAU * (i as f32) / 64.0;
                morph_center + glam::Vec2::new(a.cos(), a.sin()) * morph_radius
            })
            .collect();
        let square_pts: Vec<glam::Vec2> = [
            glam::Vec2::new(morph_radius, -morph_radius),
            glam::Vec2::new(morph_radius, morph_radius),
            glam::Vec2::new(-morph_radius, morph_radius),
            glam::Vec2::new(-morph_radius, -morph_radius),
        ]
        .iter()
        .map(|p| morph_center + *p)
        .collect();
        let morph_entity = scene.spawn((
            MorphShape::new(
                Path2D::from_polyline(&circle_pts, true),
                Path2D::from_polyline(&square_pts, true),
            ),
            // Path 是形变输出目标（首帧由 MorphApplySystem 填充）。
            Path::new(Vec::<Vec3>::new(), true),
            StrokeSolid::new_in(2.0, (255, 180, 0, 255), MeasureSpace::Screen),
        ));

        // ── S1b 语义形变 demo：x^2 ↔ x^3（按源码锚点配对）──────────────
        // `pair_by_anchor` 保证 `x` 配对到 `x`（而非按位置配到 `2`/`3`）。
        // 每对路径 spawn 一个 MorphShape 实体，由同一个形变时钟同步驱动。
        let anchor_spec = formula::FormulaSpec::new("x^2", 1.0, glam::Vec2::new(1.5, 3.0));
        let mut semantic_morph_entities: Vec<hecs::Entity> = Vec::new();
        let from_ir = formula::rebuild(&engine, &anchor_spec);
        let to_ir = formula::rebuild(
            &engine,
            &formula::FormulaSpec::new("x^3", 1.0, glam::Vec2::new(1.5, 3.0)),
        );
        if let (Some(f), Some(t)) = (from_ir, to_ir) {
            for (from_path, to_path) in formula::compile_morph_pairs(&f, &t, &anchor_spec) {
                semantic_morph_entities.push(scene.spawn((
                    MorphShape::new(from_path, to_path),
                    Path::new(Vec::<Vec3>::new(), true),
                    StrokeSolid::new_in(1.0, (255, 120, 220, 255), MeasureSpace::Screen),
                )));
            }
        }

        let world = scene.into_world();

        // ── 时钟树 + 绑定表（动画唯一驱动链路）────────────────────────
        let mut forest = ClockForest::new();
        let root_circle = forest.insert(ClockNode::new(None)).unwrap();
        let root_phase = forest.insert(ClockNode::new(None)).unwrap();
        let root_orbit = forest.insert(ClockNode::new(None)).unwrap();
        // 形变时钟：PingPong 在 [0,1] 来回 → t 平滑往返。
        let mut morph_clock = ClockNode::new(None);
        morph_clock.span = Some((0.0, 1.0));
        morph_clock.mode = PlayMode::PingPong;
        let root_morph = forest.insert(morph_clock).unwrap();

        let mut bindings = BindingTable::new();
        // 振动圆半径：radius = base + amp * sin(t * omega)，base=1, amp=0.4, omega=2
        bindings.add(Binding {
            clock: root_circle,
            curve: Curve::Sin { base: 1.0, amp: 0.4, omega: 2.0 },
            target: BindingTarget::CircleRadius(circle_entity),
            seek_kind: SeekKind::O1,
        });
        // sin 曲线相位滑动：phase = t * omega，omega=1.5
        bindings.add(Binding {
            clock: root_phase,
            curve: Curve::Phase { omega: 1.5 },
            target: BindingTarget::Phase(func_entity),
            seek_kind: SeekKind::O1,
        });
        // 3D 轨道相机：eye = radius * rotate_y(speed * t)，radius=5, speed=0.5
        bindings.add(Binding {
            clock: root_orbit,
            curve: Curve::Orbit {
                radius: 5.0,
                axis: glam::Vec3::Y,
                speed: 0.5,
                initial_angle: 0.0,
            },
            target: BindingTarget::CameraEye(camera_entity),
            seek_kind: SeekKind::O1,
        });
        // 形变参数：t = 局部时间 ∈ [0,1]（PingPong 来回）→ MorphShape.t
        bindings.add(Binding {
            clock: root_morph,
            curve: Curve::Phase { omega: 1.0 },
            target: BindingTarget::MorphT(morph_entity),
            seek_kind: SeekKind::O1,
        });
        // S1b 语义形变：每个配对实体一条 MorphT 绑定（共用同一形变时钟）。
        for &entity in &semantic_morph_entities {
            bindings.add(Binding {
                clock: root_morph,
                curve: Curve::Phase { omega: 1.0 },
                target: BindingTarget::MorphT(entity),
                seek_kind: SeekKind::O1,
            });
        }

        let mut resources = Resources::new();
        resources.insert(AppCamera::default());
        resources.insert(TimeState::default());
        resources.insert(UiConfig::default());
        resources.insert(ViewRect::default());
        resources.insert(InputEvents::default());
        resources.insert(DrawQueue::new());
        resources.insert(DrawQueue3D::new());
        resources.insert(render_ctx.clone());
        resources.insert(forest);
        resources.insert(bindings);
        resources.insert(DirectorState::new());

        let mut scheduler = Scheduler::new(world, resources);

        // P3：Director 是唯一时间源。B 阶段（UI 解耦）后的 Phase 分配：
        // - Input:        DirectorAdvance（W=DirectorState, TimeState）
        //                 CameraInput（R=InputEvents, W=AppCamera，消费视口事件）
        // - LogicUpdate:  TimeControl（R=InputEvents, W=DirectorState）
        //                 ParamPanel（R=InputEvents, W=BindingTable）
        // - SceneSync:    BindingApply（统一驱动 2D+3D 动画）
        // - Prepare:      GridBuild
        // - RenderCommit: SceneRecord（2D）+ SceneRecord3D
        // - FrameEnd:     无——内容提交由 host.present 承担（原 RenderSubmit 职责）。
        //                 面板 chrome 与 ViewRect 由 host.begin_frame 承担（run_frame 前）。
        scheduler.add_system(Phase::Input, DirectorAdvanceSystem);
        scheduler.add_system(Phase::Input, CameraInputSystem);

        scheduler.add_system(Phase::LogicUpdate, TimeControlSystem);
        scheduler.add_system(Phase::LogicUpdate, ParamPanelSystem);

        scheduler.add_system(Phase::SceneSync, BindingApplySystem);
        // MorphApply 必须在 BindingApply 之后：后者写 MorphShape.t，前者消费。
        scheduler.add_system(Phase::SceneSync, MorphApplySystem);

        scheduler.add_system(Phase::Prepare, GridBuildSystem);

        // RenderCommit：2D SceneRecord 写 DrawQueue(2D)，
        // 3D SceneRecord3D 写 DrawQueue3D，类型不相交。
        scheduler.add_system(Phase::RenderCommit, SceneRecordSystem);
        scheduler.add_system(Phase::RenderCommit, SceneRecord3DSystem);

        Ok(Self {
            scheduler,
            render_ctx,
            health: HealthOverlay::new(),
            last_frame: Instant::now(),
        })
    }

    pub fn render_context(&self) -> RenderContext {
        self.render_ctx.clone()
    }

    fn setup_fonts(ctx: &egui::Context) -> Result<()> {
        let mut fonts = egui::FontDefinitions::default();
        let font_path = "C:\\Windows\\Fonts\\msyh.ttc";
        if let Ok(font_data) = std::fs::read(font_path) {
            fonts.font_data.insert(
                "msyh".to_owned(),
                Arc::new(egui::FontData::from_owned(font_data)),
            );
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "msyh".to_owned());
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("msyh".to_owned());
            ctx.set_fonts(fonts);
        } else {
            eprintln!("警告：未找到微软雅黑字体，中文可能显示为方框。");
        }
        Ok(())
    }

    /// 调亮 UI 文字颜色：默认 dark theme 的 weak_text_color 偏灰，
    /// 把 hint / inactive 都提亮。
    fn setup_visuals(ctx: &egui::Context) {
        let mut visuals = egui::Visuals::dark();
        visuals.weak_text_color = Some(egui::Color32::from_rgb(210, 210, 210));
        visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(200, 200, 200);
        ctx.set_visuals(visuals);
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // 墙钟 dt：用单调时钟算，不随渲染帧率波动。
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // ── B 阶段帧流程（UI 与场景只经 SceneView / SceneEvent 契约交互）──
        //
        // 1. 场景 → UI：构建本帧视图声明（2D/3D 图层 + 相机）。
        //    相机取上一帧末状态——视口事件换算对该滞后不敏感
        //    （缩放因子/平移量只依赖 scale，不依赖 rect）。
        let view =
            EguiSceneHost::build_scene_view(self.scheduler.world(), self.scheduler.resources());

        // 2. UI → 场景：收集输入事件、绘制面板 chrome、写 ViewRect。
        //    必须在 run_frame 之前：System 消费 InputEvents，GridBuild 当帧读 ViewRect。
        EguiSceneHost::begin_frame(ui, &view, self.scheduler.resources_mut());

        self.scheduler.run_frame(dt);

        // 3. 内容提交：2D 队列 + 3D 队列 → painter（原 RenderSubmit 职责）。
        EguiSceneHost::present(ui, &view, self.scheduler.resources_mut());

        // ── 可观测性：F1 切换 health overlay，F10 导出 World 快照 ──
        if ui.input(|i| i.key_pressed(egui::Key::F1)) {
            self.health.toggle();
        }
        if ui.input(|i| i.key_pressed(egui::Key::F10)) {
            let tick = self.scheduler.tick();
            match self.health.dump_snapshot(
                self.scheduler.world(),
                self.scheduler.resources(),
                tick,
            ) {
                Ok(path) => tracing::info!(tick, path = %path.display(), "world snapshot exported"),
                Err(e) => tracing::error!(%e, tick, "snapshot export failed"),
            }
        }
        let draw_calls = self
            .scheduler
            .resources()
            .get::<DrawQueue>()
            .map_or(0, |q| q.len())
            + self
                .scheduler
                .resources()
                .get::<DrawQueue3D>()
                .map_or(0, |q| q.mesh_count() + q.line_count());
        self.health.update(
            dt,
            self.scheduler.frame_report(),
            self.scheduler.world().len() as usize,
            draw_calls,
        );
        self.health.show(ui);

        // 每帧清空 2D 队列（present 已消费其内容；SceneRecord 下帧重新录制）。
        if let Some(queue) = self.scheduler.resources_mut().get_mut::<DrawQueue>() {
            queue.clear();
        }

        // egui 默认事件驱动，无鼠标/键盘事件时不调度 ui()，导致
        // TimeState.elapsed 停止增长、动画卡住。动画引擎需持续重绘，每帧请求下一帧。
        ui.ctx().request_repaint();
    }
}
