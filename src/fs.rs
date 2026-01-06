use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
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

#[derive(Serialize, Deserialize, Clone)]
pub struct DirCache {
    pub entries: Vec<FsEntry>,
    pub has_files_deep: bool,
}

impl Default for DirCache {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            // Cache assumes has files until checked
            has_files_deep: true,
        }
    }
}
