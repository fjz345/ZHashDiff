use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone)]
pub enum FsEntry {
    File { path: PathBuf },
    Dir { path: PathBuf },
}

#[derive(Serialize, Deserialize, Clone)]
pub struct DirCache {
    pub entries: Vec<FsEntry>,
}
