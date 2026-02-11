use std::{collections::HashMap, path::PathBuf, sync::Arc};

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

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct FileSystem {
    pub root: PathBuf,

    #[serde(skip)]
    pub expanded: HashMap<PathBuf, bool>,
    #[serde(skip)]
    pub selected: HashMap<PathBuf, bool>,

    pub cache_enabled: bool,
    #[serde(skip)]
    pub root_dir_cache: HashMap<PathBuf, Arc<DirCache>>,
}

impl FileSystem {}
