// param_panel.rs — P4 降级方案：参数面板
//
// 在 xcal-app 侧提供一个 egui 面板，直接编辑 BindingTable 里的
// Curve::Sin / Curve::Orbit 参数。这是节点编辑器的简化版，后续替换为
// 完整节点图（xc-node-schema + xcal-node-egui）时上层逻辑不动。

use egui::Panel;
use xcal_core::animation::BindingTable;
use xcal_core::context::ExecutorContext;
use xcal_core::system::System;
use xcal_core::TimeState;
use xcal_core::Curve;
use xcal_egui::resources::EguiFrameContext;

fn extract_ui_ptr(ctx: &mut ExecutorContext<'_>) -> *mut egui::Ui {
    ctx.get::<EguiFrameContext>()
        .map(|c| c.ui_ptr())
        .unwrap_or(std::ptr::null_mut())
}

pub struct ParamPanelSystem;
impl System for ParamPanelSystem {
    type Reads = (TimeState, EguiFrameContext);
    type Writes = (BindingTable,);

    fn run(&mut self, ctx: &mut ExecutorContext<'_>) {
        let ui_ptr = extract_ui_ptr(ctx);
        if ui_ptr.is_null() {
            return;
        }
        let ui = unsafe { &mut *ui_ptr };

        // 克隆出 BindingTable 修改，避免同时持有 &mut ctx 和 &mut table。
        let mut table = ctx.get::<BindingTable>().cloned().unwrap_or_default();

        Panel::right("params")
            .resizable(true)
            .default_size(200.0)
            .show(ui, |ui| {
                ui.heading("动画参数");
                ui.separator();

                // 找 Circle 的 Sin 曲线（index 0）
                if let Some(binding) = table.bindings.get_mut(0) {
                    if let Curve::Sin { base, amp, omega } = &mut binding.curve {
                        ui.label("圆半径");
                        ui.add(egui::Slider::new(base, 0.0..=3.0).text("base").step_by(0.1));
                        ui.add(egui::Slider::new(amp, 0.0..=2.0).text("amp").step_by(0.05));
                        ui.add(egui::Slider::new(omega, 0.0..=8.0).text("omega").step_by(0.1));
                    }
                }

                ui.separator();

                // 找 sin 曲线的 Phase（index 1）
                if let Some(binding) = table.bindings.get_mut(1) {
                    if let Curve::Phase { omega } = &mut binding.curve {
                        ui.label("函数曲线相位");
                        ui.add(egui::Slider::new(omega, 0.0..=5.0).text("omega").step_by(0.1));
                    }
                }

                ui.separator();

                // 找 3D 轨道相机（index 2）
                if let Some(binding) = table.bindings.get_mut(2) {
                    if let Curve::Orbit { radius, speed, initial_angle, .. } = &mut binding.curve {
                        ui.label("3D 轨道相机");
                        ui.add(egui::Slider::new(radius, 1.0..=20.0).text("radius").step_by(0.5));
                        ui.add(egui::Slider::new(speed, -2.0..=2.0).text("speed").step_by(0.1));
                        ui.add(egui::Slider::new(initial_angle, -3.14..=3.14).text("angle").step_by(0.1));
                    }
                }
            });

        // 写回
        if let Some(t) = ctx.get_mut::<BindingTable>() {
            *t = table;
        }
    }
}
