use std::io;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// allow for custom args if using non-supported external diff tool
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct DiffToolDefaultArgs {
    pub default_args: Vec<String>,
}

impl DiffToolDefaultArgs {
    /// Convert to a single string (newline separated)
    pub fn to_string(&self) -> String {
        self.default_args.join("\n")
    }

    /// Parse from a newline-separated string
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

#[derive(Clone, Debug, Serialize, Deserialize)]
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

// #[derive(Debug, Clone)]
// enum KnownDiffTools {
//     Tortoise,
//     Unknown,
// }

// impl KnownDiffTools {
//     fn match_config(config: &DiffToolConfig) -> Self {
//         const TORTOISE_KNOWN_NAMES: [&str; 2] = ["tortoiseproc.exe", "tortoisemerge.exe"];

//         let exe_name = config
//             .exe_path
//             .file_name()
//             .and_then(|n| n.to_str())
//             .map(|s| s.to_ascii_lowercase());

//         if let Some(name) = exe_name {
//             if TORTOISE_KNOWN_NAMES.contains(&name.as_str()) {
//                 return KnownDiffTools::Tortoise;
//             }
//         }

//         KnownDiffTools::Unknown
//     }
// }

impl DiffToolConfig {
    /// Returns a default config for TortoiseSVN if installed at standard location
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

pub fn open_diff_tool(
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
