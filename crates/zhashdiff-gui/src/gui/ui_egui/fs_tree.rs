use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};
use zcommon::hash::HashService;
use zhashdiff::{
    comparison::PathComparisonResult,
    external_diff_tool::{DiffToolConfig, open_diff_tool},
    filter::PathFilter,
    fs::{FileSystemModel, FsNode, FsNodeDepth, FsNodeId, TreeIter},
};

use zcommon::ui_egui::common::{
    CheckboxSelectState, draw_persistent_hint_text_edit, hash_to_color, ui_custom_checkbox,
};

use crate::ui_egui::tree_cursor::{CursorRequest, CursorRow, TreeCursor};

#[derive(Debug, Serialize, Deserialize)]
pub struct FileSystemView {
    pub file_system: Arc<FileSystemModel>,
    pub collapsed: HashMap<FsNodeId, bool>,
    pub selected: HashMap<FsNodeId, bool>,
}

#[derive(Debug, PartialEq)]
pub struct VisibleRowTwoFolderDiff {
    /// Relative to the roots, with `/` separators. Empty for the root row.
    pub rel_path: String,
    pub is_dir: bool,
    pub depth: FsNodeDepth,
    pub diff_state: DiffState,
}

/// File comparison results of the two loaded trees, so rebuilding the two-folder rows
/// doesn't read the files again.
#[derive(Debug, Default)]
pub struct FileCompareCache {
    // Node ids only mean something in the model they came from. Every reload or rescan
    // makes a new model, whichever code path does it, so the cache follows the models
    // rather than relying on each reload site to clear it. Weak, so a replaced model is
    // freed but its address can't be reused by a new one while it is held here.
    trees: (Weak<FileSystemModel>, Weak<FileSystemModel>),
    states: HashMap<(FsNodeId, FsNodeId), DiffState>,
}

impl FileCompareCache {
    fn retain_for(
        &mut self,
        file_system_1: Option<&FileSystemView>,
        file_system_2: Option<&FileSystemView>,
    ) {
        let tree = |view: Option<&FileSystemView>| {
            view.map_or_else(Weak::new, |v| Arc::downgrade(&v.file_system))
        };
        let trees = (tree(file_system_1), tree(file_system_2));
        if !Weak::ptr_eq(&trees.0, &self.trees.0) || !Weak::ptr_eq(&trees.1, &self.trees.1) {
            self.states.clear();
            self.trees = trees;
        }
    }

    fn file_diff_state(
        &mut self,
        left: (FsNodeId, &FsNode),
        right: (FsNodeId, &FsNode),
        compare: &mut impl FnMut(&Path, &Path) -> io::Result<PathComparisonResult>,
        partial_threshold: f32,
    ) -> DiffState {
        self.states
            .entry((left.0, right.0))
            .or_insert_with(|| file_diff_state(left, right, compare, partial_threshold))
            .clone()
    }
}

#[derive(Debug)]
pub struct VisibleRow {
    pub path: FsNodeId,
    pub is_dir: bool,
    pub depth: FsNodeDepth,
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

    pub fn is_collapsed(&self, node_id: FsNodeId) -> bool {
        self.collapsed.get(&node_id).copied().unwrap_or(false)
    }

    pub fn is_parent_chain_collapsed(&self, node_id: FsNodeId) -> bool {
        if let Some(parent_id) = self.file_system.get_parent_id(node_id) {
            let is_parent_collapsed = self.collapsed.get(&parent_id).copied().unwrap_or(false);
            if is_parent_collapsed {
                return true;
            }
            return self.is_parent_chain_collapsed(parent_id);
        }

        false
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
        filter: &PathFilter,
        compare_cache: &mut FileCompareCache,
        mut compare: impl FnMut(&Path, &Path) -> io::Result<PathComparisonResult>,
    ) -> io::Result<Vec<VisibleRowTwoFolderDiff>> {
        compare_cache.retain_for(file_system_1, file_system_2);

        let mut entries_map: BTreeMap<
            String,
            (
                Option<(FsNodeId, &FsNode, FsNodeDepth)>,
                Option<(FsNodeId, &FsNode, FsNodeDepth)>,
            ),
        > = BTreeMap::new();

        let get_rel_path = |view: &FileSystemView, id: FsNodeId, node: &FsNode| -> String {
            let root_id = view.file_system.get_root_node_id();
            if id == root_id {
                return String::new();
            }

            let root_node = view.file_system.get_node(root_id).unwrap();
            let root_path = root_node.as_path();

            // Use components to rebuild the path cleanly, avoiding slash/prefix issues
            node.as_path()
                .as_ref()
                .strip_prefix(&root_path)
                .unwrap_or(node.as_path().as_ref())
                .components()
                .map(|c| c.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/")
        };

        // Filtered entries never enter the map, so folder states only see visible entries.
        if let Some(view) = file_system_1 {
            let hidden = filtered_out(view, filter);
            for (id, node, depth) in view.file_system.iter_tree() {
                if hidden[id] {
                    continue;
                }
                let rel = get_rel_path(view, id, node);
                entries_map.insert(rel, (Some((id, node, depth)), None));
            }
        }
        if let Some(view) = file_system_2 {
            let hidden = filtered_out(view, filter);
            for (id, node, depth) in view.file_system.iter_tree() {
                if hidden[id] {
                    continue;
                }
                let rel = get_rel_path(view, id, node);
                entries_map
                    .entry(rel)
                    .and_modify(|e| e.1 = Some((id, node, depth)))
                    .or_insert((None, Some((id, node, depth))));
            }
        }

        let num_files_and_folders_1 = file_system_1
            .and_then(|f| Some(f.file_system.total_files_and_folders()))
            .unwrap_or(0) as usize;
        let num_files_and_folders_2 = file_system_2
            .and_then(|f| Some(f.file_system.total_files_and_folders()))
            .unwrap_or(0) as usize;
        let initial_capacity = num_files_and_folders_1.max(num_files_and_folders_2);
        let mut out_rows = Vec::with_capacity(initial_capacity);

        for (rel_path, (left, right)) in &entries_map {
            let depth = left.map(|l| l.2).or(right.map(|r| r.2)).unwrap_or(0);

            let is_dir = left
                .map(|l| l.1.is_dir())
                .or(right.map(|r| r.1.is_dir()))
                .unwrap_or(false);

            let diff_state = match (left, right) {
                (Some((l_id, l_node, _)), Some((r_id, r_node, _))) => {
                    let partial_threshold = 1.0f32;
                    if l_node.is_dir() {
                        folder_diff_state(
                            rel_path,
                            &entries_map,
                            compare_cache,
                            &mut compare,
                            partial_threshold,
                        )
                    } else {
                        compare_cache.file_diff_state(
                            (*l_id, l_node),
                            (*r_id, r_node),
                            &mut compare,
                            partial_threshold,
                        )
                    }
                }
                (Some((l_id, _, _)), None) => DiffState::OnlyInFirst(*l_id),
                (None, Some((r_id, _, _))) => DiffState::OnlyInSecond(*r_id),
                (None, None) => panic!("unreachable"),
            };

            out_rows.push(VisibleRowTwoFolderDiff {
                rel_path: rel_path.clone(),
                is_dir,
                depth,
                diff_state,
            });
        }

        Ok(out_rows)
    }

    pub fn build_collapsed_rows(
        &self,
        start_id: FsNodeId,
        start_depth: FsNodeDepth,
    ) -> Vec<VisibleRow> {
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

/// Per node id of the view's model, true when the filter hides the node: blacklisted, or inside
/// a blacklisted folder. The root is never hidden.
fn filtered_out(view: &FileSystemView, filter: &PathFilter) -> Vec<bool> {
    let model = &view.file_system;
    let mut hidden = vec![false; model.total_files_and_folders()];
    if !filter.is_active() {
        return hidden;
    }
    // Pre-order, so a parent is decided before its children.
    for (id, node, _) in model.iter_tree() {
        let Some(parent) = node.parent else {
            continue;
        };
        hidden[id] = hidden[parent] || {
            let path = node.as_path();
            let name = path.as_ref().file_name().unwrap_or_default();
            filter.is_blacklisted(&name.to_string_lossy(), node.is_dir())
        };
    }
    hidden
}

/// The two-folder rows as the cursor sees them. A row is hidden when it isn't drawn: the
/// root row, or an entry inside a folder collapsed on a side the entry exists on. A folder is
/// collapsed when it is collapsed on a side it exists on.
pub fn two_folder_cursor_rows<'a>(
    file_system_1_view: Option<&FileSystemView>,
    file_system_2_view: Option<&FileSystemView>,
    rows: &'a [VisibleRowTwoFolderDiff],
) -> Vec<CursorRow<'a>> {
    let on_either_side = |row: &VisibleRowTwoFolderDiff,
                          test: fn(&FileSystemView, FsNodeId) -> bool| {
        let test_side = |view: Option<&FileSystemView>, id: Option<FsNodeId>| match (view, id) {
            (Some(view), Some(id)) => test(view, id),
            _ => false,
        };
        test_side(file_system_1_view, row.diff_state.first())
            || test_side(file_system_2_view, row.diff_state.second())
    };
    rows.iter()
        .map(|row| CursorRow {
            rel_path: &row.rel_path,
            hidden: row.rel_path.is_empty()
                || on_either_side(row, FileSystemView::is_parent_chain_collapsed),
            is_dir: row.is_dir,
            collapsed: row.is_dir && on_either_side(row, FileSystemView::is_collapsed),
        })
        .collect()
}

pub const PENDING_DELETION_COLOR: egui::Color32 = egui::Color32::LIGHT_RED;

pub fn draw_ui_folder_tree_with_checkbox(
    ui: &mut egui::Ui,
    file_system_view: &mut FileSystemView,
    hash_service: &mut HashService,
    pending_deletion: &HashSet<PathBuf>,
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
                                pending_deletion,
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
    visible_rows: &mut Option<Vec<VisibleRowTwoFolderDiff>>,
    open_dir_window_1: &mut bool,
    open_dir_window_2: &mut bool,
    diff_tool_config: &DiffToolConfig,
    cursor: &mut TreeCursor,
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

    // Scroll only when the keys moved the cursor, so manual scrolling isn't fought.
    let mut scroll_to_cursor = false;
    if !ui.ctx().wants_keyboard_input() {
        for key in ui.input(cursor_keys) {
            // Each key sees the rows the previous key's fold left.
            let cursor_rows = two_folder_cursor_rows(
                file_system_1_view.as_ref(),
                file_system_2_view.as_ref(),
                visible_rows,
            );
            let (moved, request) = match key {
                egui::Key::ArrowDown => (cursor.down(&cursor_rows), None),
                egui::Key::ArrowUp => (cursor.up(&cursor_rows), None),
                egui::Key::ArrowLeft => cursor.left(&cursor_rows),
                egui::Key::ArrowRight => (cursor.clone(), cursor.right(&cursor_rows)),
                egui::Key::Enter => (cursor.clone(), cursor.enter(&cursor_rows)),
                _ => unreachable!("cursor_keys returned {key:?}"),
            };
            scroll_to_cursor |= moved != *cursor;
            *cursor = moved;
            if let Some(request) = request {
                apply_cursor_request(
                    file_system_1_view.as_mut(),
                    file_system_2_view.as_mut(),
                    visible_rows,
                    &request,
                    diff_tool_config,
                );
            }
        }
    }

    // After the keys, so a fold they made is drawn this frame.
    let cursor_rows = two_folder_cursor_rows(
        file_system_1_view.as_ref(),
        file_system_2_view.as_ref(),
        visible_rows,
    );

    // Only drawn rows go to the table: it places row i at i * row height, both when
    // virtualizing and when scrolling to a row.
    let drawn_rows: Vec<usize> = (0..cursor_rows.len())
        .filter(|&i| !cursor_rows[i].hidden)
        .collect();
    let scroll_to_row = if scroll_to_cursor {
        drawn_rows
            .iter()
            .position(|&i| cursor.is_at(cursor_rows[i].rel_path))
    } else {
        None
    };

    let row_count = drawn_rows.len();
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

            let mut table = TableBuilder::new(ui)
                .sense(egui::Sense::all())
                .id_salt("two_folder_diff_table")
                .striped(true)
                .resizable(false)
                .auto_shrink([false, true]);
            if let Some(row) = scroll_to_row {
                table = table.scroll_to_row(row, None);
            }
            table
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
                                if let Some(row) = visible_rows
                                    .iter()
                                    .find(|r| r.diff_state.first() == Some(root_1_id))
                                {
                                    ui_custom_diff_state(ui, &row.diff_state);
                                }
                            } else if let Some(fs2_view) = file_system_2_view {
                                let root_2_id = fs2_view.file_system.get_root_node_id();
                                if let Some(row) = visible_rows
                                    .iter()
                                    .find(|r| r.diff_state.second() == Some(root_2_id))
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
                        let entry = &visible_rows[drawn_rows[row.index()]];
                        row.set_selected(cursor.is_at(&entry.rel_path));
                        let clicked = render_row_folder_tree_diff_column(
                            file_system_1_view.as_mut(),
                            file_system_2_view.as_mut(),
                            &mut row,
                            entry,
                            row_height,
                            diff_tool_config,
                        );
                        if clicked {
                            *cursor = TreeCursor::at(&entry.rel_path);
                        }
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
                        match FileSystemModel::new(&path) {
                            Ok(new_model) => {
                                *fs1_view = Some(FileSystemView::new(Arc::new(new_model)))
                            }
                            Err(e) => log::error!("{e}"),
                        }
                        return true;
                    } else if rect2.contains(drop_pos) {
                        match FileSystemModel::new(&path) {
                            Ok(new_model) => {
                                *fs2_view = Some(FileSystemView::new(Arc::new(new_model)))
                            }
                            Err(e) => log::error!("{e}"),
                        }
                        return true;
                    }
                    break;
                }
            }
        }
        return false;
    });
}

#[derive(PartialEq, Debug, Clone)]
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
    compare: &mut impl FnMut(&Path, &Path) -> io::Result<PathComparisonResult>,
    partial_threshold: f32,
) -> DiffState {
    let path1 = left.1.as_path();
    let path2 = right.1.as_path();

    let result = match compare(path1.as_ref(), path2.as_ref()) {
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
    entries_map: &BTreeMap<
        String,
        (
            Option<(FsNodeId, &FsNode, FsNodeDepth)>,
            Option<(FsNodeId, &FsNode, FsNodeDepth)>,
        ),
    >,
    compare_cache: &mut FileCompareCache,
    compare: &mut impl FnMut(&Path, &Path) -> io::Result<PathComparisonResult>,
    threshold: f32,
) -> DiffState {
    let (current_left, current_right) = entries_map
        .get(parent_path)
        .cloned()
        .unwrap_or((None, None));
    let l_id = current_left.map(|l| l.0).unwrap_or(0);
    let r_id = current_right.map(|r| r.0).unwrap_or(0);

    let prefix = if parent_path.is_empty() {
        String::new()
    } else {
        format!("{}/", parent_path)
    };

    for (path, (left, right)) in entries_map.range(prefix.clone()..) {
        if !path.starts_with(&prefix) {
            break;
        }
        if path == parent_path {
            continue;
        }

        match (left, right) {
            // If a child exists on one side only, the parent is "Different" (Modified)
            (Some(_), None) | (None, Some(_)) => return DiffState::Different(l_id, r_id),
            (Some((li, ln, _)), Some((ri, rn, _))) => {
                if !ln.is_dir() {
                    let s = compare_cache.file_diff_state((*li, ln), (*ri, rn), compare, threshold);
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
    mut on_select: impl FnMut(),
    mut on_open: impl FnMut(),
    mut on_toggle: impl FnMut(),
) {
    ui.horizontal(|ui| {
        ui.add_space((depth as f32) * 16.0);

        if let (Some(v), Some(id)) = (view, node_id) {
            if let Some(node) = v.file_system.get_node(id) {
                if is_dir {
                    // Openness: 0.0 is closed, 1.0 is open
                    let openness = if is_collapsed { 0.0 } else { 1.0 };
                    let (_rect, response) =
                        ui.allocate_exact_size(egui::vec2(12.0, row_height), egui::Sense::click());
                    egui::collapsing_header::paint_default_icon(ui, openness, &response);

                    if response.clicked() {
                        on_toggle();
                    }

                    let label_resp = ui
                        .label(format!("📁 {}", node.display_name()))
                        .interact(egui::Sense::click());

                    if label_resp.clicked() {
                        on_select();
                    }
                    if label_resp.double_clicked() {
                        on_toggle();
                    }
                } else {
                    let label_resp = ui.label(node.display_name()).interact(egui::Sense::click());

                    if label_resp.clicked() {
                        on_select();
                    }
                    if label_resp.double_clicked() {
                        on_open();
                    }
                }
            }
        }
    });
}

/// Draws a row that `two_folder_cursor_rows` doesn't hide. Returns whether it was clicked,
/// which moves the cursor to it.
fn render_row_folder_tree_diff_column(
    mut file_system_1_view: Option<&mut FileSystemView>,
    mut file_system_2_view: Option<&mut FileSystemView>,
    row: &mut egui_extras::TableRow,
    entry: &VisibleRowTwoFolderDiff,
    row_height: f32,
    diff_tool_config: &DiffToolConfig,
) -> bool {
    let first_node_id = entry.diff_state.first();
    let second_node_id = entry.diff_state.second();
    let mut should_toggle_row = false;
    let mut should_select_row = false;

    let is_collapsed = |view: Option<&FileSystemView>, id: Option<FsNodeId>| match (view, id) {
        (Some(view), Some(id)) => view.is_collapsed(id),
        _ => false,
    };
    let is_collapsed_1 = is_collapsed(file_system_1_view.as_deref(), first_node_id);
    let is_collapsed_2 = is_collapsed(file_system_2_view.as_deref(), second_node_id);

    // --- Left Column (Folder 1) ---
    row.col(|ui| {
        render_diff_side(
            ui,
            file_system_1_view.as_deref(),
            first_node_id,
            entry.depth,
            entry.is_dir,
            is_collapsed_1,
            row_height,
            || should_select_row = true,
            || {
                on_row_item_clicked(
                    file_system_1_view.as_deref(),
                    file_system_2_view.as_deref(),
                    entry,
                    diff_tool_config,
                );
            },
            || should_toggle_row = true,
        );
    });
    // --- Middle Column (Diff Status) ---
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui_custom_diff_state(ui, &entry.diff_state);
        });
    });
    // --- Right Column (Folder 2) ---
    row.col(|ui| {
        render_diff_side(
            ui,
            file_system_2_view.as_deref(),
            second_node_id,
            entry.depth,
            entry.is_dir,
            is_collapsed_2,
            row_height,
            || should_select_row = true,
            || {
                on_row_item_clicked(
                    file_system_1_view.as_deref(),
                    file_system_2_view.as_deref(),
                    entry,
                    diff_tool_config,
                );
            },
            || should_toggle_row = true,
        );
    });

    if should_toggle_row {
        if let Some(first) = &entry.diff_state.first() {
            if let Some(view) = file_system_1_view.as_mut() {
                view.toggle_collapse(*first);
            }
        }
        if let Some(second) = &entry.diff_state.second() {
            if let Some(view) = file_system_2_view.as_mut() {
                view.toggle_collapse(*second);
            }
        }
    }

    // Labels take their own clicks; this catches clicks elsewhere on the row.
    should_select_row || row.response().clicked()
}

fn apply_cursor_request(
    file_system_1_view: Option<&mut FileSystemView>,
    file_system_2_view: Option<&mut FileSystemView>,
    rows: &[VisibleRowTwoFolderDiff],
    request: &CursorRequest,
    diff_tool_config: &DiffToolConfig,
) {
    let Some(entry) = rows.iter().find(|row| row.rel_path == request.rel_path()) else {
        log::error!("cursor request for a row that isn't there: {request:?}");
        return;
    };
    match request {
        CursorRequest::Open(_) => {
            on_row_item_clicked(
                file_system_1_view.as_deref(),
                file_system_2_view.as_deref(),
                entry,
                diff_tool_config,
            );
        }
        // Set rather than toggled per side, so the sides end up agreeing.
        CursorRequest::Expand(_) | CursorRequest::Collapse(_) => {
            let collapse = matches!(request, CursorRequest::Collapse(_));
            let sides = [
                (file_system_1_view, entry.diff_state.first()),
                (file_system_2_view, entry.diff_state.second()),
            ];
            for (view, id) in sides {
                if let (Some(view), Some(id)) = (view, id) {
                    if collapse {
                        view.collapsed.insert(id, true);
                    } else {
                        view.collapsed.remove(&id);
                    }
                }
            }
        }
    }
}

/// The arrows and Enter without modifiers, in press order, key repeats included. Modified
/// arrows are left to other bindings.
fn cursor_keys(input: &egui::InputState) -> Vec<egui::Key> {
    if !input.modifiers.is_none() {
        return Vec::new();
    }
    input
        .events
        .iter()
        .filter_map(|event| match event {
            egui::Event::Key {
                key:
                    key @ (egui::Key::ArrowDown
                    | egui::Key::ArrowUp
                    | egui::Key::ArrowLeft
                    | egui::Key::ArrowRight
                    | egui::Key::Enter),
                pressed: true,
                ..
            } => Some(*key),
            _ => None,
        })
        .collect()
}

fn render_row_folder_tree_with_checkbox(
    hash_service: &mut HashService,
    file_system_view: &mut FileSystemView,
    pending_deletion: &HashSet<PathBuf>,
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
            } else if pending_deletion.contains(node.as_path().as_ref()) {
                ui.label(RichText::new(node.display_name()).color(PENDING_DELETION_COLOR));
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

#[cfg(test)]
mod tests {
    use crate::ui_egui::fs_tree::{
        DiffState, FileCompareCache, FileSystemView, VisibleRowTwoFolderDiff, apply_cursor_request,
        two_folder_cursor_rows,
    };
    use crate::ui_egui::tree_cursor::CursorRequest;
    use zhashdiff::external_diff_tool::DiffToolConfig;

    use std::cell::Cell;
    use std::collections::HashMap;
    use std::fs::{self, File};
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::{TempDir, tempdir};
    use zhashdiff::comparison::compare_crc;
    use zhashdiff::filter::{PathFilter, PatternList};
    use zhashdiff::fs::{FileSystemModel, FsIsDir, FsNodeDepth, FsNodeId};

    struct CollapsedTestCase {
        name: &'static str,
        structure: Vec<(&'static str, FsIsDir)>,
        collapsed: Vec<&'static str>,
        expected: Vec<(&'static str, FsNodeDepth)>,
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    #[allow(dead_code)]
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

    fn write_files(root: &Path, files: &[(&str, &str)]) {
        for (rel_path, content) in files {
            let path = root.join(rel_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, content).unwrap();
        }
    }

    fn load_view(root: &Path) -> FileSystemView {
        FileSystemView::new(Arc::new(FileSystemModel::new(root).unwrap()))
    }

    /// b.txt differs, a.txt and everything under sub/ is equal, and each side has a file
    /// of its own.
    fn two_folder_trees() -> (TempDir, TempDir) {
        let left = tempdir().unwrap();
        let right = tempdir().unwrap();
        let shared = [
            ("a.txt", "same a"),
            ("sub/c.txt", "same c"),
            ("sub/deep/d.txt", "same d"),
        ];
        write_files(left.path(), &shared);
        write_files(right.path(), &shared);
        write_files(left.path(), &[("b.txt", "left b"), ("left_only.txt", "l")]);
        write_files(
            right.path(),
            &[("b.txt", "right b"), ("right_only.txt", "r")],
        );
        (left, right)
    }

    fn state_kind(state: &DiffState) -> &'static str {
        match state {
            DiffState::Different(..) => "Different",
            DiffState::Same(..) => "Same",
            DiffState::Partial(..) => "Partial",
            DiffState::OnlyInFirst(..) => "OnlyInFirst",
            DiffState::OnlyInSecond(..) => "OnlyInSecond",
        }
    }

    #[test]
    fn two_folder_rows_have_the_expected_paths_states_and_order() {
        let (left_dir, right_dir) = two_folder_trees();
        let left = load_view(left_dir.path());
        let right = load_view(right_dir.path());

        let rows = build_counting(
            &left,
            &right,
            &mut FileCompareCache::default(),
            &Cell::new(0),
        );

        let actual: Vec<(String, &str, FsNodeDepth, bool)> = rows
            .iter()
            .map(|row| {
                let rel = get_rel_from_diff_state(
                    &left.file_system,
                    &right.file_system,
                    left_dir.path(),
                    right_dir.path(),
                    &row.diff_state,
                );
                (rel, state_kind(&row.diff_state), row.depth, row.is_dir)
            })
            .collect();
        let expected = [
            ("", "Different", 0, true),
            ("a.txt", "Same", 1, false),
            ("b.txt", "Different", 1, false),
            ("left_only.txt", "OnlyInFirst", 1, false),
            ("right_only.txt", "OnlyInSecond", 1, false),
            ("sub", "Same", 1, true),
            ("sub/c.txt", "Same", 2, false),
            ("sub/deep", "Same", 2, true),
            ("sub/deep/d.txt", "Same", 3, false),
        ]
        .map(|(rel, kind, depth, is_dir)| (rel.to_string(), kind, depth, is_dir));
        assert_eq!(actual, expected);
    }

    fn build_counting(
        left: &FileSystemView,
        right: &FileSystemView,
        cache: &mut FileCompareCache,
        comparisons: &Cell<usize>,
    ) -> Vec<VisibleRowTwoFolderDiff> {
        build_filtered(left, right, cache, comparisons, &PathFilter::default())
    }

    fn build_filtered(
        left: &FileSystemView,
        right: &FileSystemView,
        cache: &mut FileCompareCache,
        comparisons: &Cell<usize>,
        filter: &PathFilter,
    ) -> Vec<VisibleRowTwoFolderDiff> {
        FileSystemView::build_two_folder_diff_rows(
            Some(left),
            Some(right),
            filter,
            cache,
            |a, b| {
                comparisons.set(comparisons.get() + 1);
                compare_crc(a, b)
            },
        )
        .unwrap()
    }

    fn blacklist(text: &str) -> PathFilter {
        PathFilter {
            blacklist: PatternList::new(text),
            ..Default::default()
        }
    }

    fn rows_with_kinds(rows: &[VisibleRowTwoFolderDiff]) -> Vec<(&str, &'static str)> {
        rows.iter()
            .map(|r| (r.rel_path.as_str(), state_kind(&r.diff_state)))
            .collect()
    }

    #[test]
    fn blacklisted_entries_are_absent_on_both_sides() {
        let (left_dir, right_dir) = two_folder_trees();
        let left = load_view(left_dir.path());
        let right = load_view(right_dir.path());

        let rows = build_filtered(
            &left,
            &right,
            &mut FileCompareCache::default(),
            &Cell::new(0),
            &blacklist("B.TXT, *_only.txt"),
        );

        // b.txt is on both sides, left_only.txt and right_only.txt on one. With every
        // difference hidden, the root reads as Same.
        assert_eq!(
            rows_with_kinds(&rows),
            [
                ("", "Same"),
                ("a.txt", "Same"),
                ("sub", "Same"),
                ("sub/c.txt", "Same"),
                ("sub/deep", "Same"),
                ("sub/deep/d.txt", "Same"),
            ]
        );
    }

    #[test]
    fn a_blacklisted_folder_hides_its_whole_subtree() {
        let (left_dir, right_dir) = two_folder_trees();
        write_files(left_dir.path(), &[("sub/deep/more/e.txt", "l")]);
        // A file of the same name: the trailing slash spares it.
        write_files(right_dir.path(), &[("deep", "a file")]);
        let left = load_view(left_dir.path());
        let right = load_view(right_dir.path());

        let rows = build_filtered(
            &left,
            &right,
            &mut FileCompareCache::default(),
            &Cell::new(0),
            &blacklist("deep/"),
        );

        let paths: Vec<&str> = rows.iter().map(|r| r.rel_path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "",
                "a.txt",
                "b.txt",
                "deep",
                "left_only.txt",
                "right_only.txt",
                "sub",
                "sub/c.txt",
            ]
        );
    }

    #[test]
    fn a_folder_whose_only_difference_is_blacklisted_reads_as_same() {
        let (left_dir, right_dir) = two_folder_trees();
        write_files(left_dir.path(), &[("sub/build.log", "left")]);
        write_files(right_dir.path(), &[("sub/build.log", "right")]);
        write_files(left_dir.path(), &[("sub/deep/left.obj", "only left")]);
        let left = load_view(left_dir.path());
        let right = load_view(right_dir.path());
        let mut cache = FileCompareCache::default();
        let comparisons = Cell::new(0);

        let rows = build_counting(&left, &right, &mut cache, &comparisons);
        assert_eq!(kind_at(&rows, "sub"), "Different");
        assert_eq!(kind_at(&rows, "sub/deep"), "Different");

        let rows = build_filtered(
            &left,
            &right,
            &mut cache,
            &comparisons,
            &blacklist("*.log\n*.obj"),
        );
        assert_eq!(kind_at(&rows, "sub"), "Same");
        assert_eq!(kind_at(&rows, "sub/deep"), "Same");
        assert_eq!(kind_at(&rows, ""), "Different", "b.txt still differs");
    }

    #[test]
    fn the_root_is_never_filtered() {
        let (left_dir, right_dir) = two_folder_trees();
        let left = load_view(left_dir.path());
        let right = load_view(right_dir.path());

        // Matches the roots' own names too (tempdirs are named .tmpXXXX).
        let rows = build_filtered(
            &left,
            &right,
            &mut FileCompareCache::default(),
            &Cell::new(0),
            &blacklist("*, *.tmp*/"),
        );

        assert_eq!(rows_with_kinds(&rows), [("", "Same")]);
    }

    #[test]
    fn changing_the_filter_rebuilds_without_comparing_files_again() {
        let (left_dir, right_dir) = two_folder_trees();
        let left = load_view(left_dir.path());
        let right = load_view(right_dir.path());
        let comparisons = Cell::new(0);
        let mut cache = FileCompareCache::default();

        let unfiltered = build_counting(&left, &right, &mut cache, &comparisons);
        assert_eq!(comparisons.get(), 4);

        let filtered = build_filtered(&left, &right, &mut cache, &comparisons, &blacklist("sub"));
        assert!(filtered.iter().all(|r| !r.rel_path.starts_with("sub")));
        let cleared = build_counting(&left, &right, &mut cache, &comparisons);
        assert_eq!(comparisons.get(), 4);
        assert_eq!(cleared, unfiltered);
    }

    fn kind_at(rows: &[VisibleRowTwoFolderDiff], rel_path: &str) -> &'static str {
        let row = rows.iter().find(|r| r.rel_path == rel_path).unwrap();
        state_kind(&row.diff_state)
    }

    #[test]
    fn two_folder_rows_carry_their_relative_path_with_slash_separators() {
        let (left_dir, right_dir) = two_folder_trees();
        let left = load_view(left_dir.path());
        let right = load_view(right_dir.path());

        let rows = build_counting(
            &left,
            &right,
            &mut FileCompareCache::default(),
            &Cell::new(0),
        );

        let paths: Vec<&str> = rows.iter().map(|r| r.rel_path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "",
                "a.txt",
                "b.txt",
                "left_only.txt",
                "right_only.txt",
                "sub",
                "sub/c.txt",
                "sub/deep",
                "sub/deep/d.txt",
            ]
        );
    }

    #[test]
    fn rebuilding_two_folder_rows_compares_no_files_and_builds_identical_rows() {
        let (left_dir, right_dir) = two_folder_trees();
        let left = load_view(left_dir.path());
        let right = load_view(right_dir.path());
        let comparisons = Cell::new(0);
        let mut cache = FileCompareCache::default();

        let first = build_counting(&left, &right, &mut cache, &comparisons);
        // a.txt, b.txt, sub/c.txt and sub/deep/d.txt are on both sides.
        assert_eq!(comparisons.get(), 4);

        let second = build_counting(&left, &right, &mut cache, &comparisons);
        assert_eq!(comparisons.get(), 4);
        assert_eq!(second, first);
    }

    #[test]
    fn reloading_either_root_recompares_its_files() {
        let (left_dir, right_dir) = two_folder_trees();
        let mut left = load_view(left_dir.path());
        let mut right = load_view(right_dir.path());
        let comparisons = Cell::new(0);
        let mut cache = FileCompareCache::default();
        let rows = build_counting(&left, &right, &mut cache, &comparisons);
        assert_eq!(kind_at(&rows, "b.txt"), "Different");

        fs::write(right_dir.path().join("b.txt"), "left b").unwrap();
        let rows = build_counting(&left, &right, &mut cache, &comparisons);
        assert_eq!(
            comparisons.get(),
            4,
            "the same trees are not compared again"
        );
        assert_eq!(kind_at(&rows, "b.txt"), "Different");

        right = load_view(right_dir.path());
        let rows = build_counting(&left, &right, &mut cache, &comparisons);
        assert_eq!(comparisons.get(), 8);
        assert_eq!(kind_at(&rows, "b.txt"), "Same");

        fs::write(left_dir.path().join("b.txt"), "changed b").unwrap();
        left = load_view(left_dir.path());
        let rows = build_counting(&left, &right, &mut cache, &comparisons);
        assert_eq!(comparisons.get(), 12);
        assert_eq!(kind_at(&rows, "b.txt"), "Different");
    }

    #[test]
    fn rows_inside_a_collapsed_folder_and_the_root_row_are_hidden_from_the_cursor() {
        let (left_dir, right_dir) = two_folder_trees();
        write_files(left_dir.path(), &[("sub/left_only_in_sub.txt", "l")]);
        let mut left = load_view(left_dir.path());
        let mut right = load_view(right_dir.path());
        let rows = build_counting(
            &left,
            &right,
            &mut FileCompareCache::default(),
            &Cell::new(0),
        );

        // As a click on the row does: both sides.
        let sub = rows.iter().find(|r| r.rel_path == "sub").unwrap();
        left.toggle_collapse(sub.diff_state.first().unwrap());
        right.toggle_collapse(sub.diff_state.second().unwrap());

        let hidden: Vec<&str> = two_folder_cursor_rows(Some(&left), Some(&right), &rows)
            .iter()
            .filter(|r| r.hidden)
            .map(|r| r.rel_path)
            .collect();
        // sub/deep is expanded, but its parent isn't.
        assert_eq!(
            hidden,
            [
                "",
                "sub/c.txt",
                "sub/deep",
                "sub/deep/d.txt",
                "sub/left_only_in_sub.txt",
            ]
        );
    }

    #[test]
    fn expand_and_collapse_requests_set_both_sides_even_when_they_disagree() {
        let (left_dir, right_dir) = two_folder_trees();
        let mut left = load_view(left_dir.path());
        let mut right = load_view(right_dir.path());
        let rows = build_counting(
            &left,
            &right,
            &mut FileCompareCache::default(),
            &Cell::new(0),
        );
        let sub = rows.iter().find(|r| r.rel_path == "sub").unwrap();
        let (left_sub, right_sub) = (
            sub.diff_state.first().unwrap(),
            sub.diff_state.second().unwrap(),
        );
        let cursor_sees_collapsed = |left: &FileSystemView, right: &FileSystemView| {
            two_folder_cursor_rows(Some(left), Some(right), &rows)
                .iter()
                .find(|r| r.rel_path == "sub")
                .unwrap()
                .collapsed
        };
        let config = DiffToolConfig::default();

        left.toggle_collapse(left_sub);
        assert!(cursor_sees_collapsed(&left, &right), "left side only");

        let expand = CursorRequest::Expand("sub".to_string());
        apply_cursor_request(Some(&mut left), Some(&mut right), &rows, &expand, &config);
        assert!(!left.is_collapsed(left_sub) && !right.is_collapsed(right_sub));
        assert!(!cursor_sees_collapsed(&left, &right));

        right.toggle_collapse(right_sub);
        assert!(cursor_sees_collapsed(&left, &right), "right side only");

        let collapse = CursorRequest::Collapse("sub".to_string());
        apply_cursor_request(Some(&mut left), Some(&mut right), &rows, &collapse, &config);
        assert!(left.is_collapsed(left_sub) && right.is_collapsed(right_sub));
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
}
