use eframe::egui;

pub fn show_custom_popup<F>(ctx: &egui::Context, open: &mut bool, title: &str, add_contents: F)
where
    F: FnOnce(&mut egui::Ui),
{
    egui::Window::new(title)
        .open(open)
        .resizable(true)
        .collapsible(false)
        .default_size([300.0, 150.0])
        .show(ctx, |ui| {
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
