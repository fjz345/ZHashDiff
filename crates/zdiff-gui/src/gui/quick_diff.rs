use eframe::egui::{self};

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UniversalPathConfig {
    pub p4_port: String,   // e.g., "localhost:1666"
    pub p4_user: String,   // e.g., "admin"
    pub p4_client: String, // e.g., "my_workspace"
}

pub fn ui_universal_path(ui: &mut egui::Ui, config: &mut UniversalPathConfig) -> egui::Response {
    ui.vertical(|ui| {
        ui.heading("Perforce Configuration");
        ui.label("Uses $P4PORT, $P4USER, and $P4CLIENT if not specified here");

        if ui.button("Reset to Defaults").clicked() {
            *config = UniversalPathConfig::default();
        }

        ui.separator();

        egui::Grid::new("p4_settings_grid")
            .num_columns(2)
            .spacing([40.0, 8.0])
            .show(ui, |ui| {
                let mut edit_row = |label: &str, value: &mut String, hint: &str| {
                    ui.label(label);
                    ui.add(
                        egui::TextEdit::singleline(value)
                            .hint_text(hint)
                            .desired_width(200.0),
                    );
                    ui.end_row();
                };

                edit_row("P4PORT", &mut config.p4_port, "ssl:perforce:1666");
                edit_row("P4USER", &mut config.p4_user, "username");
                edit_row("P4CLIENT", &mut config.p4_client, "workspace_name");
            });

        ui.add_space(10.0);

        ui.button("Save Settings")
    })
    .inner
}
