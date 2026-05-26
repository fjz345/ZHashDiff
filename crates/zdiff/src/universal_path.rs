use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UniversalPath {
    /// Represented as //stream/path/file.txt
    Depot(String, Option<u32>),
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
            UniversalPath::Depot(s, Some(rev)) => write!(f, "{}#{}", s, rev),
            UniversalPath::Depot(s, None) => write!(f, "{}", s),
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
            if let Some(hash_idx) = cow.rfind('#') {
                if let Ok(rev) = cow[hash_idx + 1..].parse::<u32>() {
                    return Self::Depot(cow[..hash_idx].to_string(), Some(rev));
                }
            }
            Self::Depot(cow.into_owned(), None)
        } else {
            Self::Local(Self::normalize_local_path(Path::new(os_str)))
        }
    }

    fn normalize_local_path(path: &Path) -> PathBuf {
        let mut components = Vec::new();

        for comp in path.components() {
            match comp {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    if let Some(std::path::Component::Normal(_)) = components.last() {
                        components.pop();
                    } else {
                        components.push(comp);
                    }
                }
                _ => components.push(comp),
            }
        }

        components.into_iter().collect()
    }

    pub fn as_local_path(&self) -> Option<&Path> {
        match self {
            Self::Local(p) => Some(p.as_path()),
            Self::Depot(..) => None,
        }
    }

    pub fn to_p4_string(&self) -> String {
        match self {
            Self::Depot(s, Some(rev)) => format!("{}#{}", s, rev),
            Self::Depot(s, None) => s.clone(),
            Self::Local(p) => p.to_string_lossy().replace('\\', "/"),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Self::Depot(s, _) => s.is_empty(),
            Self::Local(p) => p.as_os_str().is_empty(),
        }
    }

    pub fn is_depot(&self) -> bool {
        matches!(self, Self::Depot(..))
    }

    pub fn revision(&self) -> Option<u32> {
        match self {
            Self::Depot(_, rev) => *rev,
            Self::Local(_) => None,
        }
    }

    pub fn set_revision(&mut self, new_rev: Option<u32>) {
        if let Self::Depot(_, rev) = self {
            *rev = new_rev;
        }
    }
}

impl AsRef<OsStr> for UniversalPath {
    fn as_ref(&self) -> &OsStr {
        match self {
            Self::Depot(s, _) => OsStr::new(s),
            Self::Local(p) => p.as_os_str(),
        }
    }
}
