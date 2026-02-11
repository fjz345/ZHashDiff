use std::{collections::HashMap, path::PathBuf, sync::Arc};

use eframe::egui;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum FsEntry {
    File { path: PathBuf },
    Dir { path: PathBuf },
}

impl FsEntry {
    pub fn path(&self) -> &PathBuf {
        match self {
            FsEntry::File { path } => path,
            FsEntry::Dir { path } => path,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FsPath {
    pub root: FsEntry,
    pub entries: Vec<FsEntry>,
    // Hint only, if false, can skip looking for files in subfolders
    pub has_files_deep: bool,
}

impl Default for FsPath {
    fn default() -> Self {
        Self {
            root: FsEntry::Dir {
                path: PathBuf::new(),
            },
            entries: Vec::new(),
            has_files_deep: true,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FileSystem {
    pub root: PathBuf,
    // TODO: Cache
    // #[serde(skip)]
    // pub root_dir_cache: HashMap<PathBuf, Arc<PathCache>>,
    // pub cache_enabled: bool,
}

impl FileSystem {
    pub fn new() -> Self {
        Self {
            root: PathBuf::new(),
        }
    }

    fn read_path(path: &PathBuf) -> FsPath {
        let mut entries = vec![];
        if let Ok(read_dir) = std::fs::read_dir(path) {
            for entry in read_dir.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    entries.push(FsEntry::Dir { path: p });
                } else {
                    entries.push(FsEntry::File { path: p });
                }
            }
        }
        let root = if path.is_dir() {
            FsEntry::Dir { path: path.clone() }
        } else {
            FsEntry::File { path: path.clone() }
        };
        FsPath {
            root,
            entries,
            has_files_deep: Self::has_files_recursive(path),
        }
    }

    pub fn read_path_recursive_flatten(path: &PathBuf) -> FsPath {
        let mut current = Self::read_path(path);

        if current.has_files_deep {
            // Collect dirs first to avoid borrowing issues
            let dirs: Vec<PathBuf> = current
                .entries
                .iter()
                .filter_map(|entry| {
                    if let FsEntry::Dir { path } = entry {
                        Some(path.clone())
                    } else {
                        None
                    }
                })
                .collect();

            for dir_path in dirs {
                let child = Self::read_path_recursive_flatten(&dir_path);

                // Append all child entries
                current.entries.extend(child.entries);
            }
        }

        current
    }

    // TODO: can probably optimize to do this while reading the path
    pub fn has_files_recursive(path: &PathBuf) -> bool {
        if path.is_file() {
            return false;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if Self::has_files_recursive(&p) {
                    return true;
                }
            }
        }
        false
    }

    // TODO: memory optimize Arc<PathCache>
    pub fn get(&self, path: &PathBuf) -> FsPath {
        Self::read_path(path)
    }

    // TODO: Fix whenever to use .get()
    pub fn count_files(&self, path: &PathBuf) -> usize {
        let mut count = 0;
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_file() {
                    count += 1;
                } else {
                    count += self.count_files(&p);
                }
            }
        }
        count
    }

    // TODO: Fix whenever to use .get()
    pub fn count_folders(&self, path: &PathBuf) -> usize {
        let mut count = 0;
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    count += 1;
                    count += self.count_folders(&p);
                }
            }
        }
        count
    }
}
