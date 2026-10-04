use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use zdiff::cached_file::CachedFile;
use zdiff::lexer::{LEXER_MODE_DEFAULT, RawToken};
use zdiff::universal_path::UniversalPath;

use crate::p4::P4Command;
use crate::viewer::{ViewerKind, sniff_viewer_kind};

/// A file whose content was sniffed as not text, kept as raw bytes for the Hex viewer.
pub struct BinaryFile {
    pub path: UniversalPath,
    pub bytes: Vec<u8>,
}

// Input logging prints the loaded files; the bytes can be hundreds of MB.
impl std::fmt::Debug for BinaryFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BinaryFile")
            .field("path", &self.path)
            .field("len", &self.bytes.len())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub enum LoadedFile {
    Text(Arc<CachedFile<RawToken>>),
    Binary(Arc<BinaryFile>),
}

impl LoadedFile {
    pub fn path(&self) -> &UniversalPath {
        match self {
            LoadedFile::Text(file) => &file.path,
            LoadedFile::Binary(file) => &file.path,
        }
    }

    /// The exact file bytes; text contents are read unmodified, so they are the file's bytes too.
    pub fn bytes(&self) -> &[u8] {
        match self {
            LoadedFile::Text(file) => file.contents.as_bytes(),
            LoadedFile::Binary(file) => &file.bytes,
        }
    }

    pub fn viewer_kind(&self) -> ViewerKind {
        match self {
            LoadedFile::Text(_) => ViewerKind::Text,
            LoadedFile::Binary(_) => ViewerKind::Hex,
        }
    }

    pub fn text(&self) -> Option<&Arc<CachedFile<RawToken>>> {
        match self {
            LoadedFile::Text(file) => Some(file),
            LoadedFile::Binary(_) => None,
        }
    }

    /// Same load, not just equal content: every load makes new `Arc`s.
    pub fn is_same_load(&self, other: &Self) -> bool {
        match (self, other) {
            (LoadedFile::Text(a), LoadedFile::Text(b)) => Arc::ptr_eq(a, b),
            (LoadedFile::Binary(a), LoadedFile::Binary(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

pub fn load_file(
    display_path: UniversalPath,
    physical_path: &Path,
    lexer_mode: u8,
) -> io::Result<LoadedFile> {
    let bytes = std::fs::read(physical_path)?;
    match sniff_viewer_kind(&bytes) {
        // Reads the file a second time, so text loading stays in CachedFile until the
        // text-encoding-eol decoder replaces it.
        ViewerKind::Text => CachedFile::new(display_path, physical_path, lexer_mode)
            .map(|file| LoadedFile::Text(Arc::new(file))),
        // Sniffing never picks Image; image content is binary, and the Image viewer reads bytes.
        ViewerKind::Hex | ViewerKind::Image => Ok(LoadedFile::Binary(Arc::new(BinaryFile {
            path: display_path,
            bytes,
        }))),
    }
}

/// Fetches a depot file as bytes and loads it through a temp copy under `temp_root`. The bytes
/// are never decoded here, so binary content reaches sniffing intact.
pub fn load_depot_file(
    display_path: UniversalPath,
    temp_root: &Path,
    lexer_mode: u8,
    fetch: impl FnOnce(&UniversalPath) -> Result<Vec<u8>, String>,
) -> Result<LoadedFile, String> {
    let UniversalPath::Depot(depot_str, rev) = &display_path else {
        return Err(format!("{display_path:?} is not a depot path"));
    };
    let mut temp_path = temp_root.join(depot_str.trim_start_matches('/'));
    if let Some(r) = rev {
        let mut filename = temp_path.file_name().unwrap_or_default().to_os_string();
        filename.push(format!("_rev{}", r));
        temp_path.set_file_name(filename);
    }

    if let Some(parent) = temp_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directories for {}: {}", depot_str, e))?;
    }

    let bytes = fetch(&display_path).map_err(|e| {
        format!(
            "P4 command failed for {}: {}",
            display_path.to_p4_string(),
            e
        )
    })?;
    std::fs::write(&temp_path, &bytes)
        .map_err(|e| format!("Failed to write P4 content to temp file: {}", e))?;

    let loaded = load_file(display_path.clone(), &temp_path, lexer_mode)
        .map_err(|e| format!("Cannot load file {}, Error: {e}", temp_path.display()));
    let _ = std::fs::remove_file(&temp_path);
    loaded
}

fn default_channel() -> (mpsc::Sender<UniversalPath>, mpsc::Receiver<UniversalPath>) {
    mpsc::channel()
}
fn default_channel_loaded_file() -> (
    mpsc::Sender<(UniversalPath, Option<LoadedFile>)>,
    mpsc::Receiver<(UniversalPath, Option<LoadedFile>)>,
) {
    mpsc::channel()
}

fn default_file_path() -> UniversalPath {
    UniversalPath::new("")
}

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileProcessor {
    #[cfg_attr(feature = "serde", serde(skip, default = "default_channel"))]
    channel: (mpsc::Sender<UniversalPath>, mpsc::Receiver<UniversalPath>),
    #[cfg_attr(feature = "serde", serde(skip, default = "default_file_path"))]
    file_path: UniversalPath,

    #[cfg_attr(feature = "serde", serde(skip))]
    root_path: Option<UniversalPath>,

    #[cfg_attr(feature = "serde", serde(skip))]
    loaded_file: Option<LoadedFile>,
    diff_lexer_mode: u8,

    #[cfg_attr(feature = "serde", serde(skip))]
    cached_file_path: Option<UniversalPath>, // only process once the path

    #[cfg_attr(
        feature = "serde",
        serde(skip, default = "default_channel_loaded_file")
    )]
    channel_loaded_file: (
        mpsc::Sender<(UniversalPath, Option<LoadedFile>)>,
        mpsc::Receiver<(UniversalPath, Option<LoadedFile>)>,
    ),
    #[cfg_attr(feature = "serde", serde(skip))]
    loading_path: Option<UniversalPath>,
}

impl Default for FileProcessor {
    fn default() -> Self {
        Self {
            channel: default_channel(),
            file_path: default_file_path(),
            loaded_file: None,
            diff_lexer_mode: LEXER_MODE_DEFAULT,
            cached_file_path: None,
            root_path: None,
            channel_loaded_file: default_channel_loaded_file(),
            loading_path: None,
        }
    }
}

#[allow(dead_code)]
impl FileProcessor {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get_tx(&self) -> mpsc::Sender<UniversalPath> {
        self.channel.0.clone()
    }
    pub fn get_rx(&self) -> &mpsc::Receiver<UniversalPath> {
        &self.channel.1
    }

    pub fn poll_path_channel(&mut self) {
        while let Ok(path) = self.channel.1.try_recv() {
            self.set_path(path);
        }
    }

    pub fn set_lexer_mode(&mut self, mode: u8) {
        if self.diff_lexer_mode != mode {
            log::debug!(
                "Setting lexer mode {:?} for path: {:?}",
                mode,
                self.file_path
            );
            self.diff_lexer_mode = mode;
            self.invalidate_cache_file();
        }
    }

    pub fn get_path(&mut self) -> UniversalPath {
        self.poll_path_channel();

        if let Some(root) = &self.root_path {
            if let Some(stripped_path) = Self::strip_root_prefix(root, &self.file_path) {
                return stripped_path;
            }
        }

        self.file_path.clone()
    }

    pub fn get_full_path(&mut self) -> UniversalPath {
        self.poll_path_channel();

        self.file_path.clone()
    }

    pub fn get_path_as_string(&self) -> String {
        self.file_path.to_p4_string()
    }

    pub fn set_path(&mut self, path: UniversalPath) {
        let old_path = self.file_path.clone();
        log::debug!("{:?}", self.root_path);

        if let Some(root) = &self.root_path {
            if Self::is_root_valid(&root, &path) {
                log::debug!("is_root_valid {:?}", true);
                log::debug!("is_root_depot {:?}", root.is_depot());
                let mut new_path = root.clone();
                new_path.append(path.clone());
                self.file_path = new_path
            } else {
                self.file_path = path;
            }
        } else {
            self.file_path = path;
        }

        if old_path != self.file_path {
            self.invalidate_cache_file();
        }
    }

    pub fn set_root(&mut self, root: UniversalPath) {
        self.root_path = Some(root);
    }
    pub fn get_root(&mut self) -> Option<UniversalPath> {
        self.root_path.clone()
    }

    pub fn strip_root_prefix(root: &UniversalPath, path: &UniversalPath) -> Option<UniversalPath> {
        match (root, path) {
            (UniversalPath::Local(root_local), UniversalPath::Local(path_local)) => {
                let norm_root = Self::normalize_path(root_local);
                let norm_path = Self::normalize_path(path_local);

                norm_path
                    .strip_prefix(&norm_root)
                    .ok()
                    .map(|p| UniversalPath::Local(p.to_path_buf()))
            }

            (UniversalPath::Depot(root_depot, _), UniversalPath::Depot(path_depot, rev)) => {
                let root = root_depot.trim_end_matches('/');

                if path_depot == root {
                    Some(UniversalPath::Depot(String::new(), *rev))
                } else {
                    path_depot
                        .strip_prefix(&(root.to_owned() + "/"))
                        .map(|s| UniversalPath::Depot(s.to_string(), *rev))
                }
            }

            _ => Some(path.clone()),
        }
    }

    pub fn is_root_valid(root: &UniversalPath, path: &UniversalPath) -> bool {
        match (root, path) {
            (UniversalPath::Local(root_local), UniversalPath::Local(path_local)) => {
                let norm_root = Self::normalize_path(root_local);
                let norm_path = Self::normalize_path(path_local);
                norm_path.starts_with(norm_root)
            }

            (UniversalPath::Depot(root_depot, _), UniversalPath::Depot(path_depot, _)) => {
                let root = root_depot.trim_end_matches('/');

                path_depot == root || path_depot.starts_with(&(root.to_owned() + "/"))
            }

            _ => true,
        }
    }

    fn normalize_path(path: &std::path::Path) -> PathBuf {
        let mut normalized = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    normalized.pop();
                }
                std::path::Component::CurDir => {}
                _ => normalized.push(component),
            }
        }
        normalized
    }

    pub fn invalidate_cache_file(&mut self) {
        log::debug!("Invalidating cache file for path: {:?}", self.file_path);
        self.loaded_file = None;
        self.cached_file_path = None;
    }

    pub fn get_loading_path(&self) -> Option<&UniversalPath> {
        self.loading_path.as_ref()
    }

    /// The loaded file only when it is text; a binary load gives `None`.
    pub fn get_cached_file(&mut self) -> Option<Arc<CachedFile<RawToken>>> {
        self.get_loaded_file().and_then(|file| file.text().cloned())
    }

    pub fn get_loaded_file(&mut self) -> Option<LoadedFile> {
        while let Ok((loaded_path, file_opt)) = self.channel_loaded_file.1.try_recv() {
            if self.cached_file_path.as_ref() == Some(&loaded_path) {
                self.loaded_file = file_opt;
                self.loading_path = None;
            }
        }

        let path = &self.get_full_path();

        if !path.is_empty() && self.cached_file_path.as_ref() != Some(path) {
            self.cached_file_path = Some(path.clone());
            self.loaded_file = None;
            self.loading_path = Some(path.clone());

            log::debug!(
                "Loading file asynchronously: {:?} with lexer mode {:?}",
                path,
                self.diff_lexer_mode
            );

            let tx = self.channel_loaded_file.0.clone();
            let path_clone = path.clone();
            let diff_lexer_mode = self.diff_lexer_mode;

            std::thread::spawn(move || {
                let loaded = match &path_clone {
                    UniversalPath::Local(p) => load_file(path_clone.clone(), p, diff_lexer_mode)
                        .map_err(|e| format!("Cannot load file {}, Error: {e}", p.display())),
                    UniversalPath::Depot(..) => load_depot_file(
                        path_clone.clone(),
                        &std::env::temp_dir(),
                        diff_lexer_mode,
                        P4Command::get_depot_file_bytes,
                    ),
                };
                let loaded_file_opt = loaded.inspect_err(|e| log::error!("{e}")).ok();

                let _ = tx.send((path_clone, loaded_file_opt));
            });
        }

        self.loaded_file.clone()
    }

    pub fn get_cached_file_hash(&mut self) -> Option<String> {
        self.get_cached_file()
            .as_ref()
            .and_then(|f| Some(f.hash.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewer::ViewerKind;
    use std::path::PathBuf;
    #[test]
    fn test_is_root_valid_local() {
        let root = UniversalPath::Local(PathBuf::from(r"E:\Github\ZHashDiff\crates"));

        assert!(FileProcessor::is_root_valid(
            &root,
            &UniversalPath::Local(PathBuf::from(
                r"E:\Github\ZHashDiff\crates\zdiff-gui\src\main.rs",
            ))
        ));

        assert!(!FileProcessor::is_root_valid(
            &root,
            &UniversalPath::Local(PathBuf::from(r"C:\Other\Project\src\main.rs"))
        ));

        assert!(!FileProcessor::is_root_valid(
            &root,
            &UniversalPath::Local(PathBuf::from(
                r"E:\Github\ZHashDiff\crates\zdiff-gui\..\..\test\rust_files_diff_1\advanced_rust.rs",
            ))
        ));
    }

    #[test]
    fn test_is_root_valid_depot() {
        let cases = [
            (
                UniversalPath::Depot("//depot/folder".into(), None),
                UniversalPath::Depot("//depot/folder/file.rs".into(), None),
                true,
            ),
            (
                UniversalPath::Depot("//depot/folder".into(), None),
                UniversalPath::Depot("//depot/folder/file.rs".into(), Some(5)),
                true,
            ),
            (
                UniversalPath::Depot("//depot/folder".into(), None),
                UniversalPath::Depot("//depot/other/file.rs".into(), None),
                false,
            ),
            (
                UniversalPath::Depot("//depot_2/one_deeper".into(), None),
                UniversalPath::Depot("//depot_2/one_deeper/test_folder/test_2.txt".into(), None),
                true,
            ),
            (
                UniversalPath::Depot("//depot_2/one_deeper/".into(), None),
                UniversalPath::Depot("//depot_2/one_deeper/test_folder/test_2.txt".into(), None),
                true,
            ),
            (
                UniversalPath::Depot("//depot_2/one_deeper".into(), None),
                UniversalPath::Depot("//depot_2/one_deeper".into(), None),
                true,
            ),
            (
                UniversalPath::Depot("//depot_2/one_deeper/".into(), None),
                UniversalPath::Depot("//depot_2/one_deeper/file.rs".into(), None),
                true,
            ),
            (
                UniversalPath::Depot("//depot/folder".into(), None),
                UniversalPath::Depot("//depot/folder2/file.rs".into(), None),
                false,
            ),
        ];

        for (root, path, expected) in cases {
            assert_eq!(
                FileProcessor::is_root_valid(&root, &path),
                expected,
                "root={root:?}, path={path:?}"
            );
        }
    }

    #[test]
    fn test_is_root_valid_mixed() {
        let local = UniversalPath::Local(PathBuf::from(r"E:\Github\ZHashDiff"));
        let depot = UniversalPath::Depot("//depot/folder/file.rs".into(), Some(2));

        assert!(FileProcessor::is_root_valid(&local, &depot));
        assert!(FileProcessor::is_root_valid(&depot, &local));
    }

    fn load_bytes(bytes: &[u8]) -> LoadedFile {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("file.bin");
        std::fs::write(&path, bytes).unwrap();
        load_file(UniversalPath::from(path.clone()), &path, LEXER_MODE_DEFAULT).unwrap()
    }

    #[test]
    fn valid_utf8_loads_as_text() {
        let loaded = load_bytes("fn main() {}\n// ünïcode\n".as_bytes());
        assert_eq!(loaded.viewer_kind(), ViewerKind::Text);
        let LoadedFile::Text(file) = loaded else {
            panic!("expected text");
        };
        assert_eq!(file.contents, "fn main() {}\n// ünïcode\n");
    }

    #[test]
    fn empty_file_loads_as_text() {
        assert_eq!(load_bytes(b"").viewer_kind(), ViewerKind::Text);
    }

    #[test]
    fn nul_containing_content_loads_as_bytes_for_hex() {
        // Valid UTF-8 apart from the NUL rule, so only the NUL makes it binary.
        let bytes = b"PK\x03\x04\x00\x00abc";
        let loaded = load_bytes(bytes);
        assert_eq!(loaded.viewer_kind(), ViewerKind::Hex);
        assert_eq!(loaded.bytes(), bytes);
    }

    fn load_depot(
        temp_root: &Path,
        depot: &str,
        rev: Option<u32>,
        fetched: Result<&[u8], &str>,
    ) -> Result<LoadedFile, String> {
        let path = UniversalPath::Depot(depot.into(), rev);
        load_depot_file(path, temp_root, LEXER_MODE_DEFAULT, |_| {
            fetched.map(<[u8]>::to_vec).map_err(str::to_string)
        })
    }

    #[test]
    fn non_utf8_depot_content_loads_as_bytes_for_hex() {
        let temp = tempfile::tempdir().unwrap();
        let bytes = b"\x89PNG\r\n\x1a\n\xff\xfe\x00caf\xe9";
        let loaded = load_depot(temp.path(), "//depot/art/logo.png", Some(3), Ok(bytes)).unwrap();
        assert_eq!(loaded.viewer_kind(), ViewerKind::Hex);
        assert_eq!(loaded.bytes(), bytes);
        assert_eq!(
            loaded.path(),
            &UniversalPath::Depot("//depot/art/logo.png".into(), Some(3))
        );
    }

    #[test]
    fn utf8_depot_content_still_loads_as_text() {
        let temp = tempfile::tempdir().unwrap();
        let text = "fn main() {}\n// ünïcode\n";
        let loaded = load_depot(
            temp.path(),
            "//depot/src/main.rs",
            None,
            Ok(text.as_bytes()),
        )
        .unwrap();
        assert_eq!(loaded.viewer_kind(), ViewerKind::Text);
        assert_eq!(loaded.text().unwrap().contents, text);
        assert_eq!(
            loaded.path(),
            &UniversalPath::Depot("//depot/src/main.rs".into(), None)
        );
    }

    #[test]
    fn depot_fetch_failure_loads_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let result = load_depot(temp.path(), "//depot/a.bin", None, Err("no such file(s)"));
        assert!(result.unwrap_err().contains("no such file(s)"));
    }

    #[test]
    fn depot_temp_copy_is_removed_after_loading() {
        let temp = tempfile::tempdir().unwrap();
        load_depot(temp.path(), "//depot/dir/a.bin", Some(2), Ok(b"\x00\x01")).unwrap();
        load_depot(temp.path(), "//depot/dir/b.txt", None, Ok(b"text")).unwrap();
        let left: Vec<_> = walk_files(temp.path());
        assert!(left.is_empty(), "{left:?}");
    }

    fn walk_files(dir: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                files.extend(walk_files(&path));
            } else {
                files.push(path);
            }
        }
        files
    }

    #[test]
    fn invalid_utf8_loads_as_bytes_for_hex() {
        let bytes = b"caf\xe9 au lait";
        let loaded = load_bytes(bytes);
        assert_eq!(loaded.viewer_kind(), ViewerKind::Hex);
        assert_eq!(loaded.bytes(), bytes);
    }
}
