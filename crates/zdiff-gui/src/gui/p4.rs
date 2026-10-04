use std::path::PathBuf;
use std::process::{Child, Command};
use std::{env, str};

use eframe::egui;
use zdiff::universal_path::UniversalPath;

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
    pub port: String,     // e.g., "localhost:1666"
    pub user: String,     // e.g., "admin"
    pub client: String,   // e.g., "my_workspace"
    pub charset: String,  // e.g., "utf8"
    pub password: String, // e.g., "secret"
}

pub fn ui_p4config(ui: &mut egui::Ui, config: &mut P4Config) {
    ui.vertical(|ui| {
        ui.heading("Perforce Configuration");
        ui.label("Uses $P4PORT, $P4USER, and $P4CLIENT if not specified here");
        ui.label("# to unset an env var for the command");

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

                edit_row("P4PORT", &mut config.port, "ssl:perforce:1666");
                edit_row("P4USER", &mut config.user, "username");
                edit_row("P4CLIENT", &mut config.client, "workspace_name");
                edit_row("P4CHARSET", &mut config.charset, "utf8");
            });

        ui.add_space(10.0);
    });
}

/// Runs a p4 command: stdout on success, the error text on failure. A trait so logic that asks
/// p4 can be tested with a fake.
pub trait P4Runner {
    fn run(&self, args: &[&str]) -> Result<String, String>;
}

impl P4Runner for P4Command {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        self.output(args)
    }
}

/// A local path as a p4 file argument. p4 reads `@ # *` as revision and wildcard syntax and `%`
/// as its escape, so `p4 edit a*.txt` would open every match.
pub fn escape_local_path(path: &str) -> String {
    let mut escaped = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            '%' => escaped.push_str("%25"),
            '@' => escaped.push_str("%40"),
            '#' => escaped.push_str("%23"),
            '*' => escaped.push_str("%2A"),
            _ => escaped.push(c),
        }
    }
    escaped
}

/// How one p4 command's environment differs from the app's.
#[derive(Debug, Default, PartialEq)]
pub struct P4Env {
    pub set: Vec<(&'static str, String)>,
    pub unset: Vec<&'static str>,
    pub cwd: Option<PathBuf>,
}

/// A non-empty field sets its variable, `#` unsets it, and an empty one leaves the inherited
/// value. A local file's directory becomes the working directory so p4 finds the P4CONFIG file
/// of the repository that file lives in; a depot path has no directory to pick one, and a
/// relative path keeps the app's cwd.
pub fn resolve_p4_env(config: &P4Config, file: Option<&UniversalPath>) -> P4Env {
    let mut env = P4Env::default();
    for (var, value) in [
        ("P4PORT", &config.port),
        ("P4USER", &config.user),
        ("P4PASSWORD", &config.password),
        ("P4CLIENT", &config.client),
        ("P4CHARSET", &config.charset),
    ] {
        if value == "#" {
            env.unset.push(var);
        } else if !value.is_empty() {
            env.set.push((var, value.clone()));
        }
    }
    env.cwd = match file {
        // A relative path would name another file once the cwd moves (it is also the argument).
        Some(UniversalPath::Local(path)) if path.is_absolute() => {
            path.parent().map(|dir| dir.to_path_buf())
        }
        Some(_) | None => None,
    };
    env
}

fn depot_path_without_revision(file: &UniversalPath) -> Result<&str, String> {
    match file {
        UniversalPath::Depot(path, _rev) => Ok(path),
        UniversalPath::Local(path) => Err(format!("{} is not a depot path", path.display())),
    }
}

/// What a failed command reports: stderr, else stdout, else the exit status.
fn failure_text(stderr: &[u8], stdout: &[u8], status: impl std::fmt::Display) -> String {
    [stderr, stdout]
        .iter()
        .map(|text| String::from_utf8_lossy(text).trim().to_string())
        .find(|text| !text.is_empty())
        .unwrap_or_else(|| format!("p4 failed ({status})"))
}

pub struct P4Command {
    exe_path: String,
    _is_gui: bool,
    file: Option<UniversalPath>,
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
            file: None,
        }
    }

    /// The file the command is about, so it runs with that file's environment.
    pub fn for_file(mut self, file: UniversalPath) -> Self {
        self.file = Some(file);
        self
    }

    fn build_cmd(&self, config: &P4Config) -> Command {
        let mut cmd = Command::new(&self.exe_path);
        let env = resolve_p4_env(config, self.file.as_ref());
        for (var, value) in env.set {
            cmd.env(var, value);
        }
        for var in env.unset {
            cmd.env_remove(var);
        }
        if let Some(cwd) = env.cwd {
            cmd.current_dir(cwd);
        }
        cmd
    }

    fn prepare_cmd(&self) -> Command {
        self.build_cmd(&get_p4_config())
    }

    pub fn output(&self, args: &[&str]) -> Result<String, String> {
        let output = self
            .prepare_cmd()
            .args(args)
            .output()
            .map_err(|e| format!("{}: {e}", self.exe_path))?;

        if output.status.success() {
            // p4 exits 0 on some warnings ("no such file(s)", "not in client view").
            let warnings = String::from_utf8_lossy(&output.stderr);
            if !warnings.trim().is_empty() {
                log::warn!("p4 {}: {}", args.join(" "), warnings.trim());
            }
            String::from_utf8(output.stdout).map_err(|e| e.to_string())
        } else {
            Err(failure_text(&output.stderr, &output.stdout, output.status))
        }
    }

    pub fn spawn(&self, args: &[&str]) -> Result<Child, String> {
        self.prepare_cmd()
            .args(args)
            .spawn()
            .map_err(|e| format!("{}: {e}", self.exe_path))
    }

    pub fn get_depot_file_content(file: &UniversalPath) -> Result<String, String> {
        P4Command::new(false)
            .for_file(file.clone())
            .output(&["print", "-q", &file.to_p4_string()])
    }
    pub fn open_revision_graph(file: &UniversalPath) -> Result<(), String> {
        let path = depot_path_without_revision(file)?;
        P4Command::new(true)
            .for_file(file.clone())
            .spawn(&["revisiongraph", path])?;
        Ok(())
    }
    pub fn open_timelapse_view(file: &UniversalPath) -> Result<(), String> {
        let path = depot_path_without_revision(file)?;
        P4Command::new(true)
            .for_file(file.clone())
            .spawn(&["timelapse", path])?;
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

/// A p4 runner for tests: answers through a closure and records every call.
#[cfg(test)]
pub(crate) struct FakeP4<F: Fn(&[&str]) -> Result<String, String>> {
    respond: F,
    calls: std::cell::RefCell<Vec<Vec<String>>>,
}

#[cfg(test)]
impl<F: Fn(&[&str]) -> Result<String, String>> FakeP4<F> {
    pub(crate) fn new(respond: F) -> Self {
        Self {
            respond,
            calls: Default::default(),
        }
    }

    pub(crate) fn calls(&self) -> Vec<Vec<String>> {
        self.calls.borrow().clone()
    }
}

#[cfg(test)]
impl<F: Fn(&[&str]) -> Result<String, String>> P4Runner for FakeP4<F> {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        self.calls
            .borrow_mut()
            .push(args.iter().map(|a| a.to_string()).collect());
        (self.respond)(args)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    fn config(port: &str, user: &str, client: &str, charset: &str, password: &str) -> P4Config {
        P4Config {
            port: port.into(),
            user: user.into(),
            client: client.into(),
            charset: charset.into(),
            password: password.into(),
        }
    }

    #[test]
    fn non_empty_fields_are_set() {
        let env = resolve_p4_env(&config("ssl:p4:1666", "me", "ws", "utf8", "pw"), None);
        assert_eq!(
            env.set,
            vec![
                ("P4PORT", "ssl:p4:1666".to_string()),
                ("P4USER", "me".to_string()),
                ("P4PASSWORD", "pw".to_string()),
                ("P4CLIENT", "ws".to_string()),
                ("P4CHARSET", "utf8".to_string()),
            ]
        );
        assert!(env.unset.is_empty());
    }

    #[test]
    fn empty_fields_are_left_to_the_environment() {
        assert_eq!(resolve_p4_env(&P4Config::default(), None), P4Env::default());

        let env = resolve_p4_env(&config("", "me", "", "", ""), None);
        assert_eq!(env.set, vec![("P4USER", "me".to_string())]);
        assert!(env.unset.is_empty());
    }

    #[test]
    fn hash_unsets_the_variable() {
        let env = resolve_p4_env(&config("#", "me", "#", "", "#"), None);
        assert_eq!(env.set, vec![("P4USER", "me".to_string())]);
        assert_eq!(env.unset, vec!["P4PORT", "P4PASSWORD", "P4CLIENT"]);
    }

    #[test]
    fn a_local_file_runs_in_its_directory() {
        let dir = env::temp_dir().join("repo").join("src");
        let file = UniversalPath::Local(dir.join("a.txt"));
        let env = resolve_p4_env(&P4Config::default(), Some(&file));
        assert_eq!(env.cwd, Some(dir));
    }

    #[test]
    fn a_depot_path_a_relative_path_or_no_file_keep_the_cwd() {
        let depot = UniversalPath::Depot("//depot/main/a.txt".into(), Some(3));
        let bare = UniversalPath::Local(PathBuf::from("a.txt"));
        let relative = UniversalPath::Local(PathBuf::from("sub").join("a.txt"));
        for file in [Some(&depot), Some(&bare), Some(&relative), None] {
            assert_eq!(
                resolve_p4_env(&P4Config::default(), file).cwd,
                None,
                "{file:?}"
            );
        }
    }

    #[test]
    fn the_command_sets_removes_and_runs_in_the_resolved_directory() {
        let dir = env::temp_dir().join("ws");
        let cmd = P4Command::new(false)
            .for_file(UniversalPath::Local(dir.join("a.txt")))
            .build_cmd(&config("ssl:p4:1666", "#", "", "", ""));
        let envs: Vec<_> = cmd.get_envs().collect();
        assert_eq!(
            envs,
            vec![
                (OsStr::new("P4PORT"), Some(OsStr::new("ssl:p4:1666"))),
                (OsStr::new("P4USER"), None),
            ]
        );
        assert_eq!(cmd.get_current_dir(), Some(dir.as_path()));

        let cmd = P4Command::new(false).build_cmd(&P4Config::default());
        assert_eq!(cmd.get_envs().count(), 0);
        assert_eq!(cmd.get_current_dir(), None);
    }

    #[test]
    fn failure_text_is_stderr_then_stdout_then_the_exit_status() {
        assert_eq!(failure_text(b"err\n", b"out\n", "exit code: 1"), "err");
        assert_eq!(failure_text(b"  ", b"out\n", "exit code: 1"), "out");
        assert_eq!(
            failure_text(b"", b"", "exit code: 1"),
            "p4 failed (exit code: 1)"
        );
    }

    #[test]
    fn local_path_escapes_p4_revision_and_wildcard_characters() {
        assert_eq!(
            escape_local_path(r"C:\ws\a@b #1 50%*.txt"),
            r"C:\ws\a%40b %231 50%25%2A.txt"
        );
        assert_eq!(escape_local_path(r"C:\ws\plain.txt"), r"C:\ws\plain.txt");
    }
}
