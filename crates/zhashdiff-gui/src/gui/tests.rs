#[cfg(test)]
mod tests {
    use crate::ui_egui::fs_tree::{DiffState, FileSystemView};

    use super::*;
    use std::collections::HashMap;
    use std::fs::{self, File};
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;
    use zhashdiff::fs::{FileSystemModel, FsIsDir, FsNodeDepth, FsNodeId};

    struct CollapsedTestCase {
        name: &'static str,
        structure: Vec<(&'static str, FsIsDir)>,
        collapsed: Vec<&'static str>,
        expected: Vec<(&'static str, FsNodeDepth)>,
    }

    fn get_rel(file_system: &FileSystemModel, root: &Path, id: FsNodeId) -> String {
        let node = file_system.get_node(id).expect("Node ID not found");
        if id == file_system.get_root_node_id() {
            return String::new();
        }

        node.as_path()
            .as_ref()
            .strip_prefix(root)
            .unwrap_or(node.as_path().as_ref())
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
            .trim_start_matches('/')
            .to_string()
    }

    fn get_rel_from_diff_state(
        file_system_1: &FileSystemModel,
        file_system_2: &FileSystemModel,
        r1: &Path,
        r2: &Path,
        state: &DiffState,
    ) -> String {
        match state {
            DiffState::OnlyInFirst(id) => get_rel(file_system_1, r1, *id),
            DiffState::OnlyInSecond(id) => get_rel(file_system_2, r2, *id),
            DiffState::Same(id, _) | DiffState::Different(id, _) | DiffState::Partial(id, _) => {
                get_rel(file_system_1, r1, *id)
            }
        }
    }

    fn format_tree(rows: &[(String, FsNodeDepth)]) -> String {
        rows.iter()
            .map(|(path, depth)| {
                let indent = "  ".repeat(*depth as usize);
                let name = if path.is_empty() { "/" } else { path };
                format!("{}└─ {}", indent, name)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn format_diff_tree_two(
        left: &[(String, FsNodeDepth)],
        right: &[(String, FsNodeDepth)],
        diff: &[(String, FsNodeDepth, String)],
    ) -> String {
        let mut output = String::new();
        output.push_str(&format!(
            "{:<30} | {:<30} | {:<30}\n",
            "LEFT TREE", "DIFF RESULT", "RIGHT TREE"
        ));
        output.push_str(&"-".repeat(96));
        output.push('\n');

        let l_map: HashMap<&str, FsNodeDepth> =
            left.iter().map(|(p, d)| (p.as_str(), *d)).collect();
        let r_map: HashMap<&str, FsNodeDepth> =
            right.iter().map(|(p, d)| (p.as_str(), *d)).collect();

        for (path, depth, marker) in diff {
            let l_row = l_map
                .get(path.as_str())
                .map(|d| {
                    format!(
                        "{}└─ {}",
                        "  ".repeat(*d as usize),
                        if path.is_empty() { "/" } else { path }
                    )
                })
                .unwrap_or_default();

            let r_row = r_map
                .get(path.as_str())
                .map(|d| {
                    format!(
                        "{}└─ {}",
                        "  ".repeat(*d as usize),
                        if path.is_empty() { "/" } else { path }
                    )
                })
                .unwrap_or_default();

            let d_row = format!(
                "{} {}└─ {}",
                marker,
                "  ".repeat(*depth as usize),
                if path.is_empty() { "/" } else { path }
            );

            output.push_str(&format!("{:<30} | {:<30} | {:<30}\n", l_row, d_row, r_row));
        }
        output
    }

    #[test]
    fn test_build_collapsed_rows_scenarios_single_view() {
        let cases = vec![
            CollapsedTestCase {
                name: "Simple collapsed directory",
                structure: vec![("a", true), ("a/file.txt", false)],
                collapsed: vec!["a"],
                expected: vec![("", 0), ("a", 1)],
            },
            CollapsedTestCase {
                name: "Fully expanded directory",
                structure: vec![
                    ("dir_a", true),
                    ("dir_a/file_1.txt", false),
                    ("dir_b", true),
                ],
                collapsed: vec![],
                expected: vec![("", 0), ("dir_a", 1), ("dir_a/file_1.txt", 2), ("dir_b", 1)],
            },
            CollapsedTestCase {
                name: "Deep nesting with partial collapse",
                structure: vec![
                    ("level1", true),
                    ("level1/level2", true),
                    ("level1/level2/derp.txt", false),
                    ("level1/level2/level3", true),
                    ("level1/level2/level3/file.txt", false),
                ],
                collapsed: vec!["level1/level2"],
                expected: vec![("", 0), ("level1", 1), ("level1/level2", 2)],
            },
        ];

        for case in cases {
            let temp = tempdir().unwrap();
            let root_path = temp.path();

            for (rel_path, is_dir) in &case.structure {
                let full_path = root_path.join(rel_path);
                if *is_dir {
                    fs::create_dir_all(&full_path).unwrap();
                } else {
                    if let Some(parent) = full_path.parent() {
                        fs::create_dir_all(parent).unwrap();
                    }
                    File::create(&full_path).unwrap();
                }
            }

            let model = FileSystemModel::new(root_path).expect("failed to create FileSystemModel");
            let mut view = FileSystemView {
                file_system: Arc::new(model),
                collapsed: HashMap::new(),
                selected: HashMap::new(),
            };

            for path_str in &case.collapsed {
                let id = find_id_by_rel_path(&view.file_system, root_path, path_str);
                view.collapsed.insert(id, true);
            }

            let root_id = view.file_system.get_root_node_id();
            let result = view.build_collapsed_rows(root_id, 0);

            let actual: Vec<(String, FsNodeDepth)> = result
                .into_iter()
                .map(|row| {
                    let node = view.file_system.get_node(row.path).unwrap();
                    let rel = node
                        .as_path()
                        .as_ref()
                        .strip_prefix(root_path)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/");

                    (rel, row.depth)
                })
                .collect();

            let expected_mapped: Vec<(String, FsNodeDepth)> = case
                .expected
                .iter()
                .map(|(p, d)| (p.to_string(), *d))
                .collect();

            if actual != expected_mapped {
                panic!(
                    "\nTest Case Failed: {}\n\nEXPECTED TREE:\n{}\n\nACTUAL TREE:\n{}\n",
                    case.name,
                    format_tree(&expected_mapped),
                    format_tree(&actual)
                );
            }
        }
    }

    type DiffMarker = &'static str;
    struct DiffTestCase {
        name: &'static str,
        left_structure: Vec<(&'static str, FsIsDir)>, // path, is_dir
        right_structure: Vec<(&'static str, FsIsDir)>, // path, is_dir
        left_collapsed: Vec<&'static str>,
        right_collapsed: Vec<&'static str>,
        // (Path, Depth, Diff Marker: "+" for Left, "-" for Right, "~" for Modified, "=" for Same)
        expected: Vec<(&'static str, FsNodeDepth, DiffMarker)>,
    }

    // Old test, now build_rows does not build with collapsed state, this is handled in the ui code.
    // #[test]
    // fn test_build_two_folder_diff_scenarios() {
    //     let cases = vec![
    //         DiffTestCase {
    //             name: "Standard file addition",
    //             left_structure: vec![("a.txt", false)],
    //             right_structure: vec![("a.txt", false), ("b.txt", false)],
    //             left_collapsed: vec![],
    //             right_collapsed: vec![],
    //             expected: vec![("", 0, "~"), ("a.txt", 1, "="), ("b.txt", 1, "+")],
    //         },
    //         DiffTestCase {
    //             name: "Collapsed directory hides children",
    //             left_structure: vec![("dir/file.txt", false)],
    //             right_structure: vec![
    //                 ("dir/one_extra_deep/file.txt", false),
    //                 ("dir/a_file.txt", false),
    //             ],
    //             left_collapsed: vec!["dir"],
    //             right_collapsed: vec!["dir"],
    //             expected: vec![("", 0, "~"), ("dir", 1, "~")],
    //         },
    //         DiffTestCase {
    //             name: "One side collapsed directory should NOT hide children",
    //             left_structure: vec![("dir/hidden_file.txt", false)],
    //             right_structure: vec![
    //                 ("dir/one_extra_deep", true),
    //                 ("dir/one_extra_deep/file.txt", false),
    //                 ("dir/visible_file.txt", false),
    //             ],
    //             left_collapsed: vec!["dir"],
    //             right_collapsed: vec![],
    //             expected: vec![
    //                 ("", 0, "~"),
    //                 ("dir", 1, "~"),
    //                 ("dir/hidden_file.txt", 2, "-"),
    //                 ("dir/one_extra_deep", 2, "+"),
    //                 ("dir/one_extra_deep/file.txt", 3, "+"),
    //                 ("dir/visible_file.txt", 2, "+"),
    //             ],
    //         },
    //         DiffTestCase {
    //             name: "Complex nested diff with partial collapse",
    //             left_structure: vec![
    //                 ("common/deleted.txt", false),
    //                 ("common/same.txt", false),
    //                 ("nested/level1/level2/file.txt", false),
    //                 ("only_left/a.txt", false),
    //             ],
    //             right_structure: vec![
    //                 ("common/added.txt", false),
    //                 ("common/same.txt", false),
    //                 ("nested/level1/level2/file.txt", false),
    //                 ("only_right/b.txt", false),
    //             ],
    //             left_collapsed: vec!["nested/level1"],
    //             right_collapsed: vec!["nested/level1"],
    //             expected: vec![
    //                 ("", 0, "~"),
    //                 ("common", 1, "~"),
    //                 ("common/added.txt", 2, "+"),
    //                 ("common/deleted.txt", 2, "-"),
    //                 ("common/same.txt", 2, "="),
    //                 ("nested", 1, "="),
    //                 ("nested/level1", 2, "="),
    //                 ("only_left", 1, "-"),
    //                 ("only_left/a.txt", 2, "-"),
    //                 ("only_right", 1, "+"),
    //                 ("only_right/b.txt", 2, "+"),
    //             ],
    //         },
    //     ];

    //     for case in cases {
    //         let temp_l = tempdir().unwrap();
    //         let temp_r = tempdir().unwrap();

    //         setup_fs(temp_l.path(), &case.left_structure);
    //         setup_fs(temp_r.path(), &case.right_structure);

    //         let view_l = create_view(temp_l.path(), &case.left_collapsed);
    //         let view_r = create_view(temp_r.path(), &case.right_collapsed);

    //         let left_tree_actual = view_l
    //             .build_collapsed_rows(view_l.file_system.get_root_node_id(), 0)
    //             .into_iter()
    //             .map(|row| {
    //                 (
    //                     get_rel(&view_l.file_system, temp_l.path(), row.path),
    //                     row.depth,
    //                 )
    //             })
    //             .collect::<Vec<_>>();
    //         let right_tree_actual = view_r
    //             .build_collapsed_rows(view_r.file_system.get_root_node_id(), 0)
    //             .into_iter()
    //             .map(|row| {
    //                 (
    //                     get_rel(&view_r.file_system, temp_r.path(), row.path),
    //                     row.depth,
    //                 )
    //             })
    //             .collect::<Vec<_>>();

    //         let out = FileSystemView::build_two_folder_diff_rows(
    //             Some(&view_l),
    //             Some(&view_r),
    //             &PathComparissonMethod::Byte,
    //         )
    //         .unwrap();

    //         let actual_diff: Vec<(String, FsNodeDepth, String)> = out
    //             .into_iter()
    //             .map(|row| {
    //                 let rel = get_rel_from_diff_state(
    //                     &view_l.file_system,
    //                     &view_r.file_system,
    //                     temp_l.path(),
    //                     temp_r.path(),
    //                     &row.diff_state,
    //                 );
    //                 let marker = match row.diff_state {
    //                     DiffState::OnlyInFirst(_) => "-",
    //                     DiffState::OnlyInSecond(_) => "+",
    //                     DiffState::Different(_, _) => "~",
    //                     DiffState::Same(_, _) => "=",
    //                     _ => "?",
    //                 };
    //                 (rel, row.depth, marker.to_string())
    //             })
    //             .collect();

    //         let expected_diff: Vec<(String, FsNodeDepth, String)> = case
    //             .expected
    //             .iter()
    //             .map(|(p, d, m)| (p.to_string(), *d, m.to_string()))
    //             .collect();

    //         let expected_output =
    //             format_diff_tree_two(&left_tree_actual, &right_tree_actual, &expected_diff);
    //         if actual_diff != expected_diff {
    //             let actual_output =
    //                 format_diff_tree_two(&left_tree_actual, &right_tree_actual, &actual_diff);

    //             panic!(
    //                 "\nCase Failed!: {}\n\nEXPECTED STATE:\n{}\nACTUAL STATE:\n{}",
    //                 case.name, expected_output, actual_output
    //             );
    //         } else {
    //             println!(
    //                 "\nPASS: {}\n\nACTUAL STATE:\n{}",
    //                 case.name, expected_output
    //             );
    //         }
    //     }
    // }

    fn setup_fs(root: &Path, structure: &[(&str, bool)]) {
        for (rel_path, is_dir) in structure {
            let full_path = root.join(rel_path);
            if *is_dir {
                fs::create_dir_all(&full_path).unwrap();
            } else {
                if let Some(parent) = full_path.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                File::create(&full_path).unwrap();
            }
        }
    }

    fn create_view(root: &Path, collapsed_paths: &[&str]) -> FileSystemView {
        let model = FileSystemModel::new(root).expect("Failed to create FileSystemModel");
        let mut collapsed = HashMap::new();
        for path in collapsed_paths {
            let id = find_id_by_rel_path(&model, root, path);
            collapsed.insert(id, true);
        }
        FileSystemView {
            file_system: Arc::new(model),
            collapsed,
            selected: HashMap::new(),
        }
    }

    fn find_id_by_rel_path(fs: &FileSystemModel, root: &Path, rel: &str) -> FsNodeId {
        let target = root.join(rel);
        for (id, node, _) in fs.iter_tree() {
            if node.as_path().as_ref() == target {
                return id;
            }
        }
        panic!("Test setup error: path {:?} not found in model", target);
    }
}
