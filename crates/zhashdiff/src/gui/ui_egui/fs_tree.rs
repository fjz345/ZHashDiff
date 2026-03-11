use std::{
    collections::{BTreeMap, HashMap}, io, path::Path, sync::Arc
};

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};
use zhashdiff::{
    comparison::{PathComparissonMethod, compare_paths},
    external_diff_tool::{DiffToolConfig, open_diff_tool},
    fs::{FileSystemModel, FsNode, FsNodeDepth, FsNodeId, FsNodeKind, TreeIter},
    hash::HashService,
};

use crate::ui_egui::common::{
    CheckboxSelectState, draw_persistent_hint_text_edit, hash_to_color, ui_custom_checkbox,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct FileSystemView {
    pub file_system: Arc<FileSystemModel>,
    pub collapsed: HashMap<FsNodeId, bool>,
    pub selected: HashMap<FsNodeId, bool>,
}

#[derive(Debug)]
pub struct VisibleRowTwoFolderDiff {
    pub is_dir: bool,
    pub depth: FsNodeDepth,
    pub diff_state: DiffState,
}

#[derive(Debug)]
pub struct VisibleRow {
    path: FsNodeId,
    is_dir: bool,
    depth: FsNodeDepth,
}

impl FileSystemView {
    pub fn new(model: Arc<FileSystemModel>) -> Self {
        Self {
            file_system: model,
            collapsed: HashMap::new(),
            selected: HashMap::new(),
        }
    }

    pub fn iter_nodes(&self, start_id: FsNodeId) -> TreeIter<'_> {
        self.file_system.iter_subtree(start_id)
    }

    pub fn is_anything_collapsed(&self, node_id: FsNodeId) -> bool {
        if let Some(node) = self.file_system.get_node(node_id) {
            if !node.is_dir() {
                return false;
            }
            let is_collapsed = self.collapsed.get(&node_id).copied().unwrap_or(false);
            if is_collapsed {
                return true;
            }
            if let Some(children) = node.children() {
                return self.is_anything_collapsed_slice(children);
            }
        }

        false
    }

    pub fn is_anything_collapsed_slice(&self, nodes: &[FsNodeId]) -> bool {
        nodes.iter().any(|&id| self.is_anything_collapsed(id))
    }

    pub fn recursive_collapse(&mut self, node_id: FsNodeId, collapse: bool) {
        let ids: Vec<FsNodeId> = self.iter_nodes(node_id)
            .filter(|(_, node, _)| node.is_dir())
            .map(|(id, _, _)| id)
            .collect();

        for id in ids {
            if collapse {
                self.collapsed.insert(id, true);
            } else {
                self.collapsed.remove(&id);
            }
        }
    }

    pub fn toggle_collapse(&mut self, id: FsNodeId) {
        let currently_collapsed = self.collapsed.get(&id).copied().unwrap_or(false);
        
        if currently_collapsed {
            self.collapsed.remove(&id); 
        } else {
            self.collapsed.insert(id, true);
        }
    }
    
    pub fn recursive_collapse_slice(&mut self, node_ids: &[FsNodeId], collapse: bool) {
        for node_id in node_ids {
            self.recursive_collapse(*node_id, collapse);
        }
    }

    pub fn recursive_selection(&mut self, node_id: FsNodeId, value: bool) {
        let ids: Vec<FsNodeId> = self.iter_nodes(node_id).map(|(id, _, _)| id).collect();
        for id in ids {
            self.selected.insert(id, value);
        }
    }

    pub fn build_two_folder_diff_rows(
        file_system_1: Option<&FileSystemView>,
        file_system_2: Option<&FileSystemView>,
        method: &PathComparissonMethod,
    ) -> io::Result<Vec<VisibleRowTwoFolderDiff>> {
        let mut entries_map: BTreeMap<
            String,
            (Option<(FsNodeId, &FsNode, FsNodeDepth)>, Option<(FsNodeId, &FsNode, FsNodeDepth)>),
        > = BTreeMap::new();

        let get_rel_path = |view: &FileSystemView, id: FsNodeId, node: &FsNode| -> String {
            let root_id = view.file_system.get_root_node_id();
            if id == root_id { return String::new(); }
            
            let root_node = view.file_system.get_node(root_id).unwrap();
            let root_path = root_node.as_path();
            
            // Use components to rebuild the path cleanly, avoiding slash/prefix issues
            node.as_path().as_ref().strip_prefix(&root_path)
                .unwrap_or(node.as_path().as_ref())
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        };

        if let Some(view) = file_system_1 {
            for (id, node, depth) in view.file_system.iter_tree() {
                let rel = get_rel_path(view, id, node);
                entries_map.insert(rel, (Some((id, node, depth)), None));
            }
        }
        if let Some(view) = file_system_2 {
            for (id, node, depth) in view.file_system.iter_tree() {
                let rel = get_rel_path(view, id, node);
                entries_map.entry(rel)
                    .and_modify(|e| e.1 = Some((id, node, depth)))
                    .or_insert((None, Some((id, node, depth))));
            }
        }
        
        let num_files_and_folders_1 = file_system_1.and_then(|f|Some(f.file_system.total_files_and_folders())).unwrap_or(0) as usize;
        let num_files_and_folders_2 = file_system_2.and_then(|f|Some(f.file_system.total_files_and_folders())).unwrap_or(0) as usize;
        let initial_capacity = num_files_and_folders_1.max(num_files_and_folders_2);
        let mut out_rows = Vec::with_capacity(initial_capacity);

        for (rel_path, (left, right)) in &entries_map {
            let depth = left.map(|l| l.2).or(right.map(|r| r.2)).unwrap_or(0);

            let is_dir = left.map(|l| l.1.is_dir()).or(right.map(|r| r.1.is_dir())).unwrap_or(false);

            let diff_state = match (left, right) {
                (Some((l_id, l_node, _)), Some((r_id, r_node, _))) => {
                    let partial_threshold = 1.0f32;
                    if l_node.is_dir() {
                        folder_diff_state(rel_path, &entries_map, method, partial_threshold)
                    } else {
                        file_diff_state((*l_id, l_node), (*r_id, r_node), method, partial_threshold)
                    }
                }
                (Some((l_id, _, _)), None) => DiffState::OnlyInFirst(*l_id),
                (None, Some((r_id, _, _))) => DiffState::OnlyInSecond(*r_id),
                (None, None) => panic!("unreachable"),
            };

            out_rows.push(VisibleRowTwoFolderDiff {
                is_dir,
                depth,
                diff_state,
            });
        }

        Ok(out_rows)
    }

    pub fn build_collapsed_rows(&self, start_id: FsNodeId, start_depth: FsNodeDepth) -> Vec<VisibleRow> {
        let mut out = Vec::new();
        let mut stack = vec![(start_id, start_depth)];

        while let Some((id, depth)) = stack.pop() {
            if let Some(node) = self.file_system.get_node(id) {
                let is_dir = node.is_dir();

                out.push(VisibleRow {
                    path: id,
                    is_dir,
                    depth,
                });

                let is_collapsed = self.collapsed.get(&id).copied().unwrap_or(false);
                if is_dir && !is_collapsed {
                    if let Some(children) = node.children() {
                        for &child_id in children.iter().rev() {
                            stack.push((child_id, depth + 1));
                        }
                    }
                }
            }
        }
        out
    }
}

pub fn draw_ui_folder_tree_with_checkbox(
    ui: &mut egui::Ui,
    file_system_view: &mut FileSystemView,
    hash_service: &mut HashService,
) -> egui::Response {
    let root_id = file_system_view.file_system.get_root_node_id();
    let root_path_clone = file_system_view
        .file_system
        .get_root()
        .as_path()
        .as_ref()
        .to_path_buf();

    let visible_rows = file_system_view.build_collapsed_rows(root_id, 0);
    let row_count = visible_rows.len();

    let available_height = ui.available_height();
    let mut header_toggle_selection = false;

    let root_selection_state = get_folder_selection_state(root_id, file_system_view);

    let response = egui::Frame::new()
        .fill(egui::Color32::from_gray(20))
        .inner_margin(0.0)
        .show(ui, |ui| {
            ui.set_min_height(available_height);
            let row_height = ui.text_style_height(&egui::TextStyle::Body);
            let row_height_header = ui.text_style_height(&egui::TextStyle::Heading);

            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            const DUMMY_HASH: &str =
                "321e84925aecc55ef828a41db03f0ccece66c7a6cd2a31975bcc5d029712db81";
            let galley =
                ui.painter()
                    .layout_no_wrap(DUMMY_HASH.into(), font_id, egui::Color32::PLACEHOLDER);
            let min_hash_width = galley.size().x + 20.0;

            TableBuilder::new(ui)
                .id_salt(root_path_clone)
                .striped(true)
                .resizable(true)
                .auto_shrink([false, true])
                .column(Column::exact(32.0))
                .column(Column::remainder().at_least(100.0))
                .column(
                    Column::initial(min_hash_width)
                        .at_least(min_hash_width)
                        .resizable(false),
                )
                .header(row_height_header, |mut header| {
                    header.col(|ui| {
                        ui.centered_and_justified(|ui| {
                            if ui_custom_checkbox(ui, root_selection_state.clone()).clicked() {
                                header_toggle_selection = true;
                            }
                        });
                    });
                    header.col(|ui| {
                        ui.centered_and_justified(|ui| {
                            ui.label("Name");
                        });
                    });
                    header.col(|ui| {
                        ui.centered_and_justified(|ui| {
                            ui.label("Hash");
                        });
                    });
                })
                .body(|body| {
                    body.rows(row_height, row_count, |mut row| {
                        let index = row.index();
                        if let Some(entry) = visible_rows.get(index) {
                            render_row_folder_tree_with_checkbox(
                                hash_service,
                                file_system_view,
                                &mut row,
                                entry,
                                row_height,
                            );
                        }
                    });
                });
        });

    if header_toggle_selection {
        let is_currently_checked = matches!(root_selection_state, CheckboxSelectState::Checked);
        file_system_view.recursive_selection(root_id, !is_currently_checked);
    }

    response.response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::fs::{self, File};
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;
    use zhashdiff::fs::FsIsDir;

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
        
        node.as_path().as_ref()
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
        state: &DiffState
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
        output.push_str(&format!("{:<30} | {:<30} | {:<30}\n", "LEFT TREE", "DIFF RESULT", "RIGHT TREE"));
        output.push_str(&"-".repeat(96));
        output.push('\n');

        let l_map: HashMap<&str, FsNodeDepth> = left.iter().map(|(p, d)| (p.as_str(), *d)).collect();
        let r_map: HashMap<&str, FsNodeDepth> = right.iter().map(|(p, d)| (p.as_str(), *d)).collect();
        
        for (path, depth, marker) in diff {
            let l_row = l_map.get(path.as_str())
                .map(|d| format!("{}└─ {}", "  ".repeat(*d as usize), if path.is_empty() { "/" } else { path }))
                .unwrap_or_default();
            
            let r_row = r_map.get(path.as_str())
                .map(|d| format!("{}└─ {}", "  ".repeat(*d as usize), if path.is_empty() { "/" } else { path }))
                .unwrap_or_default();
                
            let d_row = format!("{} {}└─ {}", marker, "  ".repeat(*depth as usize), if path.is_empty() { "/" } else { path });

            output.push_str(&format!("{:<30} | {:<30} | {:<30}\n", l_row, d_row, r_row));
        }
        output
    }

    #[test]
    fn test_build_collapsed_rows_scenarios_single_view() {
        let cases = vec![
            CollapsedTestCase {
                name: "Simple collapsed directory",
                structure: vec![
                    ("a", true),
                    ("a/file.txt", false),
                ],
                collapsed: vec!["a"], 
                expected: vec![
                    ("", 0), 
                    ("a", 1),
                ],
            },
            CollapsedTestCase {
                name: "Fully expanded directory",
                structure: vec![
                    ("dir_a", true),
                    ("dir_a/file_1.txt", false),
                    ("dir_b", true),
                ],
                collapsed: vec![],
                expected: vec![
                    ("", 0),
                    ("dir_a", 1),
                    ("dir_a/file_1.txt", 2),
                    ("dir_b", 1),
                ],
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
                expected: vec![
                    ("", 0),
                    ("level1", 1),
                    ("level1/level2", 2),
                ],
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

            let model = FileSystemModel::new(root_path);
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

            let actual: Vec<(String, FsNodeDepth)> = result.into_iter().map(|row| {
            let node = view.file_system.get_node(row.path).unwrap();
            let rel = node.as_path().as_ref()
                .strip_prefix(root_path).unwrap()
                .to_string_lossy()
                .replace('\\', "/");
                
                (rel, row.depth)
            }).collect();

            let expected_mapped: Vec<(String, FsNodeDepth)> = case.expected.iter()
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

    #[test]
    fn test_build_two_folder_diff_scenarios() {
        let cases = vec![
            DiffTestCase {
                name: "Standard file addition",
                left_structure: vec![("a.txt", false)],
                right_structure: vec![("a.txt", false), ("b.txt", false)],
                left_collapsed: vec![],
                right_collapsed: vec![],
                expected: vec![
                    ("", 0, "~"),
                    ("a.txt", 1, "="),
                    ("b.txt", 1, "+"),
                ],
            },
            DiffTestCase {
                name: "Collapsed directory hides children",
                left_structure: vec![("dir/file.txt", false)],
                right_structure: vec![("dir/one_extra_deep/file.txt", false), ("dir/a_file.txt", false)],
                left_collapsed: vec!["dir"],
                right_collapsed: vec!["dir"],
                expected: vec![
                    ("", 0, "~"),
                    ("dir", 1, "~"),
                ],
            },
            DiffTestCase {
                name: "One side collapsed directory should NOT hide children",
                left_structure: vec![("dir/hidden_file.txt", false)],
                right_structure: vec![("dir/one_extra_deep", true), ("dir/one_extra_deep/file.txt", false), ("dir/visible_file.txt", false)],
                left_collapsed: vec!["dir"],
                right_collapsed: vec![],
                expected: vec![
                    ("", 0, "~"),
                    ("dir", 1, "~"),
                    ("dir/hidden_file.txt", 2, "-"),
                    ("dir/one_extra_deep", 2, "+"),
                    ("dir/one_extra_deep/file.txt", 3, "+"),
                    ("dir/visible_file.txt", 2, "+"),
                ],
            },
            DiffTestCase {
                name: "Complex nested diff with partial collapse",
                left_structure: vec![
                    ("common/deleted.txt", false),
                    ("common/same.txt", false),
                    ("nested/level1/level2/file.txt", false),
                    ("only_left/a.txt", false),
                ],
                right_structure: vec![
                    ("common/added.txt", false),
                    ("common/same.txt", false),
                    ("nested/level1/level2/file.txt", false),
                    ("only_right/b.txt", false),
                ],
                left_collapsed: vec!["nested/level1"],
                right_collapsed: vec!["nested/level1"], 
                expected: vec![
                    ("", 0, "~"),
                    ("common", 1, "~"),
                    ("common/added.txt", 2, "+"),
                    ("common/deleted.txt", 2, "-"),
                    ("common/same.txt", 2, "="),
                    ("nested", 1, "="),
                    ("nested/level1", 2, "="), 
                    ("only_left", 1, "-"),
                    ("only_left/a.txt", 2, "-"),
                    ("only_right", 1, "+"),
                    ("only_right/b.txt", 2, "+"),
                ],
            },
        ];

        for case in cases {
            let temp_l = tempdir().unwrap();
            let temp_r = tempdir().unwrap();
            
            setup_fs(temp_l.path(), &case.left_structure);
            setup_fs(temp_r.path(), &case.right_structure);

            let view_l = create_view(temp_l.path(), &case.left_collapsed);
            let view_r = create_view(temp_r.path(), &case.right_collapsed);

            let left_tree_actual = view_l.build_collapsed_rows(view_l.file_system.get_root_node_id(), 0)
                .into_iter().map(|row| (get_rel(&view_l.file_system, temp_l.path(), row.path), row.depth)).collect::<Vec<_>>();
            let right_tree_actual = view_r.build_collapsed_rows(view_r.file_system.get_root_node_id(), 0)
                .into_iter().map(|row| (get_rel(&view_r.file_system, temp_r.path(), row.path), row.depth)).collect::<Vec<_>>();

            let out = FileSystemView::build_two_folder_diff_rows(
                Some(&view_l),
                Some(&view_r),
                &PathComparissonMethod::Byte,
            ).unwrap();
            

            let actual_diff: Vec<(String, FsNodeDepth, String)> = out.into_iter().map(|row| {
                let rel = get_rel_from_diff_state(&view_l.file_system, &view_r.file_system, temp_l.path(), temp_r.path(), &row.diff_state);
                let marker = match row.diff_state {
                    DiffState::OnlyInFirst(_) => "-",
                    DiffState::OnlyInSecond(_) => "+",
                    DiffState::Different(_, _) => "~",
                    DiffState::Same(_, _) => "=",
                    _ => "?",
                };
                (rel, row.depth, marker.to_string())
            }).collect();

            let expected_diff: Vec<(String, FsNodeDepth, String)> = case.expected.iter()
                .map(|(p, d, m)| (p.to_string(), *d, m.to_string()))
                .collect();

            let expected_output = format_diff_tree_two(&left_tree_actual, &right_tree_actual, &expected_diff);
            if actual_diff != expected_diff {
                let actual_output = format_diff_tree_two(&left_tree_actual, &right_tree_actual, &actual_diff);

                panic!(
                    "\nCase Failed!: {}\n\nEXPECTED STATE:\n{}\nACTUAL STATE:\n{}",
                    case.name,
                    expected_output,
                    actual_output
                );
            } else {
                println!("\nPASS: {}\n\nACTUAL STATE:\n{}", case.name, expected_output);
            }
        }
    }

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
        let model = FileSystemModel::new(root);
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

pub fn draw_ui_two_folder_tree_with_diff(
    ui: &mut egui::Ui,
    file_system_1_view: &mut Option<FileSystemView>,
    file_system_2_view: &mut Option<FileSystemView>,
    visible_rows: &mut Option<Vec<VisibleRowTwoFolderDiff>>,
    open_dir_window_1: &mut bool,
    open_dir_window_2: &mut bool,
    diff_tool_config: &DiffToolConfig,
) -> egui::response::Response {
    if file_system_1_view.is_none() && file_system_2_view.is_none() {
        return ui
            .vertical_centered(|ui| {
                ui.add_space(100.0);
                ui.heading("Welcome to ZHashDiff");
                ui.label("Drag and drop folders here or use the buttons below to start comparing.");
                ui.horizontal(|ui| {
                    if ui.button("Open Folder 1").clicked() {
                        *open_dir_window_1 = true;
                    }
                    if ui.button("Open Folder 2").clicked() {
                        *open_dir_window_2 = true;
                    }
                });
                handle_drops(
                    ui,
                    file_system_1_view,
                    file_system_2_view,
                    egui::Rect::EVERYTHING,
                    egui::Rect::EVERYTHING,
                )
            })
            .response;
    }

    let visible_rows = visible_rows.as_ref().unwrap();

    let row_count = visible_rows.len();
    let available_height = ui.available_height();
    let available_width = ui.available_width();
    let mut col0_rect = egui::Rect::NOTHING;
    let mut col2_rect = egui::Rect::NOTHING;
    let table_top = ui.cursor().top();

    let frame_output = egui::Frame::default()
        .fill(egui::Color32::from_gray(20))
        .inner_margin(0.0)
        .show(ui, |ui| {
            ui.set_min_height(available_height);
            ui.set_min_width(available_width);

            let row_height = ui.text_style_height(&egui::TextStyle::Body);
            let row_height_header = ui.text_style_height(&egui::TextStyle::Heading);
            let available_size = ui.available_size();

            TableBuilder::new(ui)
                .sense(egui::Sense::all())
                .id_salt("two_folder_diff_table")
                .striped(true)
                .resizable(false)
                .auto_shrink([false, true])
                .column(
                    Column::initial(available_size.x * 0.5)
                        .at_least(100.0)
                        .resizable(true),
                )
                .column(Column::auto().resizable(true).auto_size_this_frame(true))
                .column(Column::remainder().resizable(true).clip(false))
                .header(row_height_header, |mut header| {
                    header.col(|ui| {
                        col0_rect = ui.max_rect();
                        let available = ui.available_width();
                        ui.vertical(|ui| {
                            if ui
                                .add_sized(
                                    [available, row_height_header],
                                    egui::Button::new("Open Folder 1"),
                                )
                                .clicked()
                            {
                                *open_dir_window_1 = true;
                            }
                            let text = if let Some(fs1_view) = file_system_1_view {
                                fs1_view
                                    .file_system
                                    .get_root()
                                    .as_path()
                                    .as_ref()
                                    .to_string_lossy()
                                    .to_string()
                            } else {
                                "No folder".to_string()
                            };
                            let available = ui.available_width();
                            let id = ui.make_persistent_id("two_folder_diff_fs1_path");
                            let _ = draw_persistent_hint_text_edit(
                                ui,
                                id,
                                text,
                                [available, row_height_header],
                            );
                        });
                    });
                    header.col(|ui| {
                        ui.vertical(|ui| {
                            ui.label("≠");
                            if let Some(fs1_view) = file_system_1_view {
                                let root_1_id = fs1_view.file_system.get_root_node_id();
                                if let Some(row) = visible_rows.iter().find(|r| r.diff_state.first() == Some(root_1_id)) {
                                    ui_custom_diff_state(ui, &row.diff_state);
                                }
                            } else if let Some(fs2_view) = file_system_2_view {
                                let root_2_id = fs2_view.file_system.get_root_node_id();
                                if let Some(row) = visible_rows.iter().find(|r| r.diff_state.second() == Some(root_2_id)) {
                                    ui_custom_diff_state(ui, &row.diff_state);
                                }
                            }
                        });
                    });
                    header.col(|ui| {
                        col2_rect = ui.max_rect();
                        let available = ui.available_width();
                        ui.vertical(|ui| {
                            if ui
                                .add_sized(
                                    [available, row_height_header],
                                    egui::Button::new("Open Folder 2"),
                                )
                                .clicked()
                            {
                                *open_dir_window_2 = true;
                            }
                            let text = if let Some(fs2_view) = file_system_2_view {
                                let fs2 = &fs2_view.file_system;
                                fs2.get_root()
                                    .as_path()
                                    .as_ref()
                                    .to_string_lossy()
                                    .to_string()
                            } else {
                                "No folder".to_string()
                            };

                            let available = ui.available_width();
                            let id = ui.make_persistent_id("two_folder_diff_fs2_path");
                            let _ = draw_persistent_hint_text_edit(
                                ui,
                                id,
                                text,
                                [available, row_height_header],
                            );
                        });
                    });
                })
                .body(|body| {
                    body.rows(row_height, row_count, |mut row| {
                        let entry = &visible_rows[row.index()];
                        render_row_folder_tree_diff_column(
                            file_system_1_view.as_mut(),
                            file_system_2_view.as_mut(),
                            &mut row,
                            entry,
                            row_height,
                            diff_tool_config,
                        );
                    });
                });
        });

    let table_bottom = ui.min_rect().bottom();
    col0_rect.set_top(table_top);
    col0_rect.set_bottom(table_bottom);
    col2_rect.set_top(table_top);
    col2_rect.set_bottom(table_bottom);

    handle_drops(
        ui,
        file_system_1_view,
        file_system_2_view,
        col0_rect,
        col2_rect,
    );

    frame_output.response
}

fn handle_drops(
    ui: &egui::Ui,
    fs1_view: &mut Option<FileSystemView>,
    fs2_view: &mut Option<FileSystemView>,
    rect1: egui::Rect,
    rect2: egui::Rect,
) -> bool {
    return ui.input(|i| {
        if let Some(drop_pos) = i.pointer.hover_pos() {
            for dropped_file in &i.raw.dropped_files {
                if let Some(path) = &dropped_file.path {
                    if rect1.contains(drop_pos) {
                        *fs1_view =
                            Some(FileSystemView::new(Arc::new(FileSystemModel::new(&path))));
                        return true;
                    } else if rect2.contains(drop_pos) {
                        *fs2_view =
                            Some(FileSystemView::new(Arc::new(FileSystemModel::new(&path))));
                        return true;
                    }
                    break;
                }
            }
        }
        return false;
    });
}

#[derive(PartialEq, Debug)]
pub enum DiffState {
    Different(FsNodeId, FsNodeId),
    Same(FsNodeId, FsNodeId),
    Partial(FsNodeId, FsNodeId),
    OnlyInFirst(FsNodeId),
    OnlyInSecond(FsNodeId),
}

impl DiffState {
    pub fn first(&self) -> Option<FsNodeId> {
        match &self {
            DiffState::Same(path_buf, _)
            | DiffState::Different(path_buf, _)
            | DiffState::Partial(path_buf, _) => Some(*path_buf),
            DiffState::OnlyInFirst(path_buf) => Some(*path_buf),
            DiffState::OnlyInSecond(..) => None,
        }
    }
    pub fn second(&self) -> Option<FsNodeId> {
        match &self {
            DiffState::Same(_, path_buf1)
            | DiffState::Different(_, path_buf1)
            | DiffState::Partial(_, path_buf1) => Some(*path_buf1),
            DiffState::OnlyInFirst(..) => None,
            DiffState::OnlyInSecond(path_buf) => Some(*path_buf),
        }
    }
}

pub fn ui_custom_diff_state(ui: &mut egui::Ui, state: &DiffState) -> egui::response::Response {
    let green = egui::Color32::from_rgb(0x7E, 0xD3, 0x21);
    let red = egui::Color32::from_rgb(0xD0, 0x02, 0x1B);
    let yellow = egui::Color32::from_rgb(0xF8, 0xE7, 0x1C);
    let blue = egui::Color32::from_rgb(0x4A, 0x90, 0xE2);
    // let teal = egui::Color32::from_rgb(0x50, 0xE3, 0xC2);

    match state {
        DiffState::Same(..) => ui.label(RichText::new("=").color(green)),
        DiffState::Different(..) => ui.label(RichText::new("≠").color(red)),
        DiffState::Partial(..) => ui.label(RichText::new("≈").color(yellow)),
        DiffState::OnlyInFirst(..) => ui.label(RichText::new("−").color(blue)),
        DiffState::OnlyInSecond(..) => ui.label(RichText::new("+").color(blue)),
    }
}

fn file_diff_state(
    left: (FsNodeId, &FsNode),
    right: (FsNodeId, &FsNode),
    method: &PathComparissonMethod,
    partial_threshold: f32,
) -> DiffState {
    let path1 = left.1.as_path();
    let path2 = right.1.as_path();

    let result = match compare_paths(path1, path2, method) {
        Ok(r) => r,
        Err(_) => return DiffState::Different(left.0, right.0),
    };

    let likeness = result.likeness();

    if likeness == 1.0 {
        DiffState::Same(left.0, right.0)
    } else if likeness >= partial_threshold {
        DiffState::Partial(left.0, right.0)
    } else {
        DiffState::Different(left.0, right.0)
    }
}

fn folder_diff_state(
    parent_path: &str,
    entries_map: &BTreeMap<String, (Option<(FsNodeId, &FsNode, FsNodeDepth)>, Option<(FsNodeId, &FsNode, FsNodeDepth)>)>,
    method: &PathComparissonMethod,
    threshold: f32,
) -> DiffState {
    let (current_left, current_right) = entries_map.get(parent_path).cloned().unwrap_or((None, None));
    let l_id = current_left.map(|l| l.0).unwrap_or(0);
    let r_id = current_right.map(|r| r.0).unwrap_or(0);

    let prefix = if parent_path.is_empty() { String::new() } else { format!("{}/", parent_path) };

    for (path, (left, right)) in entries_map.range(prefix.clone()..) {
        if !path.starts_with(&prefix) { break; }
        if path == parent_path { continue; }

        match (left, right) {
            // If a child exists on one side only, the parent is "Different" (Modified)
            (Some(_), None) | (None, Some(_)) => return DiffState::Different(l_id, r_id),
            (Some((li, ln, _)), Some((ri, rn, _))) => {
                if !ln.is_dir() {
                    let s = file_diff_state((*li, ln), (*ri, rn), method, threshold);
                    // Use your specific enum variant names
                    if !matches!(s, DiffState::Same(..)) {
                        return DiffState::Different(l_id, r_id);
                    }
                }
            }
            _ => {}
        }
    }

    DiffState::Same(l_id, r_id)
}

fn on_row_item_clicked(
    file_system_1_view: Option<&FileSystemView>,
    file_system_2_view: Option<&FileSystemView>,
    entry: &VisibleRowTwoFolderDiff,
    config: &DiffToolConfig,
) -> bool {
    log::info!("on_row_iten_clicked");

    let path1 = entry.diff_state.first();
    let path2 = entry.diff_state.second();

    match (path1, path2) {
        (None, None) => panic!("should not have a row if both paths are none"),
        (None, Some(p)) | (Some(p), None) => {
            log::info!("nothing to diff, only in one tree {:?}", p);
            return false;
        }
        (Some(path1), Some(path2)) => {
            assert_eq!(file_system_1_view.is_some(), file_system_2_view.is_some()); // Model/View diff
            let diff_tool = config;
            let result = open_diff_tool(
                &diff_tool,
                file_system_1_view
                    .unwrap()
                    .file_system
                    .get_node(path1)
                    .unwrap()
                    .as_path(),
                file_system_2_view
                    .unwrap()
                    .file_system
                    .get_node(path2)
                    .unwrap()
                    .as_path(),
            );
            if let Err(err) = result {
                log::error!("diffing failed...");
                log::error!("{err}");
                return false;
            };
        }
    }

    return true;
}

fn render_diff_side(
    ui: &mut egui::Ui,
    view: Option<&FileSystemView>,
    node_id: Option<FsNodeId>,
    depth: FsNodeDepth,
    is_dir: bool,
    is_collapsed: bool,
    row_height: f32,
    mut on_click: impl FnMut(),
    mut on_toggle: impl FnMut(),
) {
    ui.horizontal(|ui| {
        ui.add_space((depth as f32) * 16.0);

        if let (Some(v), Some(id)) = (view, node_id) {
            if let Some(node) = v.file_system.get_node(id) {
                if is_dir {
                    // Openness: 0.0 is closed, 1.0 is open
                    let openness = if is_collapsed { 0.0 } else { 1.0 };
                    let (_rect, response) = ui.allocate_exact_size(
                        egui::vec2(12.0, row_height), 
                        egui::Sense::click()
                    );
                    egui::collapsing_header::paint_default_icon(ui, openness, &response);

                    if response.clicked() {
                        on_toggle();
                    }

                    let label_resp = ui
                        .label(format!("📁 {}", node.display_name()))
                        .interact(egui::Sense::click());

                    if label_resp.clicked() {
                        on_toggle();
                    }
                } else {
                    if ui
                        .label(node.display_name())
                        .interact(egui::Sense::click())
                        .clicked()
                    {
                        on_click();
                    }
                }
            }
        }
    });
}

fn render_row_folder_tree_diff_column(
    mut file_system_1_view: Option<&mut FileSystemView>,
    mut file_system_2_view: Option<&mut FileSystemView>,
    row: &mut egui_extras::TableRow,
    entry: &VisibleRowTwoFolderDiff,
    row_height: f32,
    diff_tool_config: &DiffToolConfig,
) {
    let first_node_id = entry.diff_state.first();
    let second_node_id = entry.diff_state.second();
    let mut should_toggle_row = false;

    let is_root = match (&file_system_1_view, &file_system_2_view, &first_node_id, &second_node_id) {
        (Some(v1), _, Some(first_id), _) => *first_id == v1.file_system.get_root_node_id(),
        (_, Some(v2), _, Some(second_id)) => *second_id == v2.file_system.get_root_node_id(),
        _ => false,
    };
    // Skip the root
    if is_root {
        return;
    }

    let get_collapsed_state = |v: &FileSystemView, id: &FsNodeId| {
        let collapsed = v.collapsed.get(id).copied().unwrap_or(false);
        let parent_collapsed = v.file_system.get_parent_id(*id)
            .and_then(|p_id| v.collapsed.get(&p_id).copied())
            .unwrap_or(false);
        (collapsed, parent_collapsed)
    };

    // Destructure into specific options for each view's state
    let (state1, state2) = match (&file_system_1_view, &file_system_2_view, &first_node_id, &second_node_id) {
        (Some(v1), Some(v2), Some(id1), Some(id2)) => {
            (Some(get_collapsed_state(v1, id1)), Some(get_collapsed_state(v2, id2)))
        }
        (Some(v), _, Some(id1), _) => {
            (Some(get_collapsed_state(v, id1)), None)
        }
        (_, Some(v), _, Some(id2)) => {
            (None, Some(get_collapsed_state(v, id2)))
        }
        _ => panic!("unreachable"),
    };

    let is_collapsed_1 = state1.and_then(|f|Some(f.0)).unwrap_or(false);
    let is_parent_collapsed_1 = state1.and_then(|f|Some(f.1)).unwrap_or(false);
    let is_collapsed_2 = state2.and_then(|f|Some(f.0)).unwrap_or(false);
    let is_parent_collapsed_2 = state2.and_then(|f|Some(f.1)).unwrap_or(false);
    // If both parets are collapsed or invalid, hide the whole row (skip index)
    let hide_row = (is_parent_collapsed_1 || state1.is_none()) && (is_parent_collapsed_2 || state2.is_none());
    // --- Left Column (Folder 1) ---
    if !hide_row
    {
        if !is_parent_collapsed_1
        {
            row.col(|ui| {
                render_diff_side(ui, file_system_1_view.as_deref(), first_node_id, entry.depth, entry.is_dir, is_collapsed_1, row_height, ||{
                    on_row_item_clicked(
                        file_system_1_view.as_deref(),
                        file_system_2_view.as_deref(),
                        entry,
                        diff_tool_config,
                    );
                }, ||{should_toggle_row = true});
            });
        }
        // --- Middle Column (Diff Status) ---
        if (!is_parent_collapsed_1 && !is_parent_collapsed_2)
        {
            row.col(|ui| {
                ui.horizontal(|ui| {
                    ui_custom_diff_state(ui, &entry.diff_state);
                });
            });
        }
        // --- Right Column (Folder 2) ---
        if !is_parent_collapsed_2
        {
            row.col(|ui| {
                render_diff_side(ui, file_system_2_view.as_deref(), second_node_id, entry.depth, entry.is_dir, is_collapsed_2, row_height, ||{
                    on_row_item_clicked(
                        file_system_1_view.as_deref(),
                        file_system_2_view.as_deref(),
                        entry,
                        diff_tool_config,
                    );
                }, ||{should_toggle_row = true});
            });
        }
    }

    if should_toggle_row {
        if let Some(first) = &entry.diff_state.first()
        {
            if let Some(view) = file_system_1_view.as_mut() {
                view.toggle_collapse(*first);
            }
        }
        if let Some(second) = &entry.diff_state.second()
        {
            if let Some(view) = file_system_2_view.as_mut()
            {
                view.toggle_collapse(*second);
            }
        }
    }
}

fn render_row_folder_tree_with_checkbox(
    hash_service: &mut HashService,
    file_system_view: &mut FileSystemView,
    row: &mut egui_extras::TableRow,
    entry: &VisibleRow,
    row_height: f32,
) {
    let path_id = entry.path;
    let is_dir = entry.is_dir;

    let mut toggle_collapse = false;
    let mut toggle_selection = false;

    let node = file_system_view.file_system.get_node(path_id).unwrap();
    let is_collapsed = file_system_view
        .collapsed
        .get(&path_id)
        .copied()
        .unwrap_or(false);
    let is_selected = file_system_view
        .selected
        .get(&path_id)
        .copied()
        .unwrap_or(false);

    row.col(|ui| {
        ui.centered_and_justified(|ui| {
            let state = if is_dir {
                get_folder_selection_state(path_id, &file_system_view)
            } else if is_selected {
                CheckboxSelectState::Checked
            } else {
                CheckboxSelectState::Unchecked
            };

            if ui_custom_checkbox(ui, state.clone()).clicked() {
                toggle_selection = true;
            }
        });
    });

    // Column 2: Name & collapse Icon
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui.add_space((entry.depth as f32) * 16.0);

            if is_dir {
                let openness = if is_collapsed { 1.0 } else { 0.0 };
                let (_rect, response) =
                    ui.allocate_exact_size(egui::vec2(12.0, row_height), egui::Sense::click());
                egui::collapsing_header::paint_default_icon(ui, openness, &response);

                if response.clicked() {
                    toggle_collapse = true;
                }

                let label = format!("📁 {}", node.display_name());
                if ui.label(label).interact(egui::Sense::click()).clicked() {
                    toggle_collapse = true;
                }
            } else {
                ui.label(node.display_name());
            }
        });
    });

    // Column 3: Hash / Progress
    row.col(|ui| {
        let full_path = node.as_path();
        if !is_dir {
            match hash_service.get_hash(&full_path) {
                Some(hash_str) => {
                    let bg_color = hash_to_color(&hash_str);
                    egui::Frame::canvas(ui.style())
                        .fill(bg_color)
                        .corner_radius(3.0)
                        .inner_margin(egui::Margin::symmetric(4, 2))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(hash_str)
                                    .monospace()
                                    .color(egui::Color32::BLACK),
                            );
                        });
                }
                None => {
                    hash_service.request(full_path.as_ref().to_path_buf());
                    ui.weak("pending...");
                }
            }
        } else {
            let snapshot = hash_service.snapshot();
            let subtree_prefix = full_path.as_ref();

            let mut total = 0;
            let mut hashed = 0;

            for (p, h) in &snapshot.hashes {
                if p.starts_with(subtree_prefix) {
                    total += 1;
                    if h.is_some() {
                        hashed += 1;
                    }
                }
            }

            if total > 0 {
                let progress = hashed as f32 / total as f32;
                ui.horizontal(|ui| {
                    ui.add(
                        egui::ProgressBar::new(progress)
                            .show_percentage()
                            .desired_width(100.0),
                    );
                    if progress < 1.0 {
                        ui.weak(format!("{}/{}", hashed, total));
                    }
                });
            }
        }
    });

    if toggle_collapse {
        file_system_view.toggle_collapse(path_id);
    }
    if toggle_selection {
        file_system_view.recursive_selection(path_id, !is_selected);
    }
}

fn get_folder_selection_state(
    path: FsNodeId,
    file_system_view: &FileSystemView,
) -> CheckboxSelectState {
    let mut has_selected = false;
    let mut has_unselected = false;

    if *file_system_view.selected.get(&path).unwrap_or(&false) {
        has_selected = true;
    } else {
        has_unselected = true;
    }

    let node = file_system_view.file_system.get_node(path).unwrap();
    if let Some(children) = node.children() {
        for child_node_id in children {
            let is_selected = *file_system_view
                .selected
                .get(child_node_id)
                .unwrap_or(&false);

            if is_selected {
                has_selected = true;
            } else {
                has_unselected = true;
            }

            // Early exit if mixed
            if has_selected && has_unselected {
                return CheckboxSelectState::Partial;
            }
        }
    }

    if has_selected {
        CheckboxSelectState::Checked
    } else {
        CheckboxSelectState::Unchecked
    }
}
