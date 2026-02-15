use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Debug)]
pub struct DiffToolConfig {
    pub exe_path: PathBuf,
    pub default_args: Vec<String>,
}

impl DiffToolConfig {
    /// Returns a default config for TortoiseSVN if installed at standard location
    pub fn default_tortoise() -> Self {
        Self {
            exe_path: PathBuf::from(r"C:\Program Files\TortoiseSVN\bin\TortoiseProc.exe"),
            default_args: vec![],
        }
    }
}

pub fn open_diff_tool(config: &DiffToolConfig, file1: &Path, file2: &Path) -> io::Result<()> {
    let mut cmd = Command::new(&config.exe_path);

    for arg in &config.default_args {
        cmd.arg(arg);
    }
    cmd.arg("/command:diff");
    // !!! Important !!! use raw_arg.
    cmd.raw_arg(format!(r#"/path:"{}""#, file1.to_string_lossy()));
    cmd.raw_arg(format!(r#"/path2:"{}""#, file2.to_string_lossy()));
    cmd.arg("/closeonend:1");

    log::info!("Opening diff tool {:?}", cmd);
    cmd.spawn()?;

    Ok(())
}
