use std::path::PathBuf;

use eframe::egui::{self, Modifiers, Ui};
use zhashdiff::fs::FsEntry;

use crate::ui_egui::panes::DuplicateFilesPaneCtx;

pub fn hash_to_color(hash: &str) -> egui::Color32 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::Hash;

    let hue_digit = hash
        .chars()
        .next()
        .and_then(|c| c.to_digit(16))
        .unwrap_or(0) as f32;
    let shade_digit = hash
        .chars()
        .nth(1)
        .and_then(|c| c.to_digit(16))
        .unwrap_or(0) as f32;

    let mut hasher = DefaultHasher::new();
    hash.hash(&mut hasher);

    let hue = hue_digit / 16.0;

    let s_base = 0.4 + (shade_digit / 16.0) * 0.4;
    let v_base = 0.6 + (1.0 - (shade_digit / 16.0)) * 0.3;

    let saturation = (s_base).clamp(0.3, 0.95);
    let value = (v_base).clamp(0.4, 0.95);

    egui::Color32::from(egui::ecolor::Hsva::new(hue, saturation, value, 1.0))
}

#[derive(PartialEq, Eq, Clone)]
pub enum CheckboxSelectState {
    Unchecked,
    Checked,
    Partial,
}

pub fn ui_custom_checkbox(
    ui: &mut egui::Ui,
    state: CheckboxSelectState,
) -> egui::response::Response {
    let icon_size = ui.spacing().icon_width;
    let icon_rect = egui::Vec2::splat(icon_size);

    let (rect, response) = ui.allocate_exact_size(ui.spacing().interact_size, egui::Sense::click());
    let visual_rect = egui::Rect::from_center_size(rect.center(), icon_rect);

    if ui.is_rect_visible(visual_rect) {
        let visuals = ui.style().interact(&response);
        let painter = ui.painter();
        let rounding = ui.visuals().widgets.active.corner_radius;

        // Background
        let bg_fill = if state != CheckboxSelectState::Unchecked {
            visuals.bg_fill
        } else {
            ui.visuals().gray_out(visuals.bg_fill)
        };
        painter.rect_filled(visual_rect, rounding, bg_fill);

        // Border
        painter.rect_stroke(
            visual_rect,
            rounding,
            visuals.bg_stroke,
            egui::StrokeKind::Middle,
        );

        let stroke = visuals.fg_stroke;
        match state {
            CheckboxSelectState::Checked => {
                let points = vec![
                    visual_rect.center() + egui::vec2(-icon_size * 0.25, 0.0),
                    visual_rect.center() + egui::vec2(-icon_size * 0.05, icon_size * 0.2),
                    visual_rect.center() + egui::vec2(icon_size * 0.3, -icon_size * 0.25),
                ];
                painter.add(egui::Shape::line(points, stroke));
            }
            CheckboxSelectState::Partial => {
                let dash_rect = egui::Rect::from_center_size(
                    visual_rect.center(),
                    egui::vec2(icon_size * 0.5, 2.0),
                );
                painter.rect_filled(dash_rect, 0.0, stroke.color);
            }
            CheckboxSelectState::Unchecked => {}
        }
    }
    response
}
