use eframe::egui;
use xcal::App;

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default().with_inner_size([800.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native("xcal", options, Box::new(|cc| Ok(Box::new(App::new(cc)?))))
}
