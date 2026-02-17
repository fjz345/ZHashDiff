use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum FsEntry {
    File { path: PathBuf },
    Dir { path: PathBuf },
}

impl FsEntry {
    pub fn relative_path_buf(&self, root: &PathBuf) -> PathBuf {
        match self {
            FsEntry::File { path } => path.strip_prefix(root).unwrap_or(path).to_path_buf(),
            FsEntry::Dir { path } => path.strip_prefix(root).unwrap_or(path).to_path_buf(),
        }
    }
    pub fn path(&self) -> &Path {
        match self {
            FsEntry::File { path } => path.as_path(),
            FsEntry::Dir { path } => path.as_path(),
        }
    }
    pub fn path_buf(&self) -> &PathBuf {
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

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FsPathFlat {
    pub root: FsEntry,
    // Entry with depth from root
    pub entries: Vec<(FsEntry, usize)>,
}

impl Default for FsPathFlat {
    fn default() -> Self {
        Self {
            root: FsEntry::Dir {
                path: PathBuf::new(),
            },
            entries: Vec::new(),
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
    pub fn read_path_recursive_flatten(path: &PathBuf) -> FsPathFlat {
        let root_entry = FsEntry::Dir { path: path.clone() };

        let mut flat = FsPathFlat {
            root: root_entry,
            entries: Vec::new(),
        };

        Self::read_path_recursive_flatten_inner(path, 0, &mut flat);

        flat
    }

    fn read_path_recursive_flatten_inner(path: &PathBuf, depth: usize, flat: &mut FsPathFlat) {
        let current = Self::read_path(path);

        for entry in current.entries {
            let entry_depth = depth + 1;

            // Store entry with depth
            flat.entries.push((entry.clone(), entry_depth));

            // Recurse if directory
            if let FsEntry::Dir { path } = &entry {
                Self::read_path_recursive_flatten_inner(path, entry_depth, flat);
            }
        }
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

// ^^^^^^^^^^^^^^^^^ OLD ^^^^^^^^^^^^^^^^

type FsNodeId = usize;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum FsNodeKind {
    File {
        path: PathBuf,
    },
    Dir {
        path: PathBuf,
        children: Vec<FsNodeId>,
    },
}

impl FsNodeKind {
    pub fn new_file(path: impl AsRef<Path>) -> Self {
        Self::File {
            path: path.as_ref().into(),
        }
    }
    pub fn new_empty_dir(path: impl AsRef<Path>) -> Self {
        Self::Dir {
            path: path.as_ref().into(),
            children: Vec::new(),
        }
    }
    pub fn new_dir(path: impl AsRef<Path>, children: Vec<FsNodeId>) -> Self {
        Self::Dir {
            path: path.as_ref().into(),
            children,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FsNode {
    pub parent: Option<FsNodeId>,
    pub kind: FsNodeKind,
}

#[derive(Debug)]
pub struct FileSystemModel {
    root_path: PathBuf,
    nodes: Vec<FsNode>,
}

impl FileSystemModel {
    pub fn new(root_path: &Path) -> Self {
        Self::build_model(root_path).expect("failed to build model")
    }

    pub fn get_node(&self, node_id: FsNodeId) -> Option<&FsNode> {
        self.nodes.get(node_id)
    }
    pub fn get_node_mut(&mut self, node_id: FsNodeId) -> Option<&mut FsNode> {
        self.nodes.get_mut(node_id)
    }

    fn push_node(&mut self, parent: Option<FsNodeId>, kind: FsNodeKind) -> FsNodeId {
        let new_node = FsNode { parent, kind };
        let next_node_id: FsNodeId = self.nodes.len();
        self.nodes.push(new_node);
        next_node_id
    }

    fn build_model(root: impl AsRef<Path>) -> io::Result<Self> {
        let root_path = root.as_ref().to_path_buf();
        let nodes = Vec::new();
        let mut model = Self { root_path, nodes };

        let root_node_id = model.read_dir(root.as_ref().to_path_buf(), None)?;
        assert_eq!(root_node_id, 0);

        Ok(model)
    }

    fn read_dir(
        &mut self,
        path: impl AsRef<Path>,
        parent: Option<FsNodeId>,
    ) -> io::Result<FsNodeId> {
        let read_dir = std::fs::read_dir(&path)?;
        let dir_node_id = self.push_node(parent, FsNodeKind::new_empty_dir(&path));

        let mut children = Vec::new();
        for entry in read_dir.flatten() {
            let p = entry.path();
            let child_id = if p.is_dir() {
                self.read_dir(p, Some(dir_node_id))?
            } else {
                self.push_node(Some(dir_node_id), FsNodeKind::new_file(p))
            };
            children.push(child_id);
        }

        if let Some(node) = self.get_node_mut(dir_node_id) {
            if let FsNodeKind::Dir { children: c, .. } = &mut node.kind {
                *c = children;
            }
        }
        Ok(dir_node_id)
    }
}

//// TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::path::Path;
    use tempfile::tempdir;

    fn create_file(path: &Path) {
        File::create(path).expect("failed to create file");
    }

    #[test]
    fn builds_empty_directory() {
        let dir = tempdir().unwrap();
        let model = FileSystemModel::new(dir.path());

        let root = model.get_node(0).unwrap();

        match &root.kind {
            FsNodeKind::Dir { path, children } => {
                assert_eq!(path, dir.path());
                assert!(children.is_empty());
            }
            _ => panic!("root is not a directory"),
        }

        assert!(root.parent.is_none());
    }

    #[test]
    fn builds_directory_with_files() {
        let dir = tempdir().unwrap();

        create_file(&dir.path().join("a.txt"));
        create_file(&dir.path().join("b.txt"));

        let model = FileSystemModel::new(dir.path());
        let root = model.get_node(0).unwrap();

        let children_ids = match &root.kind {
            FsNodeKind::Dir { children, .. } => children,
            _ => panic!("root not dir"),
        };

        assert_eq!(children_ids.len(), 2);

        let mut file_names = Vec::new();

        for child_id in children_ids {
            let node = model.get_node(*child_id).unwrap();
            assert_eq!(node.parent, Some(0));

            match &node.kind {
                FsNodeKind::File { path } => {
                    file_names.push(path.file_name().unwrap().to_string_lossy().to_string());
                }
                _ => panic!("expected file"),
            }
        }

        file_names.sort();
        assert_eq!(file_names, vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn builds_nested_directories() {
        let dir = tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();
        create_file(&sub.join("nested.txt"));

        let model = FileSystemModel::new(dir.path());

        let root = model.get_node(0).unwrap();

        let root_children = match &root.kind {
            FsNodeKind::Dir { children, .. } => children,
            _ => panic!("root not dir"),
        };

        assert_eq!(root_children.len(), 1);

        let subdir_id = root_children[0];
        let subdir_node = model.get_node(subdir_id).unwrap();
        assert_eq!(subdir_node.parent, Some(0));

        let sub_children = match &subdir_node.kind {
            FsNodeKind::Dir { children, .. } => children,
            _ => panic!("subdir not dir"),
        };

        assert_eq!(sub_children.len(), 1);

        let nested_id = sub_children[0];
        let nested_node = model.get_node(nested_id).unwrap();
        assert_eq!(nested_node.parent, Some(subdir_id));

        match &nested_node.kind {
            FsNodeKind::File { path } => {
                assert_eq!(path.file_name().unwrap(), "nested.txt");
            }
            _ => panic!("expected file"),
        }
    }
}
