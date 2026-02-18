use eframe::egui;

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

/// Preview hovering files:
pub fn preview_files_being_dropped(ctx: &egui::Context) {
    use egui::{Align2, Color32, Id, LayerId, Order, TextStyle};
    use std::fmt::Write as _;

    if !ctx.input(|i| i.raw.hovered_files.is_empty()) {
        let text = ctx.input(|i| {
            let mut text = "Dropping files:\n".to_owned();
            for file in &i.raw.hovered_files {
                if let Some(path) = &file.path {
                    write!(text, "\n{}", path.display()).ok();
                } else if !file.mime.is_empty() {
                    write!(text, "\n{}", file.mime).ok();
                } else {
                    text += "\n???";
                }
            }
            text
        });

        let painter =
            ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("file_drop_target")));

        let content_rect = ctx.content_rect();
        painter.rect_filled(content_rect, 0.0, Color32::from_black_alpha(192));
        painter.text(
            content_rect.center(),
            Align2::CENTER_CENTER,
            text,
            TextStyle::Heading.resolve(&ctx.style()),
            Color32::WHITE,
        );
    }
}

pub fn preview_files_being_dropped_in_rect(ctx: &egui::Context, rect: egui::Rect, label: &str) {
    use egui::{Align2, Color32, Id, LayerId, Order, TextStyle};
    use std::fmt::Write as _;

    let mut hover_pos = None;
    let mut latest_pos = None;
    if !ctx.input(|i| {
        hover_pos = i.pointer.hover_pos();
        latest_pos = i.pointer.latest_pos();
        i.raw.hovered_files.is_empty()
    }) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
            egui::WindowLevel::AlwaysOnTop,
        ));
        let text = ctx.input(|i| {
            let mut text = "Dropping files:\n".to_owned();
            for file in &i.raw.hovered_files {
                if let Some(path) = &file.path {
                    write!(text, "\n{}", path.display()).ok();
                } else if !file.mime.is_empty() {
                    write!(text, "\n{}", file.mime).ok();
                } else {
                    text += "\n???";
                }
            }
            text
        });

        log::error!("HOVER POS: {:?}", hover_pos);
        log::error!("LATEST POS: {:?}", latest_pos);
        let Some(pos) = hover_pos else {
            return;
        };

        // 3. Check if that position is inside our column rect
        if rect.contains(pos) {
            let painter =
                ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("file_drop_target")));

            let content_rect = ctx.content_rect();
            painter.rect_filled(content_rect, 0.0, Color32::from_black_alpha(192));
            painter.text(
                content_rect.center(),
                Align2::CENTER_CENTER,
                text,
                TextStyle::Heading.resolve(&ctx.style()),
                Color32::WHITE,
            );
        }
    }
}
