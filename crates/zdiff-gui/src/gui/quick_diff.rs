use std::io::Write;
use std::path::PathBuf;

use eframe::egui::{self};
use tempfile::NamedTempFile;
use zdiff::universal_path::UniversalPath;

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

pub fn quick_diff_process_paths(
    path_source: &UniversalPath,
    path_target: &UniversalPath,
) -> (PathBuf, PathBuf) {
    let handle_path = |u_path: &UniversalPath| -> PathBuf {
        match u_path {
            UniversalPath::Local(p) => p.to_path_buf(),
            UniversalPath::Depot(_depot_str) => {
                let mut tmp = NamedTempFile::new().expect("Failed to create tmp file");

                // could call 'p4 print' or similar
                tmp.write_all(b"Contents of depot file")
                    .expect("Write failed");

                tmp.path().to_path_buf()
            }
        }
    };

    (handle_path(path_source), handle_path(path_target))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parsing() {
        let depot = UniversalPath::new("//stream/main/file.txt");
        assert!(matches!(depot, UniversalPath::Depot(_)));
        assert_eq!(depot.to_p4_string(), "//stream/main/file.txt");

        let local = UniversalPath::new(r"C:\User\File.txt");
        assert!(matches!(local, UniversalPath::Local(_)));
        assert_eq!(local.to_p4_string(), "C:/User/File.txt");
    }
}
