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
        UniversalPath::new(path.as_os_str())
    }
}

impl From<&Path> for UniversalPath {
    fn from(path: &Path) -> Self {
        UniversalPath::new(path.as_os_str())
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

    pub fn append(&mut self, other: UniversalPath) {
        match (self, other) {
            (Self::Local(base), Self::Local(addition)) => {
                base.push(addition);
                *base = Self::normalize_local_path(base);
            }
            (Self::Local(base), Self::Depot(addition, _)) => {
                let clean_addition = addition.trim_start_matches('/');
                base.push(clean_addition);
                *base = Self::normalize_local_path(base);
            }
            (Self::Depot(base, _), Self::Depot(addition, _)) => {
                let clean_base = base.trim_end_matches('/');
                let clean_addition = addition.trim_start_matches('/');
                *base = format!("{}/{}", clean_base, clean_addition);
            }
            (Self::Depot(base, _), Self::Local(addition)) => {
                let clean_base = base.trim_end_matches('/');
                let add_str = addition.to_string_lossy().replace('\\', "/");
                let clean_addition = add_str.trim_start_matches('/');
                *base = format!("{}/{}", clean_base, clean_addition);
            }
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

    pub fn as_path(&self) -> Option<&Path> {
        match self {
            Self::Local(p) => Some(p.as_path()),
            Self::Depot(..) => None, // TODO: get local path for depot
        }
    }

    pub fn as_local_path(&self) -> String {
        match self {
            Self::Local(p) => p.to_string_lossy().replace('\\', "/"),
            Self::Depot(..) => todo!(), // TODO: get local path for depot
        }
    }

    pub fn to_p4_string(&self) -> String {
        match self {
            Self::Depot(s, Some(rev)) => format!("{}#{}", s, rev),
            Self::Depot(s, None) => s.clone(),
            Self::Local(p) => self.as_local_path(),
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

    /// Whether this is a temporary file: a local path inside `temp_root` (in practice the OS
    /// temp dir, where p4 puts the copies it hands an external diff tool). Depot paths never
    /// are, and an empty root contains nothing.
    ///
    /// Lexical, so it is cheap enough to ask every frame. Separators, case and a `\\?\` prefix
    /// don't matter; an 8.3 short name and its long name don't match (canonicalize both first
    /// where that matters).
    pub fn is_temp(&self, temp_root: &Path) -> bool {
        let Self::Local(path) = self else {
            return false;
        };
        let root = lexical_components(temp_root);
        if root == (false, Vec::new()) {
            return false;
        }
        let path = lexical_components(path);
        path.0 == root.0 && path.1.starts_with(&root.1)
    }
}

/// Whether the path starts at a root separator, and its lowercased components, with either
/// separator and the `\\?\` and `\\?\UNC\` verbatim prefixes taken as their plain forms.
fn lexical_components(path: &Path) -> (bool, Vec<String>) {
    let s = path.to_string_lossy().replace('\\', "/");
    let s = if let Some(unc) = s.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else {
        s.strip_prefix("//?/").map(str::to_owned).unwrap_or(s)
    };
    let components = s
        .split('/')
        .filter(|c| !c.is_empty() && *c != ".")
        .map(str::to_lowercase)
        .collect();
    (s.starts_with('/'), components)
}

impl AsRef<OsStr> for UniversalPath {
    fn as_ref(&self) -> &OsStr {
        match self {
            Self::Depot(s, _) => OsStr::new(s),
            Self::Local(p) => p.as_os_str(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parsing() {
        let depot = UniversalPath::new("//stream/main/file.txt");
        assert!(matches!(depot, UniversalPath::Depot(..)));
        assert_eq!(depot.to_p4_string(), "//stream/main/file.txt");

        let local = UniversalPath::new(r"C:\User\File.txt");
        assert!(matches!(local, UniversalPath::Local(..)));
        assert_eq!(local.to_p4_string(), "C:/User/File.txt");
    }

    #[test]
    fn test_append_local_to_local() {
        let mut base = UniversalPath::new(r"C:\User\Project");
        let addition = UniversalPath::new(r"src\main.rs");
        base.append(addition);
        assert_eq!(
            base,
            UniversalPath::Local(PathBuf::from(r"C:\User\Project\src\main.rs"))
        );
    }

    #[test]
    fn test_append_depot_to_local() {
        let mut base = UniversalPath::new(r"C:\User\Project");
        let addition = UniversalPath::new("//depot/src/main.rs");
        base.append(addition);
        assert_eq!(
            base,
            UniversalPath::Local(PathBuf::from(r"C:\User\Project\depot\src\main.rs"))
        );
    }

    #[test]
    fn test_append_depot_to_depot() {
        let mut base = UniversalPath::new("//stream/main/");
        let addition = UniversalPath::new("//folder/file.txt#3");
        base.append(addition);
        assert_eq!(
            base,
            UniversalPath::Depot(String::from("//stream/main/folder/file.txt"), None)
        );
    }

    const TEMP: &str = r"C:\Users\me\AppData\Local\Temp";

    fn is_temp(path: &str, root: &str) -> bool {
        UniversalPath::new(path).is_temp(Path::new(root))
    }

    #[test]
    fn a_path_inside_the_temp_root_is_temp() {
        assert!(is_temp(
            r"C:\Users\me\AppData\Local\Temp\p4v\file#3.rs",
            TEMP
        ));
        assert!(is_temp(r"C:\Users\me\AppData\Local\Temp\a.txt", TEMP));
        assert!(is_temp(TEMP, TEMP));
    }

    #[test]
    fn a_path_outside_the_temp_root_is_not_temp() {
        assert!(!is_temp(r"C:\Users\me\work\src\main.rs", TEMP));
        assert!(!is_temp(r"D:\Users\me\AppData\Local\Temp\a.txt", TEMP));
        assert!(!is_temp(r"C:\Users\me\AppData\Local", TEMP));
        // Same leading characters, another directory.
        assert!(!is_temp(r"C:\Users\me\AppData\Local\Temp2\a.txt", TEMP));
        assert!(!is_temp(r"C:\Users\me\AppData\Local\Tempfile.txt", TEMP));
        // Relative paths are not under an absolute root, even with matching names.
        assert!(!is_temp(r"Users\me\AppData\Local\Temp\a.txt", TEMP));
        assert!(!is_temp("tmp/a.txt", "/tmp"));
    }

    #[test]
    fn case_and_separators_do_not_matter() {
        assert!(is_temp(r"c:\users\ME\appdata\local\TEMP\a.txt", TEMP));
        assert!(is_temp("C:/Users/me/AppData/Local/Temp/p4v/a.txt", TEMP));
        assert!(is_temp(
            r"C:\Users\me\AppData\Local\Temp\a.txt",
            "C:/users/me/appdata/local/temp"
        ));
        // GetTempPath returns the root with a trailing separator.
        assert!(is_temp(
            r"C:\Users\me\AppData\Local\Temp\a.txt",
            r"C:\Users\me\AppData\Local\Temp\"
        ));
        assert!(is_temp(r"C:\Users\me\AppData\Local\Temp\\p4v\\a.txt", TEMP));
        assert!(is_temp("/tmp/p4/a.txt", "/tmp/"));
    }

    #[test]
    fn a_verbatim_prefix_does_not_matter() {
        // canonicalize returns \\?\ paths; a path it couldn't resolve keeps its plain form.
        assert!(is_temp(
            r"C:\Users\me\AppData\Local\Temp\a.txt",
            r"\\?\C:\Users\me\AppData\Local\Temp"
        ));
        assert!(is_temp(r"\\?\C:\Users\me\AppData\Local\Temp\a.txt", TEMP));
        assert!(is_temp(
            r"\\?\UNC\server\share\tmp\a.txt",
            r"\\server\share\tmp"
        ));
        assert!(!is_temp(
            r"\\?\UNC\server\share\a.txt",
            r"\\server\share\tmp"
        ));
    }

    #[test]
    fn depot_paths_are_never_temp() {
        assert!(!is_temp("//depot/main/file.rs#3", TEMP));
        assert!(!is_temp("//depot/main/file.rs", "//depot/main"));
        assert!(!is_temp("//depot/main/file.rs", "/"));
    }

    #[test]
    fn an_empty_root_contains_nothing() {
        assert!(!is_temp(r"C:\Users\me\AppData\Local\Temp\a.txt", ""));
        assert!(!is_temp("a.txt", ""));
        assert!(!is_temp("", ""));
    }

    #[test]
    fn test_append_local_to_depot() {
        let mut base = UniversalPath::new("//stream/main");
        let addition = UniversalPath::new(r"folder\file.txt");
        base.append(addition);
        assert_eq!(
            base,
            UniversalPath::Depot(String::from("//stream/main/folder/file.txt"), None)
        );
    }
}
