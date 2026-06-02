use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use zdiff::cached_file::CachedFile;
use zdiff::lexer::{LEXER_MODE_DEFAULT, RawToken};
use zdiff::universal_path::UniversalPath;

use crate::p4::P4Command;

fn default_channel() -> (mpsc::Sender<UniversalPath>, mpsc::Receiver<UniversalPath>) {
    mpsc::channel()
}
fn default_channel_cached_file() -> (
    mpsc::Sender<(UniversalPath, Option<Arc<CachedFile<RawToken>>>)>,
    mpsc::Receiver<(UniversalPath, Option<Arc<CachedFile<RawToken>>>)>,
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
    cached_file: Option<Arc<CachedFile<RawToken>>>,
    diff_lexer_mode: u8,

    #[cfg_attr(feature = "serde", serde(skip))]
    cached_file_path: Option<UniversalPath>, // only process once the path

    #[cfg_attr(
        feature = "serde",
        serde(skip, default = "default_channel_cached_file")
    )]
    channel_cached_file: (
        mpsc::Sender<(UniversalPath, Option<Arc<CachedFile<RawToken>>>)>,
        mpsc::Receiver<(UniversalPath, Option<Arc<CachedFile<RawToken>>>)>,
    ),
    #[cfg_attr(feature = "serde", serde(skip))]
    loading_path: Option<UniversalPath>,
}

impl Default for FileProcessor {
    fn default() -> Self {
        Self {
            channel: default_channel(),
            file_path: default_file_path(),
            cached_file: None,
            diff_lexer_mode: LEXER_MODE_DEFAULT,
            cached_file_path: None,
            root_path: None,
            channel_cached_file: default_channel_cached_file(),
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
        self.cached_file = None;
        self.cached_file_path = None;
    }

    pub fn get_loading_path(&self) -> Option<&UniversalPath> {
        self.loading_path.as_ref()
    }

    pub fn get_cached_file(&mut self) -> Option<Arc<CachedFile<RawToken>>> {
        while let Ok((loaded_path, file_opt)) = self.channel_cached_file.1.try_recv() {
            if self.cached_file_path.as_ref() == Some(&loaded_path) {
                self.cached_file = file_opt;
                self.loading_path = None;
            }
        }

        let path = &self.get_full_path();

        if !path.is_empty() && self.cached_file_path.as_ref() != Some(path) {
            self.cached_file_path = Some(path.clone());
            self.cached_file = None;
            self.loading_path = Some(path.clone());

            log::debug!(
                "Constructing CachedFile asynchronously: {:?} with lexer mode {:?}",
                path,
                self.diff_lexer_mode
            );

            let tx = self.channel_cached_file.0.clone();
            let path_clone = path.clone();
            let diff_lexer_mode = self.diff_lexer_mode;

            std::thread::spawn(move || {
                let target_path = match &path_clone {
                    UniversalPath::Local(p) => p.clone(),
                    UniversalPath::Depot(depot_str, rev) => {
                        let sanitized = depot_str.trim_start_matches('/');
                        let mut temp_path = std::env::temp_dir().join(sanitized);

                        if let Some(r) = rev {
                            let mut filename =
                                temp_path.file_name().unwrap_or_default().to_os_string();
                            filename.push(format!("_rev{}", r));
                            temp_path.set_file_name(filename);
                        }

                        if let Some(parent) = temp_path.parent() {
                            if let Err(e) = std::fs::create_dir_all(parent) {
                                log::error!(
                                    "Failed to create directories for {}: {}",
                                    depot_str,
                                    e
                                );
                                let _ = tx.send((path_clone, None));
                                return;
                            }
                        }

                        let p4_path = path_clone.to_p4_string();
                        let content = match P4Command::get_depot_file_content(&p4_path, None) {
                            Ok(c) => c,
                            Err(e) => {
                                log::error!("P4 command failed for {}: {}", p4_path, e);
                                let _ = tx.send((path_clone, None));
                                return;
                            }
                        };

                        if let Err(e) = std::fs::write(&temp_path, content.as_bytes()) {
                            log::error!("Failed to write P4 content to temp file: {}", e);
                            let _ = tx.send((path_clone, None));
                            return;
                        }

                        temp_path
                    }
                };

                let cached_file_opt =
                    match CachedFile::new(path_clone.clone(), &target_path, diff_lexer_mode) {
                        Ok(r) => Some(Arc::new(r)),
                        Err(e) => {
                            log::error!("Cannot find file {}, Error: {e}", target_path.display());
                            None
                        }
                    };

                if path_clone.is_depot() {
                    let _ = std::fs::remove_file(&target_path);
                }

                let _ = tx.send((path_clone, cached_file_opt));
            });
        }

        self.cached_file.clone()
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
}
