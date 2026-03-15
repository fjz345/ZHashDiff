use std::{
    io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

pub type FsNodeId = usize;
pub type FsNodeDepth = u16;
pub type FsIsDir = bool;

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

    pub fn as_path(&self) -> impl AsRef<Path> {
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
    pub fn new(root_path: impl AsRef<Path>) -> io::Result<Self> {
        Self::build_model(root_path)
    }

    pub fn get_node(&self, node_id: FsNodeId) -> Option<&FsNode> {
        self.nodes.get(node_id)
    }
    fn get_node_mut(&mut self, node_id: FsNodeId) -> Option<&mut FsNode> {
        self.nodes.get_mut(node_id)
    }
    pub fn get_parent_id(&self, node_id: FsNodeId) -> Option<FsNodeId> {
        self.get_node(node_id)?.parent
    }
    pub fn get_parent(&self, node_id: FsNodeId) -> Option<&FsNode> {
        let parent_id = self.get_parent_id(node_id)?;
        self.get_node(parent_id)
    }
    fn get_parent_mut(&mut self, node_id: FsNodeId) -> Option<&mut FsNode> {
        let parent_id = self.get_parent_id(node_id)?;
        self.get_node_mut(parent_id)
    }
    pub fn get_root(&self) -> &FsNode {
        &self.nodes[self.root_id]
    }
    #[allow(dead_code)]
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
                if node.as_path().as_ref() == path.as_ref() {
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

    #[allow(dead_code)]
    pub fn assert_tree_consistency(model: &FileSystemModel) {
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
