// time_control.rs — 时间控制 UI（LogicUpdate 阶段）
//
// 播放/暂停按钮、速度滑块、时间轴拖动条（seek）。
// 读 TimeState + EguiFrameContext，写 DirectorState。

use egui::Panel;

use xcal_core::context::ExecutorContext;
use xcal_core::system::System;
use xcal_core::{DirectorState, TimeState};

use crate::resources::EguiFrameContext;

fn extract_ui_ptr(ctx: &mut ExecutorContext<'_>) -> *mut egui::Ui {
    ctx.get::<EguiFrameContext>()
        .map(|c| c.ui_ptr())
        .unwrap_or(std::ptr::null_mut())
}

pub struct TimeControlSystem;
impl System for TimeControlSystem {
    type Reads = (TimeState, EguiFrameContext);
    type Writes = (DirectorState,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let ui_ptr = extract_ui_ptr(ctx);
        if ui_ptr.is_null() {
            return;
        }
        let ui = unsafe { &mut *ui_ptr };

        let time = ctx.get::<TimeState>().map(|t| t.elapsed).unwrap_or(0.0);
        let mut dir = ctx.get::<DirectorState>().cloned().unwrap_or_default();

        Panel::left("time_control")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.heading("时间控制");
                ui.label(format!("t = {:.3}", time));
                ui.separator();

                // 播放/暂停切换
                let label = if dir.director.playing { "⏸ 暂停" } else { "▶ 播放" };
                if ui.button(label).clicked() {
                    dir.director.toggle();
                }

                // 速度滑块
                ui.separator();
                ui.label("速度");
                let mut speed_f = dir.director.speed as f32;
                if ui
                    .add(egui::Slider::new(&mut speed_f, -3.0..=3.0))
                    .changed()
                {
                    dir.director.set_speed(speed_f as f64);
                }

                // 时间轴拖动条（seek）——只在用户真的拖完才 seek，
                // step_by 会把 global_t 对齐到 0.1 网格，速度慢时 knob 卡 0。
                ui.separator();
                ui.label("时间轴（seek）");
                let mut t_f = dir.director.global_t as f32;
                let resp = ui.add(
                    egui::Slider::new(&mut t_f, -10.0..=30.0)
                        .text("t"),
                );
                if resp.drag_stopped() {
                    dir.director.seek(t_f as f64);
                }
            });

        if let Some(d) = ctx.get_mut::<DirectorState>() {
            *d = dir;
        }
    }
}
