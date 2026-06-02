use std::process::{Child, Command};
use std::{env, str};

use eframe::egui;

use std::sync::RwLock;

static GLOBAL_P4_CONFIG: std::sync::OnceLock<RwLock<P4Config>> = std::sync::OnceLock::new();
pub fn get_p4_config() -> P4Config {
    let config = GLOBAL_P4_CONFIG
        .get_or_init(|| RwLock::new(P4Config::default()))
        .read()
        .unwrap()
        .clone();
    log::trace!("Reading P4 config: {:?}", config);
    config
}
pub fn update_p4_config(new_config: P4Config) {
    log::trace!("Updating P4 config: {:?}", new_config);
    if let Some(lock) = GLOBAL_P4_CONFIG.get() {
        let mut w = lock.write().unwrap();
        *w = new_config;
    } else {
        let _ = GLOBAL_P4_CONFIG.set(RwLock::new(new_config));
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct P4Config {
    pub p4_port: String,    // e.g., "localhost:1666"
    pub p4_user: String,    // e.g., "admin"
    pub p4_client: String,  // e.g., "my_workspace"
    pub p4_charset: String, // e.g., "utf8"
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
                edit_row("P4CHARSET", &mut config.p4_charset, "utf8");
            });

        ui.add_space(10.0);

        ui.button("Save Settings")
    })
    .inner
}

pub struct P4Command {
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
            exe_path,
            _is_gui: is_gui,
        }
    }

    fn prepare_cmd(&self) -> Command {
        let mut cmd = Command::new(&self.exe_path);

        // Extremely important, will silently fail
        if env::var("P4CHARSET").is_err() {
            cmd.args(["-C", "utf8"]);
        }

        let config = get_p4_config();
        if !config.p4_port.is_empty() {
            cmd.env("P4PORT", &config.p4_port);
        }
        if !config.p4_user.is_empty() {
            cmd.env("P4USER", &config.p4_user);
        }
        if !config.p4_client.is_empty() {
            cmd.env("P4CLIENT", &config.p4_client);
        }
        if !config.p4_charset.is_empty() {
            cmd.env("P4CHARSET", &config.p4_charset);
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

    pub fn get_depot_file_content(path: &str) -> Result<String, String> {
        P4Command::new(false).output(&["print", "-q", path])
    }
    pub fn open_revision_graph(path: &str) -> Result<(), String> {
        P4Command::new(true).spawn(&["revisiongraph", path])?;
        Ok(())
    }
    pub fn open_timelapse_view(path: &str) -> Result<(), String> {
        P4Command::new(true).spawn(&["timelapse", path])?;
        Ok(())
    }
    #[allow(dead_code)]
    pub fn get_revision_history(path: &str) -> Result<String, String> {
        P4Command::new(false).output(&["-ztag", "filelog", "-m", "10", path])
    }
}

#[allow(dead_code)]
pub struct P4Revision {
    pub rev: u32,
    pub change: u32,
    pub action: String,
    pub date: String,
}
