use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub type FsNodeId = usize;
pub type FsNodeDepth = u16;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
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

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct FsNode {
    pub parent: Option<FsNodeId>,
    pub kind: FsNodeKind,
}

impl FsNode {
    pub fn is_dir(&self) -> bool {
        match self.kind {
            FsNodeKind::File { .. } => false,
            FsNodeKind::Dir { .. } => true,
        }
    }
    pub fn is_file(&self) -> bool {
        match self.kind {
            FsNodeKind::File { .. } => true,
            FsNodeKind::Dir { .. } => false,
        }
    }

    pub fn children(&self) -> Option<&Vec<FsNodeId>> {
        match &self.kind {
            FsNodeKind::File { .. } => None,
            FsNodeKind::Dir { children, .. } => Some(children),
        }
    }

    pub fn pathbuf(&self) -> impl AsRef<Path> {
        match &self.kind {
            FsNodeKind::File { path } | FsNodeKind::Dir { path, .. } => path,
        }
    }

    pub fn display_name(&self) -> &str {
        match &self.kind {
            FsNodeKind::File { path } | FsNodeKind::Dir { path, .. } => path
                .file_name()
                .expect("failed to get file_name")
                .to_str()
                .expect("failed to get str"),
        }
    }
}

pub struct TreeIter<'a> {
    model: &'a FileSystemModel,
    stack: Vec<(FsNodeId, FsNodeDepth)>,
}

impl<'a> TreeIter<'a> {
    fn new(model: &'a FileSystemModel, root: FsNodeId) -> Self {
        Self {
            model,
            stack: vec![(root, 0)],
        }
    }
}

impl<'a> Iterator for TreeIter<'a> {
    type Item = (FsNodeId, &'a FsNode, FsNodeDepth);

    fn next(&mut self) -> Option<Self::Item> {
        let (node_id, depth) = self.stack.pop()?;
        let node = self.model.get_node(node_id)?;

        if let FsNodeKind::Dir { children, .. } = &node.kind {
            if let Some(next_depth) = depth.checked_add(1) {
                for &child_id in children.iter().rev() {
                    self.stack.push((child_id, next_depth));
                }
            } else {
                log::warn!(
                    "Maximum tree depth (65535) reached. Pruning subtree at {:?}",
                    node_id
                );
            }
        }

        Some((node_id, node, depth))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FileSystemModel {
    root_path: PathBuf,
    root_id: FsNodeId,
    nodes: Vec<FsNode>,
}

impl FileSystemModel {
    pub fn new(root_path: impl AsRef<Path>) -> Self {
        Self::build_model(root_path).expect("failed to build model")
    }

    pub fn get_node(&self, node_id: FsNodeId) -> Option<&FsNode> {
        self.nodes.get(node_id)
    }
    fn get_node_mut(&mut self, node_id: FsNodeId) -> Option<&mut FsNode> {
        self.nodes.get_mut(node_id)
    }
    pub fn get_root(&self) -> &FsNode {
        &self.nodes[self.root_id]
    }
    fn get_root_mut(&mut self) -> &mut FsNode {
        &mut self.nodes[self.root_id]
    }
    pub fn get_root_node_id(&self) -> FsNodeId {
        assert_eq!(self.root_id, 0);
        self.root_id
    }

    pub fn total_files_and_folders(&self) -> usize {
        self.nodes.len()
    }

    pub fn iter_files(&self) -> impl Iterator<Item = FsNodeId> + '_ {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| matches!(n.kind, FsNodeKind::File { .. }))
            .map(|(id, _)| id)
    }

    pub fn iter_nodes(&self) -> impl Iterator<Item = FsNodeId> + '_ {
        self.nodes.iter().enumerate().map(|(id, _)| id)
    }

    pub fn iter_tree(&self) -> TreeIter<'_> {
        TreeIter::new(self, 0)
    }

    pub fn iter_subtree(&self, root: FsNodeId) -> TreeIter<'_> {
        TreeIter::new(self, root)
    }

    pub fn get_node_id(&self, node: &FsNode) -> FsNodeId {
        let start = self.nodes.as_ptr() as usize;
        let current = node as *const FsNode as usize;
        (current - start) / std::mem::size_of::<FsNode>()
    }

    // Slow, avoid
    pub fn find_path(&self, path: impl AsRef<Path>) -> Option<FsNodeId> {
        for node_id in self.iter_nodes() {
            if let Some(node) = self.get_node(node_id) {
                if node.pathbuf().as_ref() == path.as_ref() {
                    return Some(node_id);
                }
            }
        }
        None
    }

    pub fn total_files(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| match n.kind {
                FsNodeKind::File { .. } => true,
                FsNodeKind::Dir { .. } => false,
            })
            .count()
    }

    pub fn total_folders(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| match n.kind {
                FsNodeKind::File { .. } => false,
                FsNodeKind::Dir { .. } => true,
            })
            .count()
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
        let mut model = Self {
            root_path,
            nodes,
            root_id: 0,
        };

        model.root_id = model.read_dir(root.as_ref().to_path_buf(), None)?;
        assert_eq!(model.root_id, 0);

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

    fn assert_tree_consistency(model: &FileSystemModel) {
        for (id, node) in model.nodes.iter().enumerate() {
            if let Some(parent_id) = node.parent {
                let parent = model.get_node(parent_id).unwrap();
                match &parent.kind {
                    FsNodeKind::Dir { children, .. } => {
                        assert!(children.contains(&id));
                    }
                    _ => panic!("parent is not a directory"),
                }
            }
        }
    }
}

//// TESTS
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::path::Path;
    use tempfile::{TempDir, tempdir};

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

    /// Builds a large deterministic directory tree:
    /// - 10 top-level dirs, each with 5 subdirs and 5 files
    /// - Each subdir has 5 files
    /// - Deep chain 10 levels, 1 file per level
    ///
    /// Totals (including root):
    /// - Directories: 72
    /// - Files: 310
    /// - Nodes: 382
    ///
    /// Returns (TempDir, FileSystemModel)
    fn build_large_test_tree() -> (TempDir, FileSystemModel) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();

        // 10 top-level directories
        for i in 0..10 {
            let top = root.join(format!("dir_{i}"));
            fs::create_dir(&top).unwrap();

            // 5 files at top-level dir
            for f in 0..5 {
                create_file(&top.join(format!("file_{i}_{f}.txt")));
            }

            // 5 subdirectories
            for j in 0..5 {
                let sub = top.join(format!("sub_{i}_{j}"));
                fs::create_dir(&sub).unwrap();

                // 5 files per subdirectory
                for k in 0..5 {
                    create_file(&sub.join(format!("file_{i}_{j}_{k}.txt")));
                }
            }
        }

        // Deep nested chain (10 levels)
        let mut current = root.join("deep_chain");
        fs::create_dir(&current).unwrap();

        for depth in 0..10 {
            create_file(&current.join(format!("deep_file_{depth}.txt")));
            let next = current.join(format!("level_{depth}"));
            fs::create_dir(&next).unwrap();
            current = next;
        }

        let model = FileSystemModel::new(root);

        (temp, model)
    }

    #[test]
    fn test_large_tree_consistency() {
        let (_temp_dir, file_system_model) = build_large_test_tree();
        FileSystemModel::assert_tree_consistency(&file_system_model);
    }

    #[test]
    fn test_count_files_and_folders() {
        let (_temp_dir, file_system_model) = build_large_test_tree();
        let large_tree_num_files = 310;
        let large_tree_num_folders = 72;
        assert_eq!(file_system_model.total_files(), large_tree_num_files);
        assert_eq!(file_system_model.total_folders(), large_tree_num_folders);
        assert_eq!(
            file_system_model.total_files_and_folders(),
            large_tree_num_files + large_tree_num_folders
        );
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
