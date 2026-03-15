#[cfg(test)]
mod tests {
    use crate::fs::{FileSystemModel, FsNodeKind};

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
        let model = FileSystemModel::new(dir.path()).expect("Failed to create FileSystemModel");

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

        let model = FileSystemModel::new(dir.path()).expect("Failed to create FileSystemModel");
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

        let model = FileSystemModel::new(root).expect("Failed to create FileSystemModel");

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

        let model = FileSystemModel::new(dir.path()).expect("Failed to create FileSystemModel");

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
