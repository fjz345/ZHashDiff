use std::io;
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiffToolDefaultArgs {
    pub default_args: Vec<String>,
}

impl DiffToolDefaultArgs {
    pub fn to_string(&self) -> String {
        self.default_args.join("\n")
    }

    pub fn from_string(input: &str) -> Self {
        let default_args = input
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();

        Self { default_args }
    }
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiffToolConfig {
    pub exe_path: PathBuf,
    pub prefix_args: DiffToolDefaultArgs,
    pub diff_path_1_args: String,
    pub diff_path_2_args: String,
    pub suffix_args: DiffToolDefaultArgs,
}

impl Default for DiffToolConfig {
    fn default() -> Self {
        Self::default_zdiff()
    }
}

impl DiffToolConfig {
    pub fn default_zdiff() -> Self {
        Self {
            exe_path: PathBuf::from(r"zdiff-gui.exe"),
            prefix_args: DiffToolDefaultArgs::from_string(""),
            diff_path_1_args: String::from_str(r#"{}"#).unwrap(),
            diff_path_2_args: String::from_str(r#"{}"#).unwrap(),
            suffix_args: DiffToolDefaultArgs::from_string(""),
        }
    }
    pub fn default_tortoise() -> Self {
        Self {
            exe_path: PathBuf::from(r"C:\Program Files\TortoiseSVN\bin\TortoiseProc.exe"),
            prefix_args: DiffToolDefaultArgs::from_string("/command:diff"),
            diff_path_1_args: String::from_str(r#"/path:"{}""#).unwrap(),
            diff_path_2_args: String::from_str(r#"/path2:"{}""#).unwrap(),
            suffix_args: DiffToolDefaultArgs::from_string("/closeonend:1"),
        }
    }
}
#[cfg(target_os = "windows")]
pub fn open_diff_tool_windows(
    config: &DiffToolConfig,
    file1: impl AsRef<Path>,
    file2: impl AsRef<Path>,
) -> io::Result<()> {
    let mut cmd = Command::new(&config.exe_path);

    for arg in &config.prefix_args.default_args {
        cmd.arg(arg);
    }
    // !!! Important !!! use raw_arg.
    cmd.raw_arg(
        config
            .diff_path_1_args
            .replace("{}", &file1.as_ref().to_string_lossy()),
    );
    cmd.raw_arg(
        config
            .diff_path_2_args
            .replace("{}", &file2.as_ref().to_string_lossy()),
    );
    for arg in &config.suffix_args.default_args {
        cmd.arg(arg);
    }

    log::info!("Opening diff tool {:?}", cmd);
    cmd.spawn()?;

    Ok(())
}
#[cfg(target_os = "macos")]
pub fn open_diff_tool_macos(
    config: &DiffToolConfig,
    file1: impl AsRef<Path>,
    file2: impl AsRef<Path>,
) -> io::Result<()> {
    todo!();
    let mut cmd = Command::new(&config.exe_path);

    for arg in &config.prefix_args.default_args {
        cmd.arg(arg);
    }
    // !!! Important !!! use raw_arg.
    // cmd.raw_arg(
    //     config
    //         .diff_path_1_args
    //         .replace("{}", &file1.as_ref().to_string_lossy()),
    // );
    // cmd.raw_arg(
    //     config
    //         .diff_path_2_args
    //         .replace("{}", &file2.as_ref().to_string_lossy()),
    // );
    for arg in &config.suffix_args.default_args {
        cmd.arg(arg);
    }

    log::info!("Opening diff tool {:?}", cmd);
    cmd.spawn()?;

    Ok(())
}


pub fn open_diff_tool(
    config: &DiffToolConfig,
    file1: impl AsRef<Path>,
    file2: impl AsRef<Path>,
) -> io::Result<()> {
    #[cfg(target_os = "windows")]
    open_diff_tool_windows(config, file1, file2)?;

    #[cfg(target_os = "macos")]
    open_diff_tool_macos(config, file1, file2)?;

    Ok(())
}
