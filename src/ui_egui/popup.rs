use eframe::egui;

pub fn show_custom_popup<F>(ctx: &egui::Context, open: &mut bool, title: &str, add_contents: F)
where
    F: FnOnce(&mut egui::Ui),
{
    // We pass the open bool directly. egui::Window handles
    // the internal logic of setting it to false if "X" is clicked.
    egui::Window::new(title)
        .open(open)
        .resizable(true)
        .collapsible(false)
        .default_size([300.0, 150.0])
        .show(ctx, |ui| {
            add_contents(ui);
        });
}
