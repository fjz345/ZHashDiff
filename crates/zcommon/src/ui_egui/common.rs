use eframe::egui::{self, Vec2};

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
#[allow(dead_code)]
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

#[allow(dead_code)]
pub fn preview_files_being_dropped_in_rect(ctx: &egui::Context, rect: egui::Rect, _label: &str) {
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

pub fn draw_persistent_hint_text_edit(
    ui: &mut egui::Ui,
    id_source: impl std::hash::Hash,
    hint: String,
    size: impl Into<Vec2>,
) -> egui::Response {
    let id = ui.make_persistent_id(id_source);
    let mut text_edit_val = ui.data_mut(|d| d.get_persisted::<String>(id).unwrap_or_default());

    let size = size.into();
    let output = ui
        .allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
            egui::TextEdit::singleline(&mut text_edit_val)
                .clip_text(true)
                .desired_width(size.x)
                .show(ui)
        })
        .inner;

    let painter = ui.painter_at(output.response.rect);
    let text_color = egui::Color32::from_rgba_premultiplied(100, 100, 100, 100);
    let galley = painter.layout(
        hint,
        egui::TextStyle::Body.resolve(ui.style()),
        text_color,
        size.x,
    );

    painter.galley(output.galley_pos, galley, text_color);

    if output.response.changed() {
        ui.data_mut(|d| d.insert_persisted(id, text_edit_val));
    }

    output.response
}

pub fn show_custom_popup<F>(
    ctx: &egui::Context,
    open: &mut bool,
    title: &str,
    auto_size: bool,
    add_contents: F,
) where
    F: FnOnce(&mut egui::Ui),
{
    let mut window = egui::Window::new(title)
        .open(open)
        .resizable(true)
        .collapsible(false)
        .default_size([300.0, 150.0]);
    if auto_size {
        window = window.auto_sized();
    }

    window.show(ctx, |ui| {
        add_contents(ui);
    });
}

// Want to only change the title bar background color, but could not manage how to...
pub fn show_custom_popup_with_color<F>(
    ctx: &egui::Context,
    open: &mut bool,
    title: &str,
    title_color: egui::Color32,
    add_contents: F,
) where
    F: FnOnce(&mut egui::Ui),
{
    let original_visuals = ctx.style().visuals.clone();
    let mut title_visuals = original_visuals.clone();

    title_visuals.window_fill = title_color;
    title_visuals.override_text_color = Some(egui::Color32::BLACK);
    title_visuals.widgets.active.fg_stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);
    title_visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, egui::Color32::BLACK);

    ctx.set_visuals(title_visuals);

    egui::Window::new(title)
        .open(open)
        .resizable(true)
        .collapsible(false)
        .default_size([300.0, 150.0])
        .frame(
            egui::Frame::window(&ctx.style())
                .fill(title_color)
                .inner_margin(0.0),
        )
        .show(ctx, |ui| {
            ui.ctx().set_visuals(original_visuals.clone());

            ui.scope(|ui| {
                egui::Frame::new().inner_margin(8.0).show(ui, |ui| {
                    ui.set_min_width(ui.available_width());
                    ui.set_min_height(ui.available_height());

                    add_contents(ui);
                });
            });
        });
}
