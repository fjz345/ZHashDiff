use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UniversalPath {
    /// Represented as //stream/path/file.txt
    Depot(String),
    /// Represented as C:\User\File.txt or /home/user/file.txt
    Local(PathBuf),
}

impl Default for UniversalPath {
    fn default() -> Self {
        UniversalPath::Local(PathBuf::new())
    }
}

impl std::fmt::Display for UniversalPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UniversalPath::Local(p) => write!(f, "{}", p.display()),
            UniversalPath::Depot(s) => write!(f, "{}", s),
        }
    }
}

impl From<&String> for UniversalPath {
    fn from(s: &String) -> Self {
        UniversalPath::new(s)
    }
}
impl From<PathBuf> for UniversalPath {
    fn from(path: PathBuf) -> Self {
        UniversalPath::Local(path)
    }
}

impl From<&std::path::Path> for UniversalPath {
    fn from(path: &std::path::Path) -> Self {
        UniversalPath::Local(path.to_path_buf())
    }
}

impl From<String> for UniversalPath {
    fn from(s: String) -> Self {
        UniversalPath::new(s)
    }
}

impl From<&str> for UniversalPath {
    fn from(s: &str) -> Self {
        UniversalPath::new(s)
    }
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

    pub fn is_empty(&self) -> bool {
        self.to_p4_string().is_empty()
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
