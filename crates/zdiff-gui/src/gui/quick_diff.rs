use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Response};
use tempfile::{NamedTempFile, TempPath};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UniversalPath {
    /// Represented as //stream/path/file.txt
    Depot(String),
    /// Represented as C:\User\File.txt or /home/user/file.txt
    Local(PathBuf),
}

impl UniversalPath {
    pub fn new<S: AsRef<OsStr>>(s: S) -> Self {
        let os_str = s.as_ref();
        let cow = os_str.to_string_lossy();

        if cow.starts_with("//") {
            Self::Depot(cow.into_owned())
        } else {
            Self::Local(PathBuf::from(os_str))
        }
    }

    pub fn as_local_path(&self) -> Option<&Path> {
        match self {
            Self::Local(p) => Some(p.as_path()),
            Self::Depot(_) => None,
        }
    }

    pub fn to_p4_string(&self) -> String {
        match self {
            Self::Depot(s) => s.clone(),
            Self::Local(p) => p.to_string_lossy().replace('\\', "/"),
        }
    }

    pub fn is_depot(&self) -> bool {
        matches!(self, Self::Depot(_))
    }
}

impl AsRef<OsStr> for UniversalPath {
    fn as_ref(&self) -> &OsStr {
        match self {
            Self::Depot(s) => OsStr::new(s),
            Self::Local(p) => p.as_os_str(),
        }
    }
}

#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UniversalPathConfig {
    pub p4_path: String,
}

pub fn ui_universal_path(
    ui: &mut egui::Ui,
    universal_path_config: &mut UniversalPathConfig,
) -> Response {
    ui.label("Universal Path Configuration");
    ui.text_edit_singleline(&mut universal_path_config.p4_path);
    ui.button("Save")
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

                // Here you would actually call 'p4 print' or similar
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
