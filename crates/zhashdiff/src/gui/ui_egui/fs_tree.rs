use std::{
    collections::{BTreeMap, HashMap},
    io,
    sync::Arc,
};

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};
use zhashdiff::{
    comparison::{PathComparissonMethod, compare_paths},
    external_diff_tool::{DiffToolConfig, open_diff_tool},
    fs::{FileSystemModel, FsNode, FsNodeId, FsNodeKind, TreeIter},
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
    path: FsNodeId,
    is_dir: bool,
    depth: u16,
    diff_state: DiffState,
}

#[derive(Debug)]
pub struct VisibleRow {
    path: FsNodeId,
    is_dir: bool,
    depth: u16,
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

    pub fn recursive_selection(&mut self, node_id: FsNodeId, value: bool) {
        let ids: Vec<FsNodeId> = self.iter_nodes(node_id).map(|(id, _, _)| id).collect();
        for id in ids {
            self.selected.insert(id, value);
        }
    }

    pub fn recursive_collapse(&mut self, node_id: FsNodeId, collapse: bool) {
        let ids: Vec<FsNodeId> = self
            .iter_nodes(node_id)
            .filter(|(_, node, _)| node.is_dir())
            .map(|(id, _, _)| id)
            .collect();

        for id in ids {
            if collapse {
                self.collapsed.insert(id, true);
            } else {
                self.collapsed.insert(id, false);
                // self.collapsed.remove(&id);
            }
        }
    }
    
    pub fn recursive_collapse_slice(&mut self, node_ids: &[FsNodeId], collapse: bool) {
        for node_id in node_ids {
            self.recursive_collapse(*node_id, collapse);
        }
    }

    pub fn toggle_collapse(&mut self, id: FsNodeId) {
        let entry = self.collapsed.entry(id).or_insert(false);
        *entry = !*entry;
    }

    pub fn build_two_folder_diff_rows(
        file_system_1: Option<&FileSystemView>,
        file_system_2: Option<&FileSystemView>,
        out: &mut Vec<VisibleRowTwoFolderDiff>,
        method: &PathComparissonMethod,
    ) -> io::Result<()> {
        let partial_threshold = 1.0f32;
        out.clear();

        let mut entries_map: BTreeMap<
            FsNodeId,
            (
                Option<(FsNodeId, &FsNode, u16)>,
                Option<(FsNodeId, &FsNode, u16)>,
            ),
        > = BTreeMap::new();

        if let Some(view) = file_system_1 {
            for (id, node, depth) in view.file_system.iter_tree() {
                entries_map.insert(id, (Some((id, node, depth)), None));
            }
        }

        if let Some(view) = file_system_2 {
            for (id, node, depth) in view.file_system.iter_tree() {
                entries_map
                    .entry(id)
                    .and_modify(|e| e.1 = Some((id, node, depth)))
                    .or_insert((None, Some((id, node, depth))));
            }
        }

        let mut skip_below_depth: Option<u16> = None;

        for (&node_id, (left, right)) in &entries_map {
            let depth = left.map(|l| l.2).or(right.map(|r| r.2)).unwrap_or(0);
            if let Some(limit) = skip_below_depth {
                if depth > limit {
                    continue;
                } else {
                    skip_below_depth = None;
                }
            }

            let is_collapsed = match (left, file_system_1, right, file_system_2) {
                (Some((id, _, _)), Some(v1), _, _) => v1.collapsed.get(id).copied().unwrap_or(false),
                (_, _, Some((id, _, _)), Some(v2)) => v2.collapsed.get(id).copied().unwrap_or(false),
                _ => true,
            };

            let is_dir = left
                .map(|l| l.1.is_dir())
                .or(right.map(|r| r.1.is_dir()))
                .unwrap_or(false);

            if is_dir && is_collapsed {
                skip_below_depth = Some(depth);
            }

            // Diff Calculation
            let diff_state = match (left, right) {
                (Some((l_id, l_node, _)), Some((r_id, r_node, _))) => {
                    if l_node.is_dir() {
                        folder_diff_state(node_id, &entries_map, method, partial_threshold)
                    } else {
                        file_diff_state((*l_id, l_node), (*r_id, r_node), method, partial_threshold)
                    }
                }
                (Some((l_id, _, _)), None) => DiffState::OnlyInFirst(*l_id),
                (None, Some((r_id, _, _))) => DiffState::OnlyInSecond(*r_id),
                (None, None) => continue,
            };

            out.push(VisibleRowTwoFolderDiff {
                path: node_id,
                is_dir,
                depth,
                diff_state,
            });
        }

        Ok(())
    }

    pub fn build_collapsed_rows(&self, start_id: FsNodeId, start_depth: u16) -> Vec<VisibleRow> {
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

                if is_dir && self.collapsed.get(&id).copied().unwrap_or(false) {
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
        .pathbuf()
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

pub fn draw_ui_two_folder_tree_with_diff(
    ui: &mut egui::Ui,
    file_system_1_view: &mut Option<FileSystemView>,
    file_system_2_view: &mut Option<FileSystemView>,
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
    let mut visible_rows = Vec::new();
    FileSystemView::build_two_folder_diff_rows(
        file_system_1_view.as_ref(),
        file_system_2_view.as_ref(),
        &mut visible_rows,
        &PathComparissonMethod::CrC,
    ).expect("Failed");

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
                                    .pathbuf()
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
                            if let Some(fs1_fiew) = file_system_1_view {
                                let fs1 = &fs1_fiew.file_system;
                                let root_1 = fs1.get_root();
                                if let Some(row) = visible_rows
                                    .iter()
                                    .find(|r| fs1.get_node(r.path).map_or(false, |n| n == root_1))
                                {
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
                                    .pathbuf()
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
    let path1 = left.1.pathbuf();
    let path2 = right.1.pathbuf();

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
pub fn folder_diff_state(
    node_id: FsNodeId,
    entries_map: &BTreeMap<
        FsNodeId,
        (
            Option<(FsNodeId, &FsNode, u16)>,
            Option<(FsNodeId, &FsNode, u16)>,
        ),
    >,
    method: &PathComparissonMethod,
    partial_threshold: f32,
) -> DiffState {
    use DiffState::*;

    let mut seen_same = false;
    let mut seen_partial = false;
    let mut seen_diff = false;
    let mut seen_only_first = false;
    let mut seen_only_second = false;
    let mut has_children = false;

    // Find children of this node
    for (&_child_id, (left, right)) in entries_map {
        let parent_id = left
            .as_ref()
            .map(|(_, n, _)| n.parent)
            .or_else(|| right.as_ref().map(|(_, n, _)| n.parent));

        if parent_id.unwrap() != Some(node_id) {
            continue;
        }

        has_children = true;

        let state = match (left, right) {
            (Some((l_id, l_node, _)), Some((r_id, r_node, _))) => {
                if matches!(l_node.kind, FsNodeKind::Dir { .. }) {
                    folder_diff_state(*l_id, entries_map, method, partial_threshold)
                } else {
                    file_diff_state((*l_id, l_node), (*r_id, r_node), method, partial_threshold)
                }
            }
            (Some((l_id, _, _)), None) => OnlyInFirst(*l_id),
            (None, Some((r_id, _, _))) => OnlyInSecond(*r_id),
            (None, None) => continue,
        };

        match state {
            Same(..) => seen_same = true,
            Partial(..) => seen_partial = true,
            Different(..) => seen_diff = true,
            OnlyInFirst(..) => seen_only_first = true,
            OnlyInSecond(..) => seen_only_second = true,
        }
    }

    if !has_children {
        if let Some((id, _, _)) = entries_map.get(&node_id).and_then(|(l, _)| *l) {
            return Same(id, id);
        } else if let Some((id, _, _)) = entries_map.get(&node_id).and_then(|(_, r)| *r) {
            return Same(id, id);
        } else {
            panic!("Folder node not found in entries_map");
        }
    }

    let has_any_diff = seen_diff || seen_partial || seen_only_first || seen_only_second;

    if seen_partial || (seen_same && has_any_diff) {
        if let Some((id, _, _)) = entries_map.get(&node_id).and_then(|(l, _)| *l) {
            Partial(id, id)
        } else if let Some((id, _, _)) = entries_map.get(&node_id).and_then(|(_, r)| *r) {
            Partial(id, id)
        } else {
            panic!("Folder node not found in entries_map");
        }
    } else if seen_same {
        if let Some((id, _, _)) = entries_map.get(&node_id).and_then(|(l, _)| *l) {
            Same(id, id)
        } else if let Some((id, _, _)) = entries_map.get(&node_id).and_then(|(_, r)| *r) {
            Same(id, id)
        } else {
            panic!("Folder node not found in entries_map");
        }
    } else if seen_only_first && !seen_only_second && !seen_diff {
        if let Some((id, _, _)) = entries_map.get(&node_id).and_then(|(l, _)| *l) {
            OnlyInFirst(id)
        } else {
            panic!("Folder node not found in entries_map");
        }
    } else if seen_only_second && !seen_only_first && !seen_diff {
        if let Some((id, _, _)) = entries_map.get(&node_id).and_then(|(_, r)| *r) {
            OnlyInSecond(id)
        } else {
            panic!("Folder node not found in entries_map");
        }
    } else {
        if let Some((id, _, _)) = entries_map.get(&node_id).and_then(|(l, _)| *l) {
            Different(id, id)
        } else if let Some((id, _, _)) = entries_map.get(&node_id).and_then(|(_, r)| *r) {
            Different(id, id)
        } else {
            panic!("Folder node not found in entries_map");
        }
    }
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
                    .pathbuf(),
                file_system_2_view
                    .unwrap()
                    .file_system
                    .get_node(path2)
                    .unwrap()
                    .pathbuf(),
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

fn render_row_folder_tree_diff_column(
    file_system_1_view: Option<&mut FileSystemView>,
    file_system_2_view: Option<&mut FileSystemView>,
    row: &mut egui_extras::TableRow,
    entry: &VisibleRowTwoFolderDiff,
    row_height: f32,
    diff_tool_config: &DiffToolConfig,
) {
    let path = entry.path;
    let is_dir = entry.is_dir;
    let mut should_toggle = false;

    let is_root = match (&file_system_1_view, &file_system_2_view) {
        (Some(v1), _) => path == v1.file_system.get_root_node_id(),
        (_, Some(v2)) => path == v2.file_system.get_root_node_id(),
        _ => false,
    };

    // Skip the root
    if is_root {
        return;
    }

    let is_open = if is_root {
        true
    } else {
        match (&file_system_1_view, &file_system_2_view) {
            (Some(v1), _) => v1.collapsed.get(&path).copied().unwrap_or(false),
            (_, Some(v2)) => v2.collapsed.get(&path).copied().unwrap_or(false),
            _ => false,
        }
    };

    // --- Left Column (Folder 1) ---
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui.add_space((entry.depth as f32) * 16.0);

            if is_dir {
                let openness = if is_open { 1.0 } else { 0.0 };
                let (_rect, response) =
                    ui.allocate_exact_size(egui::vec2(12.0, row_height), egui::Sense::click());
                egui::collapsing_header::paint_default_icon(ui, openness, &response);

                if response.clicked() {
                    should_toggle = true;
                }

                if let Some(v1) = &file_system_1_view {
                    if let Some(node) = v1.file_system.get_node(path) {
                        let label_resp = ui
                            .label(format!("📁 {}", node.display_name()))
                            .interact(egui::Sense::click());

                        if label_resp.clicked() {
                            should_toggle = true;
                        }
                    }
                }
            } else if !matches!(entry.diff_state, DiffState::OnlyInSecond(..)) {
                if let Some(v1) = &file_system_1_view {
                    if let Some(node) = v1.file_system.get_node(path) {
                        if ui
                            .label(node.display_name())
                            .interact(egui::Sense::click())
                            .clicked()
                        {
                            on_row_item_clicked(
                                file_system_1_view.as_deref(),
                                file_system_2_view.as_deref(),
                                entry,
                                diff_tool_config,
                            );
                        }
                    }
                }
            }
        });
    });

    // --- Middle Column (Diff Status) ---
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui_custom_diff_state(ui, &entry.diff_state);
        });
    });

    // --- Right Column (Folder 2) ---
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui.add_space((entry.depth as f32) * 16.0);

            if is_dir {
                let openness = if is_open { 1.0 } else { 0.0 };
                let (_rect, response) =
                    ui.allocate_exact_size(egui::vec2(12.0, row_height), egui::Sense::click());
                egui::collapsing_header::paint_default_icon(ui, openness, &response);

                if response.clicked() {
                    should_toggle = true;
                }

                if let Some(v2) = &file_system_2_view {
                    if let Some(node) = v2.file_system.get_node(path) {
                        let label_resp = ui
                            .label(format!("📁 {}", node.display_name()))
                            .interact(egui::Sense::click());

                        if label_resp.clicked() {
                            should_toggle = true;
                        }
                    }
                }
            } else if !matches!(entry.diff_state, DiffState::OnlyInFirst(..)) {
                if let Some(v2) = &file_system_2_view {
                    if let Some(node) = v2.file_system.get_node(path) {
                        if ui
                            .label(node.display_name())
                            .interact(egui::Sense::click())
                            .clicked()
                        {
                            on_row_item_clicked(
                                file_system_1_view.as_deref(),
                                file_system_2_view.as_deref(),
                                entry,
                                diff_tool_config,
                            );
                        }
                    }
                }
            }
        });
    });

    if should_toggle {
        if let Some(v1) = file_system_1_view {
            v1.toggle_collapse(path);
        }
        if let Some(v2) = file_system_2_view {
            v2.toggle_collapse(path);
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
    let is_open = file_system_view
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
                let openness = if is_open { 1.0 } else { 0.0 };
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
        let full_path = node.pathbuf();
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
