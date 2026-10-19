//! `draw_property` — enum-dispatched mapping from a [`Property`] schema to the
//! natural egui widget.
//!
//! Signature contract: `draw_property(ui, prop) -> Option<PropertyValue>`.
//!   * `None`  — the widget did not change (no edit this frame).
//!   * `Some(v)` — the user edited the value; `v` is the new value, already
//!     constrained by the schema's bounds. The caller is expected to feed `v`
//!     back into the node via [`crate::NodeSpec::apply_property`] so the node
//!     becomes the single source of truth (the next `properties()` call will
//!     return the updated schema).
//!
//! Why this shape: the `Property` enum carries the *schema + current value*
//! together, so the widget can read `c.value` for display and write back into
//! `c.value` directly. The returned `PropertyValue` is for the caller's
//! `apply_property` round-trip — the schema mutation here is a side-effect
//! mirror so the widget stays in sync until the node's `apply_property`
//! completes (which may further validate / reject).
//!
//! No `downcast_mut` is used anywhere in this file: the dispatch is purely
//! `match Property { ... }`, exhaustive on every variant.

use egui::Ui;

use ze_node_schema::{
    BoolConfig, ColorConfig, DoubleConfig, EnumConfig, IntegerConfig, Property, PropertyValue,
    RangeConfig, ScalarConfig, StringConfig, StringKind, VEC_MAX_COMPONENTS, VecNConfig,
};

/// Render the appropriate widget for `prop`. Returns `Some(new_value)` if the
/// widget was edited this frame, so the caller can `apply_property(name, v)`.
///
/// Variants not yet mapped to a widget (Function, Dynamic, List) are rendered
/// as a read-only label and return `None`; they can be enriched later without
/// breaking the call sites (forward-compatible per plan §4).
pub fn draw_property(ui: &mut Ui, prop: &mut Property) -> Option<PropertyValue> {
    match prop {
        Property::Scalar(c) => draw_scalar(ui, c).map(PropertyValue::Scalar),
        Property::Double(c) => draw_double(ui, c).map(PropertyValue::Double),
        Property::Integer(c) => draw_integer(ui, c).map(PropertyValue::Integer),
        Property::Bool(c) => draw_bool(ui, c).map(PropertyValue::Bool),
        Property::String(c) => draw_string(ui, c).map(PropertyValue::String),
        Property::VecN(c) => draw_vecn(ui, c).map(|v| PropertyValue::VecN {
            size: c.size,
            components: v,
        }),
        Property::Color(c) => draw_color(ui, c).map(PropertyValue::Color),
        Property::Enum(c) => draw_enum(ui, c).map(PropertyValue::Enum),
        Property::Range(c) => draw_range(ui, c).map(|(lo, hi)| PropertyValue::Range { lo, hi }),
        Property::Function(c) => {
            ui.label(format!("{}: (function)", c.label));
            None
        }
        Property::Dynamic(c) => {
            ui.label(format!("{}: (dynamic, {} fields)", c.label, c.value.len()));
            None
        }
        Property::List(c) => {
            ui.label(format!("{}: (list, {} items)", c.label, c.values.len()));
            None
        }
    }
}

fn draw_scalar(ui: &mut Ui, c: &mut ScalarConfig) -> Option<f32> {
    let mut v = c.value;
    // Sliders default to a fixed `spacing.slider_width`; stretch them to the
    // card's body width so they grow/shrink live with a card resize.
    ui.spacing_mut().slider_width = ui.available_width();
    let slider = egui::Slider::new(&mut v, c.min..=c.max)
        .text(&c.label)
        .step_by(c.step.unwrap_or(0.0) as f64);
    let resp = ui.add(slider);
    if resp.changed() {
        c.value = v;
        Some(v)
    } else {
        None
    }
}

fn draw_double(ui: &mut Ui, c: &mut DoubleConfig) -> Option<f64> {
    let mut v = c.value;
    ui.spacing_mut().slider_width = ui.available_width();
    let slider = egui::Slider::new(&mut v, c.min..=c.max)
        .text(&c.label)
        .step_by(c.step.unwrap_or(0.0));
    let resp = ui.add(slider);
    if resp.changed() {
        c.value = v;
        Some(v)
    } else {
        None
    }
}

fn draw_integer(ui: &mut Ui, c: &mut IntegerConfig) -> Option<i64> {
    let mut v = c.value;
    ui.spacing_mut().slider_width = ui.available_width();
    let slider = egui::Slider::new(&mut v, c.min..=c.max)
        .text(&c.label)
        .step_by(c.step.unwrap_or(0) as f64);
    let resp = ui.add(slider);
    if resp.changed() {
        c.value = v;
        Some(v)
    } else {
        None
    }
}

fn draw_bool(ui: &mut Ui, c: &mut BoolConfig) -> Option<bool> {
    let mut v = c.value;
    let resp = ui.checkbox(&mut v, &c.label);
    if resp.changed() {
        c.value = v;
        Some(v)
    } else {
        None
    }
}

fn draw_string(ui: &mut Ui, c: &mut StringConfig) -> Option<String> {
    let mut v = c.value.clone();
    // Fields fill the card body width so they reflow on resize.
    let width = ui.available_width();
    let resp = match c.kind {
        StringKind::Text => ui.add(
            egui::TextEdit::singleline(&mut v)
                .hint_text(&c.label)
                .desired_width(width),
        ),
        StringKind::Multiline => ui.add(
            egui::TextEdit::multiline(&mut v)
                .hint_text(&c.label)
                .desired_width(width),
        ),
        StringKind::Path => ui.add(
            egui::TextEdit::singleline(&mut v)
                .hint_text(&c.label)
                .desired_width(width),
        ),
        StringKind::Password => ui.add(
            egui::TextEdit::singleline(&mut v)
                .password(true)
                .hint_text(&c.label)
                .desired_width(width),
        ),
    };
    if resp.changed() {
        c.value = v.clone();
        Some(v)
    } else {
        None
    }
}

fn draw_vecn(ui: &mut Ui, c: &mut VecNConfig) -> Option<[f32; VEC_MAX_COMPONENTS]> {
    let size = c.size as usize;
    let mut changed = false;
    let mut comps = c.components;
    ui.horizontal(|ui| {
        ui.label(&c.label);
        for i in 0..size {
            let mut v = comps[i];
            let range = match (c.min, c.max) {
                (Some(lo), Some(hi)) => lo[i]..=hi[i],
                (Some(lo), None) => lo[i]..=f32::INFINITY,
                (None, Some(hi)) => f32::NEG_INFINITY..=hi[i],
                (None, None) => f32::NEG_INFINITY..=f32::INFINITY,
            };
            // Split the remaining row width evenly across the sliders that
            // still need placing, so the row fills the card body width.
            ui.spacing_mut().slider_width = ui.available_width() / (size - i) as f32;
            let slider = egui::Slider::new(&mut v, range).step_by(c.step.unwrap_or(0.0) as f64);
            let resp = ui.add(slider);
            if resp.changed() {
                comps[i] = v;
                changed = true;
            }
        }
    });
    if changed {
        c.components = comps;
        Some(comps)
    } else {
        None
    }
}

fn draw_color(ui: &mut Ui, c: &mut ColorConfig) -> Option<[u8; 4]> {
    // egui's `color_edit_button_srgba` takes `&mut Color32`; we round-trip
    // through `Color32` and back to our `[u8; 4]` storage.
    let [r, g, b, a] = c.value;
    let mut color = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
    let resp = ui.color_edit_button_srgba(&mut color);
    if resp.changed() {
        let [nr, ng, nb, na] = color.to_srgba_unmultiplied();
        let out = if c.with_alpha {
            [nr, ng, nb, na]
        } else {
            // Preserve the original alpha.
            [nr, ng, nb, a]
        };
        c.value = out;
        Some(out)
    } else {
        None
    }
}

fn draw_enum(ui: &mut Ui, c: &mut EnumConfig) -> Option<String> {
    let mut selected = c.value.clone();
    let inner = egui::ComboBox::from_label(&c.label)
        .selected_text(&selected)
        .show_ui(ui, |ui| {
            for opt in &c.options {
                ui.selectable_value(&mut selected, opt.clone(), opt);
            }
        });
    if inner.response.changed() && c.options.contains(&selected) {
        c.value = selected.clone();
        Some(selected)
    } else {
        None
    }
}

fn draw_range(ui: &mut Ui, c: &mut RangeConfig) -> Option<(f64, f64)> {
    let mut lo = c.value[0];
    let mut hi = c.value[1];
    ui.spacing_mut().slider_width = ui.available_width();
    let resp = ui.add(
        egui::Slider::new(&mut lo, c.bound_min..=c.bound_max)
            .text("lo")
            .clamping(egui::SliderClamping::Always),
    );
    ui.spacing_mut().slider_width = ui.available_width();
    let resp2 = ui.add(
        egui::Slider::new(&mut hi, c.bound_min..=c.bound_max)
            .text("hi")
            .clamping(egui::SliderClamping::Always),
    );
    if resp.changed() || resp2.changed() {
        if lo > hi {
            std::mem::swap(&mut lo, &mut hi);
        }
        c.value = [lo, hi];
        Some((lo, hi))
    } else {
        None
    }
}
