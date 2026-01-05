use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Clone)]
pub enum FsEntry {
    File { path: PathBuf },
    Dir { path: PathBuf },
}

#[derive(Clone)]
pub struct DirCache {
    pub entries: Vec<FsEntry>,
}
