use std::process::{Child, Command};
use std::{env, str};

use eframe::egui;

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct P4Config {
    pub p4_port: String,   // e.g., "localhost:1666"
    pub p4_user: String,   // e.g., "admin"
    pub p4_client: String, // e.g., "my_workspace"
}

pub fn ui_p4config(ui: &mut egui::Ui, config: &mut P4Config) -> egui::Response {
    ui.vertical(|ui| {
        ui.heading("Perforce Configuration");
        ui.label("Uses $P4PORT, $P4USER, and $P4CLIENT if not specified here");

        if ui.button("Reset to Defaults").clicked() {
            *config = P4Config::default();
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

pub struct P4Command {
    config: Option<P4Config>,
    exe_path: String,
    _is_gui: bool,
}

impl P4Command {
    pub fn new(is_gui: bool) -> Self {
        let mut exe_path = env::var("P4PATH").unwrap_or_else(|_| "p4.exe".to_string());
        if is_gui {
            exe_path = exe_path.replace("p4.exe", "p4vc.bat")
        }

        Self {
            config: None,
            exe_path,
            _is_gui: is_gui,
        }
    }

    pub fn with_config(mut self, config: P4Config) -> Self {
        self.config = Some(config);
        self
    }

    fn prepare_cmd(&self) -> Command {
        let mut cmd = Command::new(&self.exe_path);

        if let Some(config) = &self.config {
            if !config.p4_port.is_empty() {
                cmd.env("P4PORT", &config.p4_port);
            }
            if !config.p4_user.is_empty() {
                cmd.env("P4USER", &config.p4_user);
            }
            if !config.p4_client.is_empty() {
                cmd.env("P4CLIENT", &config.p4_client);
            }
        }
        // Extremely important, will silently fail
        if env::var("P4CHARSET").is_err() {
            cmd.args(["-C", "utf8"]);
        }
        cmd
    }

    pub fn output(&self, args: &[&str]) -> Result<String, String> {
        let output = self
            .prepare_cmd()
            .args(args)
            .output()
            .map_err(|e| e.to_string())?;

        if output.status.success() {
            String::from_utf8(output.stdout).map_err(|e| e.to_string())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).to_string())
        }
    }

    pub fn spawn(&self, args: &[&str]) -> Result<Child, String> {
        self.prepare_cmd()
            .args(args)
            .spawn()
            .map_err(|e| e.to_string())
    }

    pub fn get_depot_file_content(path: &str, config: Option<P4Config>) -> Result<String, String> {
        let mut cmd = P4Command::new(false);
        if let Some(config) = config {
            cmd = cmd.with_config(config);
        }
        cmd.output(&["print", "-q", path])
    }
    pub fn open_revision_graph(path: &str, config: Option<P4Config>) -> Result<(), String> {
        let mut cmd = P4Command::new(true);
        if let Some(config) = config {
            cmd = cmd.with_config(config);
        }
        cmd.spawn(&["revisiongraph", path])?;
        Ok(())
    }
    #[allow(dead_code)]
    pub fn get_revision_history(path: &str, config: Option<P4Config>) -> Result<String, String> {
        let mut cmd = P4Command::new(false);
        if let Some(config) = config {
            cmd = cmd.with_config(config);
        }
        cmd.output(&["-ztag", "filelog", "-m", "10", path])
    }
}

#[allow(dead_code)]
pub struct P4Revision {
    pub rev: u32,
    pub change: u32,
    pub action: String,
    pub date: String,
}
