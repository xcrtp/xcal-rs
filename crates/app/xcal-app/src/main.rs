use eframe::egui;
use xcal_app::App;

fn main() -> eframe::Result<()> {
    // 结构化日志：过滤器由 XCAL_LOG 控制（默认 xcal=debug,warn）。
    xcal_core::telemetry::init_tracing();

    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default().with_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("xcal", options, Box::new(|cc| Ok(Box::new(App::new(cc)?))))
}
