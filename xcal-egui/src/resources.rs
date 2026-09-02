// resources.rs: egui 层的全局 Resource 类型 + EguiFrameContext

use egui::Pos2;
use xcal_core::types::{Camera2D, Rect};

// ─── EguiFrameContext ──────────────────────────────────────────────────────

pub struct EguiFrameContext {
    ui_ptr: *mut egui::Ui,
}

unsafe impl Send for EguiFrameContext {}
unsafe impl Sync for EguiFrameContext {}

impl EguiFrameContext {
    pub fn new(ui: &mut egui::Ui) -> Self {
        Self {
            ui_ptr: ui as *mut egui::Ui,
        }
    }

    pub fn ui_ptr(&self) -> *mut egui::Ui {
        self.ui_ptr
    }
}

// ─── AppCamera ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AppCamera {
    pub center: Pos2,
    pub scale: f32,
}

impl Default for AppCamera {
    fn default() -> Self {
        Self {
            center: Pos2::new(0.0, 0.0),
            scale: 40.0,
        }
    }
}

impl AppCamera {
    pub fn handle_input(&mut self, ui: &egui::Ui) {
        let input = ui.input(|i| i.clone());
        let scroll = input.smooth_scroll_delta.y;
        if scroll != 0.0 {
            let zoom_factor = 1.0 + scroll * 0.001;
            self.scale = (self.scale * zoom_factor).clamp(5.0, 500.0);
        }
        if input.pointer.secondary_down() || input.pointer.middle_down() {
            let delta = input.pointer.delta();
            self.center.x -= delta.x / self.scale;
            self.center.y += delta.y / self.scale;
        }
    }

    pub fn to_camera_2d(&self, rect: Rect) -> Camera2D {
        Camera2D::new((self.center.x, self.center.y).into(), self.scale, rect)
    }
}

// ─── TimeState ─────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct TimeState {
    pub elapsed: f32,
}

// ─── UiConfig ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct UiConfig {
    pub show_grid: bool,
    pub show_field: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            show_grid: true,
            show_field: false,
        }
    }
}

// ─── ViewRect ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ViewRect {
    pub rect: Rect,
}

impl Default for ViewRect {
    fn default() -> Self {
        Self {
            rect: Rect::new((0.0, 0.0).into(), (800.0, 600.0).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xcal_core::resource::Resources;

    #[test]
    fn test_app_camera_default() {
        let c = AppCamera::default();
        assert_eq!(c.center, Pos2::new(0.0, 0.0));
        assert_eq!(c.scale, 40.0);
    }

    #[test]
    fn test_ui_config_default() {
        let cfg = UiConfig::default();
        assert!(cfg.show_grid);
        assert!(!cfg.show_field);
    }

    #[test]
    fn test_resources_remove() {
        let mut r = Resources::new();
        r.insert(42_i32);
        assert!(r.get::<i32>().is_some());
        let removed = r.remove::<i32>();
        assert_eq!(removed, Some(42));
        assert!(r.get::<i32>().is_none());
    }
}
