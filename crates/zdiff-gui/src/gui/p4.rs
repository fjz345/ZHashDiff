use std::path::PathBuf;
use std::process::{Child, Command};
use std::{env, str};

use eframe::egui;
use zdiff::universal_path::UniversalPath;

use std::sync::RwLock;

/// What every p4 command resolves its config from. Global because commands run on loader
/// threads too.
#[derive(Default)]
struct P4Settings {
    profiles: P4Profiles,
    /// The profile of the last Quick Diff; `None` is Auto.
    slot_profile: Option<u64>,
}

static GLOBAL_P4_SETTINGS: std::sync::OnceLock<RwLock<P4Settings>> = std::sync::OnceLock::new();
fn p4_settings() -> &'static RwLock<P4Settings> {
    GLOBAL_P4_SETTINGS.get_or_init(Default::default)
}
pub fn get_p4_config() -> P4Config {
    let settings = p4_settings().read().unwrap();
    // No config in the log: it holds the password.
    log::trace!(
        "Reading P4 config for slot profile {:?}",
        settings.slot_profile
    );
    settings.profiles.resolve(settings.slot_profile)
}
pub fn update_p4_profiles(profiles: P4Profiles) {
    p4_settings().write().unwrap().profiles = profiles;
}
/// Commands from now on use this slot profile over the default one.
pub fn set_p4_slot_profile(slot_profile: Option<u64>) {
    p4_settings().write().unwrap().slot_profile = slot_profile;
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

#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct P4Profile {
    /// Stable across renames; Quick Diff slots and the default refer to it.
    pub id: u64,
    pub name: String,
    pub config: P4Config,
}

#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct P4Profiles {
    pub profiles: Vec<P4Profile>,
    pub default: Option<u64>,
}

impl P4Profiles {
    pub fn get(&self, id: u64) -> Option<&P4Profile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    /// Adds an empty profile and returns its id, unused by any listed profile.
    pub fn add(&mut self, name: String) -> u64 {
        let id = self.profiles.iter().map(|p| p.id + 1).max().unwrap_or(0);
        self.profiles.push(P4Profile {
            id,
            name,
            config: P4Config::default(),
        });
        id
    }

    /// Slots still referring to it fall back to Auto.
    pub fn remove(&mut self, id: u64) {
        self.profiles.retain(|profile| profile.id != id);
        if self.default == Some(id) {
            self.default = None;
        }
    }

    /// The config a command runs with, field by field: the slot's profile, then the default
    /// profile, then (left empty) whatever p4 resolves from P4CONFIG or the environment. `None`
    /// is Auto, and so is a slot whose profile was removed.
    pub fn resolve(&self, slot: Option<u64>) -> P4Config {
        let empty = P4Config::default();
        let config_of = |id: Option<u64>| {
            id.and_then(|id| self.get(id))
                .map_or(&empty, |profile| &profile.config)
        };
        let (slot, default) = (config_of(slot), config_of(self.default));
        let pick = |field: fn(&P4Config) -> &String| {
            let value = field(slot);
            if value.is_empty() {
                field(default).clone()
            } else {
                value.clone()
            }
        };
        P4Config {
            port: pick(|c| &c.port),
            user: pick(|c| &c.user),
            client: pick(|c| &c.client),
            charset: pick(|c| &c.charset),
            password: pick(|c| &c.password),
        }
    }
}

pub fn ui_p4_profiles(ui: &mut egui::Ui, profiles: &mut P4Profiles) {
    ui.vertical(|ui| {
        ui.heading("Perforce Profiles");
        ui.label("A Quick Diff slot's profile goes over the default profile");
        ui.label("Empty fields come from the default profile, then P4CONFIG or the environment");
        ui.label("# to unset an env var for the command");

        ui.horizontal(|ui| {
            ui.label("Default profile");
            ui_p4_profile_combo(
                ui,
                "p4_default_profile",
                &profiles.profiles,
                "None",
                &mut profiles.default,
            );
        });
        if ui.button("Add Profile").clicked() {
            let name = format!("Profile {}", profiles.profiles.len() + 1);
            profiles.add(name);
        }

        let mut remove = None;
        egui::ScrollArea::vertical().show(ui, |ui| {
            for profile in &mut profiles.profiles {
                ui.separator();
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut profile.name)
                            .hint_text("name")
                            .desired_width(200.0),
                    );
                    if ui.button("Remove").clicked() {
                        remove = Some(profile.id);
                    }
                });

                let config = &mut profile.config;
                egui::Grid::new(("p4_profile_grid", profile.id))
                    .num_columns(2)
                    .spacing([40.0, 8.0])
                    .show(ui, |ui| {
                        let mut edit_row =
                            |label: &str, value: &mut String, hint: &str, password: bool| {
                                ui.label(label);
                                ui.add(
                                    egui::TextEdit::singleline(value)
                                        .hint_text(hint)
                                        .password(password)
                                        .desired_width(200.0),
                                );
                                ui.end_row();
                            };

                        edit_row("P4PORT", &mut config.port, "ssl:perforce:1666", false);
                        edit_row("P4USER", &mut config.user, "username", false);
                        edit_row("P4CLIENT", &mut config.client, "workspace_name", false);
                        edit_row("P4CHARSET", &mut config.charset, "utf8", false);
                        edit_row("P4PASSWD", &mut config.password, "password", true);
                    });
            }
        });
        if let Some(id) = remove {
            profiles.remove(id);
        }

        ui.add_space(10.0);
    });
}

/// Picks a profile, or `none_label` for none. A removed profile shows as none, which is how it
/// resolves.
pub fn ui_p4_profile_combo(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    profiles: &[P4Profile],
    none_label: &str,
    selected: &mut Option<u64>,
) {
    let selected_text = selected
        .and_then(|id| profiles.iter().find(|profile| profile.id == id))
        .map_or(none_label, |profile| profile.name.as_str());
    egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected_text)
        .show_ui(ui, |ui| {
            ui.selectable_value(selected, None, none_label);
            for profile in profiles {
                ui.selectable_value(selected, Some(profile.id), &profile.name);
            }
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
    pub args: Vec<String>,
    pub set: Vec<(&'static str, String)>,
    pub unset: Vec<&'static str>,
    pub cwd: Option<PathBuf>,
}

/// A non-empty field is passed as p4's global option, `#` unsets its variable, and an empty one
/// leaves the inherited value. Options, unlike variables, win over a P4CONFIG file. p4vc (`gui`)
/// gets variables instead: its global options aren't confirmed to match p4's. A local file's
/// directory becomes the working directory so p4 finds the P4CONFIG file of the repository that
/// file lives in; a depot path has no directory to pick one, and a relative path keeps the
/// app's cwd.
pub fn resolve_p4_env(config: &P4Config, file: Option<&UniversalPath>, gui: bool) -> P4Env {
    let mut env = P4Env::default();
    for (option, var, value) in [
        ("-p", "P4PORT", &config.port),
        ("-u", "P4USER", &config.user),
        ("-P", "P4PASSWD", &config.password),
        ("-c", "P4CLIENT", &config.client),
        ("-C", "P4CHARSET", &config.charset),
    ] {
        match value.as_str() {
            "" => {}
            "#" => env.unset.push(var),
            _ if gui => env.set.push((var, value.clone())),
            _ => env.args.extend([option.to_string(), value.clone()]),
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
    is_gui: bool,
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
            is_gui: is_gui,
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
        let env = resolve_p4_env(config, self.file.as_ref(), self.is_gui);
        // Global options must come before the command, which the caller appends.
        cmd.args(env.args);
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
        String::from_utf8(self.output_bytes(args)?).map_err(|e| e.to_string())
    }

    /// Like `output`, but stdout as raw bytes, for file content that may not be text.
    pub fn output_bytes(&self, args: &[&str]) -> Result<Vec<u8>, String> {
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
            Ok(output.stdout)
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

    pub fn get_depot_file_bytes(file: &UniversalPath) -> Result<Vec<u8>, String> {
        P4Command::new(false).for_file(file.clone()).output_bytes(&[
            "print",
            "-q",
            &file.to_p4_string(),
        ])
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

    fn two_profiles(slot: P4Config, default: P4Config) -> P4Profiles {
        P4Profiles {
            profiles: vec![
                P4Profile {
                    id: 1,
                    name: "slot".into(),
                    config: slot,
                },
                P4Profile {
                    id: 2,
                    name: "default".into(),
                    config: default,
                },
            ],
            default: Some(2),
        }
    }

    /// The global options and unset variables a p4 command runs with for a slot.
    fn command_shape(profiles: &P4Profiles, slot: Option<u64>) -> (Vec<String>, Vec<&str>) {
        let env = resolve_p4_env(&profiles.resolve(slot), None, false);
        assert!(env.set.is_empty(), "p4 gets flags, not variables");
        (env.args, env.unset)
    }

    fn args(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn slot_profile_over_default_profile_over_environment() {
        let profiles = two_profiles(
            config("slot:1666", "", "", "", ""),
            config("default:1666", "me", "", "", ""),
        );
        let (args_, unset) = command_shape(&profiles, Some(1));
        assert_eq!(args_, args(&["-p", "slot:1666", "-u", "me"]));
        assert!(unset.is_empty());
    }

    #[test]
    fn empty_slot_fields_fall_through_to_the_default_then_the_environment() {
        let profiles = two_profiles(config("", "", "", "", ""), config("", "me", "", "utf8", ""));
        let (args_, unset) = command_shape(&profiles, Some(1));
        assert_eq!(args_, args(&["-u", "me", "-C", "utf8"]));
        assert!(unset.is_empty());

        let empty = two_profiles(P4Config::default(), P4Config::default());
        assert_eq!(command_shape(&empty, Some(1)), (vec![], vec![]));
    }

    #[test]
    fn hash_in_the_slot_unsets_even_when_the_default_has_a_value() {
        let profiles = two_profiles(
            config("", "#", "#", "", ""),
            config("", "me", "ws", "", "pw"),
        );
        let (args_, unset) = command_shape(&profiles, Some(1));
        assert_eq!(args_, args(&["-P", "pw"]));
        assert_eq!(unset, vec!["P4USER", "P4CLIENT"]);
    }

    #[test]
    fn auto_uses_the_default_profile_and_the_environment_only() {
        let profiles = two_profiles(
            config("slot:1666", "slot_user", "", "", ""),
            config("", "me", "", "", ""),
        );
        let (args_, unset) = command_shape(&profiles, None);
        assert_eq!(args_, args(&["-u", "me"]));
        assert!(unset.is_empty());
    }

    #[test]
    fn a_removed_slot_profile_is_auto_and_a_removed_default_is_the_environment() {
        let mut profiles = two_profiles(
            config("slot:1666", "", "", "", ""),
            config("", "me", "", "", ""),
        );
        profiles.remove(1);
        assert_eq!(command_shape(&profiles, Some(1)).0, args(&["-u", "me"]));

        profiles.remove(2);
        assert_eq!(profiles.default, None);
        assert_eq!(command_shape(&profiles, Some(1)), (vec![], vec![]));
        assert_eq!(command_shape(&profiles, None), (vec![], vec![]));
    }

    #[test]
    fn a_dangling_default_is_the_environment() {
        let mut profiles = two_profiles(P4Config::default(), config("", "me", "", "", ""));
        profiles.default = Some(7);
        assert_eq!(command_shape(&profiles, None), (vec![], vec![]));
    }

    #[test]
    fn added_profiles_get_fresh_ids() {
        let mut profiles = P4Profiles::default();
        let a = profiles.add("a".into());
        let b = profiles.add("b".into());
        assert_ne!(a, b);
        assert_eq!(profiles.get(b).unwrap().name, "b");
        assert_eq!(profiles.get(b).unwrap().config, P4Config::default());

        profiles.remove(a);
        assert!(profiles.get(a).is_none());
        let c = profiles.add("c".into());
        assert_ne!(c, b);
        assert_eq!(profiles.profiles.len(), 2);
    }

    #[test]
    fn p4vc_gets_variables_instead_of_flags() {
        let env = resolve_p4_env(&config("ssl:p4:1666", "#", "", "", "pw"), None, true);
        assert!(env.args.is_empty());
        assert_eq!(
            env.set,
            vec![
                ("P4PORT", "ssl:p4:1666".to_string()),
                ("P4PASSWD", "pw".to_string()),
            ]
        );
        assert_eq!(env.unset, vec!["P4USER"]);
    }

    #[test]
    fn non_empty_fields_are_passed_as_global_options() {
        let env = resolve_p4_env(
            &config("ssl:p4:1666", "me", "ws", "utf8", "pw"),
            None,
            false,
        );
        assert_eq!(
            env.args,
            args(&[
                "-p",
                "ssl:p4:1666",
                "-u",
                "me",
                "-P",
                "pw",
                "-c",
                "ws",
                "-C",
                "utf8"
            ])
        );
        assert!(env.set.is_empty());
        assert!(env.unset.is_empty());
    }

    #[test]
    fn empty_fields_are_left_to_the_environment() {
        assert_eq!(
            resolve_p4_env(&P4Config::default(), None, false),
            P4Env::default()
        );
        assert_eq!(
            resolve_p4_env(&P4Config::default(), None, true),
            P4Env::default()
        );

        let env = resolve_p4_env(&config("", "me", "", "", ""), None, false);
        assert_eq!(env.args, args(&["-u", "me"]));
        assert!(env.unset.is_empty());
    }

    #[test]
    fn hash_unsets_the_variable() {
        let env = resolve_p4_env(&config("#", "me", "#", "", "#"), None, false);
        assert_eq!(env.args, args(&["-u", "me"]));
        assert_eq!(env.unset, vec!["P4PORT", "P4PASSWD", "P4CLIENT"]);
    }

    #[test]
    fn a_local_file_runs_in_its_directory() {
        let dir = env::temp_dir().join("repo").join("src");
        let file = UniversalPath::Local(dir.join("a.txt"));
        let env = resolve_p4_env(&P4Config::default(), Some(&file), false);
        assert_eq!(env.cwd, Some(dir));
    }

    #[test]
    fn a_depot_path_a_relative_path_or_no_file_keep_the_cwd() {
        let depot = UniversalPath::Depot("//depot/main/a.txt".into(), Some(3));
        let bare = UniversalPath::Local(PathBuf::from("a.txt"));
        let relative = UniversalPath::Local(PathBuf::from("sub").join("a.txt"));
        for file in [Some(&depot), Some(&bare), Some(&relative), None] {
            assert_eq!(
                resolve_p4_env(&P4Config::default(), file, false).cwd,
                None,
                "{file:?}"
            );
        }
    }

    #[test]
    fn the_command_passes_options_before_the_command_removes_and_runs_in_the_directory() {
        let dir = env::temp_dir().join("ws");
        let mut cmd = P4Command::new(false)
            .for_file(UniversalPath::Local(dir.join("a.txt")))
            .build_cmd(&config("ssl:p4:1666", "#", "", "", ""));
        cmd.args(["print", "-q", "a.txt"]);
        let cmd_args: Vec<_> = cmd.get_args().collect();
        assert_eq!(cmd_args, ["-p", "ssl:p4:1666", "print", "-q", "a.txt"]);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert_eq!(envs, vec![(OsStr::new("P4USER"), None)]);
        assert_eq!(cmd.get_current_dir(), Some(dir.as_path()));

        let cmd = P4Command::new(false).build_cmd(&P4Config::default());
        assert_eq!(cmd.get_args().count(), 0);
        assert_eq!(cmd.get_envs().count(), 0);
        assert_eq!(cmd.get_current_dir(), None);
    }

    #[test]
    fn p4vc_commands_set_variables() {
        let cmd = P4Command::new(true).build_cmd(&config("ssl:p4:1666", "#", "", "", ""));
        assert_eq!(cmd.get_args().count(), 0);
        let envs: Vec<_> = cmd.get_envs().collect();
        assert_eq!(
            envs,
            vec![
                (OsStr::new("P4PORT"), Some(OsStr::new("ssl:p4:1666"))),
                (OsStr::new("P4USER"), None),
            ]
        );
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
