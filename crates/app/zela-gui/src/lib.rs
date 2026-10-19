// zela-gui — GUI 可执行壳。
//
// 依赖方向（S0 后）：`zela-app` 与 `zela-egui` 是**同级**下层；本 crate 在最上，
// 负责把它们撮合成 eframe::App。这样 `zela-app` 可以彻底不依赖 UI：
// headless 导出、脚本装配、录像都能独立编译。

use std::sync::Arc;

use eframe::{App as EframeApp, egui as gui};

use ze_pod::Pod;
use zela_app::{SceneChoice, build_scene_runtime};
use zela_egui::{EguiSceneHost, FrameProgress, SceneFrame, install_visuals};
use zela_render_backends::{RenderContext, install_standard_backends};
use zela_scene::DriverPod;
use zela_scene::Result;
use zela_scene::errors::Error;

/// GUI 应用壳：持有 Runtime 与每帧帧体。
pub struct App {
    /// 场景推进器（GUI 与 headless 共用同一条命令路径）。
    driver: Arc<DriverPod>,
    /// 事件订阅方：只观测 `FrameFinished`（进度条 / health overlay）。
    progress: Arc<FrameProgress>,
    render_ctx: RenderContext,
    /// 每帧帧体（墙钟 / health / 呈现）—— UI 层实现，壳只委托。
    frame: SceneFrame,
}

impl App {
    /// 默认场景（§1.1 分镜）。
    pub fn new(ctx: &eframe::CreationContext) -> Result<Self> {
        Self::new_with_scene(ctx, SceneChoice::Storyboard)
    }

    /// 按 [`SceneChoice`] 装配 GUI。
    pub fn new_with_scene(ctx: &eframe::CreationContext, choice: SceneChoice) -> Result<Self> {
        // 宿主字体（平台相关）：候选路径 + `ZELA_CJK_FONT`；找不到只提示、不中止。
        if EguiSceneHost::install_cjk_font(&ctx.egui_ctx).is_none() {
            eprintln!(
                "警告：未找到可用的中文字体（可用环境变量 ZELA_CJK_FONT 指定字体路径），中文可能显示为方框。"
            );
        }
        install_visuals(&ctx.egui_ctx);

        // 渲染环境（后端 + 合成器 + RenderContext）由 backends 侧的"标准安装"负责。
        let render_ctx = {
            let rs = ctx
                .wgpu_render_state
                .clone()
                .ok_or(Error::InitializationError(
                    "Wgpu render state not available".to_string(),
                ))?;
            install_standard_backends(&rs)
        };

        let runtime = build_scene_runtime(render_ctx, choice)?;
        // 进度只观测 `FrameFinished`（不碰命令队列）。
        let progress = Arc::new(FrameProgress::new());
        let sink: Arc<dyn Pod> = Arc::clone(&progress) as Arc<dyn Pod>;
        runtime
            .driver
            .attach_sink(sink)
            .map_err(|e| Error::InitializationError(format!("Driver 事件订阅失败：{e}")))?;
        let frame = SceneFrame::new(Arc::clone(&runtime.driver));
        Ok(Self {
            driver: runtime.driver,
            progress,
            render_ctx: runtime.render_ctx,
            frame,
        })
    }

    /// 本帧渲染上下文。
    pub fn render_context(&self) -> RenderContext {
        self.render_ctx.clone()
    }

    /// 场景推进器句柄（测试 / headless 与 GUI 共用同一条命令路径）。
    pub fn driver(&self) -> Arc<DriverPod> {
        Arc::clone(&self.driver)
    }
}

impl EframeApp for App {
    fn ui(&mut self, ui: &mut gui::Ui, _frame: &mut eframe::Frame) {
        // 帧体（视图 / 输入 / 命令 / 呈现 / 可观测性）在 UI 层实现；
        // 壳只把事件订阅方收到的进度交出去。
        self.frame
            .ui(ui, (self.progress.frames(), self.progress.time()));
    }
}
