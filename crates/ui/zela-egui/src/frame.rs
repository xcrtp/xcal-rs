// frame.rs — eframe 宿主的**每帧帧体**（A5：从 `zela-app` 归位到 UI 层）。
//
// app 只保留壳子职责（装配 / 命令提交 / 事件订阅）；"怎么画一帧"属于 UI 层。
// 本模块与 `EguiSceneHost` 的关系：host 负责场景 ↔ egui 的适配，本模块负责帧序
// （构建视图 → 收集输入 → 提交命令 → 呈现 → 可观测性 → 帧末清理）。

use std::sync::Arc;
use std::time::Instant;

use zela_scene::{DriverCommand, DriverPod};

use crate::health::HealthOverlay;
use crate::host::EguiSceneHost;

/// 安装宿主视觉（深色主题 + 提亮弱文字）。
pub fn install_visuals(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.weak_text_color = Some(egui::Color32::from_rgb(210, 210, 210));
    visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_rgb(200, 200, 200);
    ctx.set_visuals(visuals);
}

/// GUI 窗口参数（app 的 `main` 不再认识 egui 类型）。
pub fn window_options() -> eframe::NativeOptions {
    eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default().with_inner_size([800.0, 600.0]),
        ..Default::default()
    }
}

/// 每帧帧体状态（墙钟、健康 overlay）。
pub struct SceneFrame {
    driver: Arc<DriverPod>,
    health: HealthOverlay,
    /// 墙钟时间基准：每帧用 `Instant::now() - last_frame` 算 dt，
    /// 不依赖渲染帧间隔（帧率波动、卡顿都不影响 Director 时间流速）。
    last_frame: Instant,
}

impl SceneFrame {
    pub fn new(driver: Arc<DriverPod>) -> Self {
        Self {
            driver,
            health: HealthOverlay::new(),
            last_frame: Instant::now(),
        }
    }

    /// 推进并呈现一帧。`progress` = 事件订阅方收到的 `(帧数, 全局时间)`。
    pub fn ui(&mut self, ui: &mut egui::Ui, progress: (u32, f32)) {
        // 墙钟 dt：用单调时钟算，不随渲染帧率波动。
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;

        // ── B 阶段帧流程（UI 与场景只经 SceneView / SceneEvent 契约交互）──
        //
        // 1. 场景 → UI：构建本帧视图声明（2D/3D 图层 + 相机）。
        //    相机取上一帧末状态——视口事件换算对该滞后不敏感
        //    （缩放因子/平移量只依赖 scale，不依赖 rect）。
        let driver = Arc::clone(&self.driver);
        let view = driver.with_scheduler(|scheduler| {
            EguiSceneHost::build_scene_view(scheduler.world(), scheduler.resources())
        });

        // 2. UI → 场景：收集输入事件、绘制面板 chrome、写 ViewRect。
        //    必须在推进之前：System 消费 InputEvents，Axes/FunctionGraph
        //    刷新当帧读 ViewRect。
        driver.with_scheduler_mut(|scheduler| {
            EguiSceneHost::begin_frame(ui, &view, scheduler.resources_mut());
        });

        // 3. **命令推进**：GUI 与 headless 共用同一条 `Advance(dt)` 命令。
        //    墙钟只决定 dt；推进语义完全由 Driver pod 负责。
        let _receipt = driver.submit(DriverCommand::Advance { dt });

        // 4. 内容提交：2D 队列 + 3D 队列 → painter（原 RenderSubmit 职责）。
        driver.with_scheduler_mut(|scheduler| {
            EguiSceneHost::present(ui, &view, scheduler.resources_mut());
        });

        // ── 可观测性：F1 切换 health overlay，F10 导出 World 快照 ──
        if ui.input(|i| i.key_pressed(egui::Key::F1)) {
            self.health.toggle();
        }
        if ui.input(|i| i.key_pressed(egui::Key::F10)) {
            let (tick, result) = driver.with_scheduler(|scheduler| {
                let tick = scheduler.tick();
                let result =
                    self.health
                        .dump_snapshot(scheduler.world(), scheduler.resources(), tick);
                (tick, result)
            });
            match result {
                Ok(path) => {
                    tracing::info!(tick, path = %path.display(), "world snapshot exported")
                }
                Err(e) => tracing::error!(%e, tick, "snapshot export failed"),
            }
        }
        driver.with_scheduler(|scheduler| {
            let draw_calls = EguiSceneHost::draw_call_count(scheduler.resources());
            self.health.update(
                dt,
                scheduler.frame_report(),
                scheduler.world().len() as usize,
                draw_calls,
            );
        });
        self.health.set_progress(progress.0, progress.1);
        self.health.show(ui);

        // 帧末：清空已被 present 消费的 2D 队列（帧管理归 host）。
        driver.with_scheduler_mut(|scheduler| {
            EguiSceneHost::end_frame(scheduler.resources_mut());
        });

        // egui 默认事件驱动，无鼠标/键盘事件时不调度 ui()，导致
        // TimeState.elapsed 停止增长、动画卡住。动画引擎需持续重绘，每帧请求下一帧。
        ui.ctx().request_repaint();
    }
}
