use std::{
    collections::{BTreeMap, HashMap},
    io,
    path::{Path, PathBuf},
};

use eframe::egui::{self, Pos2, Rect, RichText, ScrollArea, Sense, Widget};
use egui_extras::{Column, TableBuilder};
use zhashdiff::{
    comparison::{PathComparissonMethod, compare_paths},
    external_diff_tool::{DiffToolConfig, open_diff_tool},
    fs::{FileSystemModel, FsNode, FsNodeId, FsNodeKind},
    hash::HashService,
};

use crate::ui_egui::common::{
    CheckboxSelectState, hash_to_color, preview_files_being_dropped, ui_custom_checkbox,
};

pub fn draw_ui_folder_tree_with_checkbox(
    ui: &mut egui::Ui,
    expanded: &mut HashMap<FsNodeId, bool>,
    selected: &mut HashMap<FsNodeId, bool>,
    file_system: &mut FileSystemModel,
    hash_service: &mut HashService,
) -> egui::response::Response {
    let root = file_system.get_root().clone();
    let mut visible_rows = Vec::new();
    build_expanded_rows(
        expanded,
        file_system,
        file_system.get_root_node_id(),
        0,
        &mut visible_rows,
    );
    let row_count = visible_rows.len();

    let available_height = ui.available_height();

    let response = egui::Frame::new()
        .fill(egui::Color32::from_gray(20))
        .inner_margin(0.0)
        .show(ui, |ui| {
            ui.set_min_height(available_height);
            let row_height = ui.text_style_height(&egui::TextStyle::Body);
            let row_height_header = ui.text_style_height(&egui::TextStyle::Heading);

            // Calculate exact width for a 64-char monospace hash + padding
            let font_id = egui::TextStyle::Monospace.resolve(ui.style());
            const DUMMY_HASH: &str =
                "321e84925aecc55ef828a41db03f0ccece66c7a6cd2a31975bcc5d029712db81";
            let galley =
                ui.painter()
                    .layout_no_wrap(DUMMY_HASH.into(), font_id, egui::Color32::PLACEHOLDER);
            let min_hash_width = galley.size().x + 20.0;

            TableBuilder::new(ui)
                .id_salt(root.pathbuf().as_ref())
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
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.centered_and_justified(|ui| {
                                let state = get_folder_selection_state(
                                    file_system.get_root_node_id(),
                                    file_system,
                                    selected,
                                );
                                folder_state_ui_custom_checkbox(
                                    ui,
                                    file_system,
                                    selected,
                                    state,
                                    Some(file_system.get_root_node_id()),
                                );
                            });
                        });
                    });
                    header.col(|ui| {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            ui.label("Name");
                        });
                    });
                    header.col(|ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label("Hash");
                        });
                    });
                })
                .body(|body| {
                    body.rows(row_height, row_count, |mut row| {
                        let entry = &visible_rows[row.index()];
                        render_row_folder_tree_with_checkbox(
                            hash_service,
                            file_system,
                            expanded,
                            selected,
                            &mut row,
                            entry,
                            row_height,
                        );
                    });
                });
        });

    response.response
}

pub fn draw_ui_two_folder_tree_with_diff(
    ui: &mut egui::Ui,
    expanded: &mut HashMap<FsNodeId, bool>,
    selected: &mut HashMap<FsNodeId, bool>,
    file_system_1: &mut FileSystemModel,
    file_system_2: &mut FileSystemModel,
    open_dir_window_1: &mut bool,
    open_dir_window_2: &mut bool,
    diff_tool_config: &DiffToolConfig,
) -> egui::response::Response {
    let root_1 = file_system_1.get_root().clone();
    let root_2 = file_system_1.get_root().clone();

    let mut visible_rows = Vec::new();
    let io_read_result = build_two_folder_diff_rows(
        expanded,
        file_system_1,
        file_system_2,
        &mut visible_rows,
        &PathComparissonMethod::CrC,
    );
    if let Err(err) = io_read_result {
        log::error!("{:?}", err);
    }
    let row_count = visible_rows.len();

    let available_height = ui.available_height();
    let available_width = ui.available_width();

    let mut col0_rect = egui::Rect::NOTHING; // File System 1
    let mut col2_rect = egui::Rect::NOTHING; // Fily System 2
    let table_top = ui.cursor().top();
    let response = egui::Frame::default()
        .fill(egui::Color32::from_gray(20))
        .inner_margin(0.0)
        .show(ui, |ui| {
            ui.set_min_height(available_height);
            ui.set_min_width(available_width);

            let row_height = ui.text_style_height(&egui::TextStyle::Body);
            let row_height_header = ui.text_style_height(&egui::TextStyle::Heading);

            let available_size = ui.available_size();
            let scroll_area_output = TableBuilder::new(ui)
                .sense(egui::Sense::all())
                .id_salt(root_1.pathbuf().as_ref())
                .striped(true)
                .resizable(false)
                .auto_shrink([false, true])
                .column(
                    Column::initial(available_size.x * 0.5)
                        .at_least(100.0)
                        .resizable(true),
                )
                .column(Column::auto().resizable(true).auto_size_this_frame(true))
                .column(
                    Column::remainder().resizable(true).clip(false), // Clip Bugged
                )
                .header(row_height_header, |mut header| {
                    // COLUMN 0 (Folder 1)
                    header.col(|ui| {
                        col0_rect = ui.max_rect();
                        let available = ui.available_width();

                        ui.vertical(|ui| {
                            if ui
                                .add_sized(
                                    [available, row_height_header],
                                    egui::Button::new("Open Folder 1"),
                                )
                                .interact(egui::Sense::HOVER)
                                .clicked()
                            {
                                *open_dir_window_1 = true;
                            }

                            let mut text = root_1.pathbuf().as_ref().to_string_lossy().to_string();
                            ui.add_sized(
                                [available, row_height_header],
                                egui::TextEdit::singleline(&mut text),
                            );

                            ui.spacing();
                        });
                    });

                    // COLUMN 1 (Diff icon)
                    header.col(|ui| {
                        ui.vertical(|ui| {
                            ui.label("≠");

                            // Find the root folder row in visible_rows
                            if let Some(root_row) = visible_rows
                                .iter()
                                .find(|row| file_system_1.get_node(row.path).unwrap() == &root_1)
                            {
                                // Draw the diff icon for this row
                                ui_custom_diff_state(ui, &root_row.diff_state);
                            }
                        });
                    });

                    // COLUMN 2 (Folder 2)
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

                            let mut text = root_2.pathbuf().as_ref().to_string_lossy().to_string();
                            ui.add_sized(
                                [available, row_height_header],
                                egui::TextEdit::singleline(&mut text).clip_text(true),
                            );

                            ui.spacing();
                        });
                    });
                })
                .body(|body| {
                    body.rows(row_height, row_count, |mut row| {
                        let entry = &visible_rows[row.index()];
                        render_row_folder_tree_diff_column(
                            file_system_1,
                            file_system_2,
                            expanded,
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

    // log::error!("ASD: {:?}", response.response.interact_pointer_pos());
    preview_files_being_dropped(ui.ctx());
    // preview_files_being_dropped_in_rect(&ui.ctx(), col0_rect, "Folder 1");
    // preview_files_being_dropped_in_rect(&ui.ctx(), col2_rect, "Folder 2");
    ui.input(|i| {
        if !i.raw.dropped_files.is_empty() {
            // Get the drop position (where the mouse was)
            if let Some(drop_pos) = i.pointer.hover_pos() {
                for dropped_file in &i.raw.dropped_files {
                    if let Some(path) = &dropped_file.path {
                        if col0_rect.contains(drop_pos) {
                            log::info!("File dropped in Column 1: {:?}", path);
                            *file_system_1 = FileSystemModel::new(&path);
                            expanded.clear();
                        } else if col2_rect.contains(drop_pos) {
                            log::info!("File dropped in Column 2: {:?}", path);
                            *file_system_2 = FileSystemModel::new(&path);
                            expanded.clear();
                        }
                    }
                }
            }
        }
    });
    response.response
}

struct VisibleRow {
    path: FsNodeId,
    is_dir: bool,
    depth: usize,
}

#[derive(PartialEq)]
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
            DiffState::Same(path_buf, path_buf1)
            | DiffState::Different(path_buf, path_buf1)
            | DiffState::Partial(path_buf, path_buf1) => Some(*path_buf),
            DiffState::OnlyInFirst(path_buf) => Some(*path_buf),
            DiffState::OnlyInSecond(..) => None,
        }
    }
    pub fn second(&self) -> Option<FsNodeId> {
        match &self {
            DiffState::Same(path_buf, path_buf1)
            | DiffState::Different(path_buf, path_buf1)
            | DiffState::Partial(path_buf, path_buf1) => Some(*path_buf1),
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

struct VisibleRowTwoFolderDiff {
    path: FsNodeId,
    is_dir: bool,
    depth: usize,
    diff_state: DiffState,
}

pub fn build_expanded_rows(
    expanded: &HashMap<FsNodeId, bool>,
    file_system: &FileSystemModel,
    node_id: FsNodeId,
    depth: usize,
    out: &mut Vec<VisibleRow>,
) {
    let node = file_system.get_node(node_id).unwrap();

    // Only directories have children
    if let FsNodeKind::Dir { path, children } = &node.kind {
        for &child_id in children {
            let child_node = file_system.get_node(child_id).unwrap();

            // Determine path and type
            let (path, is_dir) = match &child_node.kind {
                FsNodeKind::Dir { path, .. } => (path, true),
                FsNodeKind::File { path } => (path, false),
            };

            // Add to visible rows
            out.push(VisibleRow {
                path: child_id,
                is_dir,
                depth,
            });

            // Recurse if expanded and directory
            if is_dir && expanded.get(&child_id).copied().unwrap_or(false) {
                build_expanded_rows(expanded, file_system, child_id, depth + 1, out);
            }
        }
    }
}

// Check if a given relative path is visible based on expanded folders
pub fn is_visible(
    file_system: &FileSystemModel,
    expanded: &HashMap<FsNodeId, bool>,
    node_id: FsNodeId,
) -> bool {
    let mut current_id = Some(node_id);

    while let Some(id) = current_id {
        let node = file_system.get_node(id).unwrap();

        // Skip root — it's always visible
        if id != 0 {
            if let FsNodeKind::Dir { path, .. } = &node.kind {
                if let Some(&is_expanded) = expanded.get(&id) {
                    if !is_expanded {
                        return false;
                    }
                }
            }
        }

        current_id = node.parent;
    }

    true
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
            Option<(FsNodeId, &FsNode, usize)>,
            Option<(FsNodeId, &FsNode, usize)>,
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
    for (&child_id, (left, right)) in entries_map {
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
        // leaf folder itself
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

pub fn build_two_folder_diff_rows(
    expanded: &mut HashMap<FsNodeId, bool>,
    file_system_1: &FileSystemModel,
    file_system_2: &FileSystemModel,
    out: &mut Vec<VisibleRowTwoFolderDiff>,
    method: &PathComparissonMethod,
) -> io::Result<()> {
    let partial_threshold = 1.0f32;
    out.clear();

    let root1 = file_system_1.get_root();
    let root2 = file_system_2.get_root();

    // Iterate both trees: returns (FsNodeId, depth)
    let fs_tree_1 = file_system_1.iter_tree();
    let fs_tree_2 = file_system_2.iter_tree();

    // Key by FsNodeId
    let mut entries_map: BTreeMap<
        FsNodeId,
        (
            Option<(FsNodeId, &FsNode, usize)>,
            Option<(FsNodeId, &FsNode, usize)>,
        ),
    > = BTreeMap::new();

    // Populate folder 1
    for (node_id, depth) in fs_tree_1 {
        let node = file_system_1.get_node(node_id).unwrap();
        entries_map.insert(node_id, (Some((node_id, node, depth.into())), None));

        if matches!(node.kind, FsNodeKind::Dir { .. }) {
            expanded.entry(node_id).or_insert(false);
        }
    }

    // Populate folder 2
    for (node_id, depth) in fs_tree_2 {
        let node = file_system_2.get_node(node_id).unwrap();
        entries_map
            .entry(node_id)
            .and_modify(|e| e.1 = Some((node_id, node, depth.into())))
            .or_insert((None, Some((node_id, node, depth.into()))));

        if matches!(node.kind, FsNodeKind::Dir { .. }) {
            expanded.entry(node_id).or_insert(false);
        }
    }

    // Ensure root is included
    let root1_id = file_system_1.get_node_id(root1);
    let root2_id = file_system_2.get_node_id(root2);

    entries_map
        .entry(root1_id)
        .or_insert((Some((root1_id, root1, 0)), Some((root2_id, root2, 0))));

    // Compute root folder diff state using ID-based version
    let root_diff_state = folder_diff_state(root1_id, &entries_map, method, partial_threshold);

    out.push(VisibleRowTwoFolderDiff {
        path: root1_id,
        is_dir: true,
        depth: 0,
        diff_state: root_diff_state,
    });

    // Build visible rows
    for (&node_id, (left, right)) in &entries_map {
        // Skip invisible nodes
        let visible = left
            .map(|(id, _, _)| is_visible(file_system_1, expanded, id))
            .or_else(|| right.map(|(id, _, _)| is_visible(file_system_2, expanded, id)))
            .unwrap_or(true);

        if !visible {
            continue;
        }

        let depth = left
            .map(|(_, _, d)| d)
            .or_else(|| right.map(|(_, _, d)| d))
            .unwrap_or(0);

        let is_dir = left
            .map(|(_, n, _)| matches!(n.kind, FsNodeKind::Dir { .. }))
            .or_else(|| right.map(|(_, n, _)| matches!(n.kind, FsNodeKind::Dir { .. })))
            .unwrap_or(false);

        let diff_state = match (left, right) {
            (Some((l_id, l_node, _)), Some((r_id, r_node, _))) => {
                if matches!(l_node.kind, FsNodeKind::Dir { .. }) {
                    // ID-based folder diff
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
            path: node_id, // now correct
            is_dir,
            depth,
            diff_state,
        });
    }

    Ok(())
}

fn on_row_item_clicked(
    file_system_1: &FileSystemModel,
    file_system_2: &FileSystemModel,
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
            let diff_tool = config;
            let result = open_diff_tool(
                &diff_tool,
                file_system_1.get_node(path1).unwrap().pathbuf(),
                file_system_2.get_node(path2).unwrap().pathbuf(),
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
    file_system_1: &FileSystemModel,
    file_system_2: &FileSystemModel,
    expanded: &mut HashMap<FsNodeId, bool>,
    row: &mut egui_extras::TableRow,
    entry: &VisibleRowTwoFolderDiff,
    row_height: f32,
    diff_tool_config: &DiffToolConfig,
) {
    let path = &entry.path;
    let is_dir = entry.is_dir;

    // Column 1
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui.add_space((entry.depth as f32) * 16.0);

            if is_dir {
                let is_open = expanded.get(path).copied().unwrap_or(false);
                let openness = if is_open { 1.0 } else { 0.0 };
                let (_rect, response) =
                    ui.allocate_exact_size(egui::vec2(12.0, row_height), egui::Sense::click());
                egui::collapsing_header::paint_default_icon(ui, openness, &response);

                if response.clicked() {
                    expanded.insert(*path, !is_open);
                }

                let filename = file_system_1.get_node(*path).unwrap().display_name();
                let label = format!("📁 {}", filename);
                if ui.label(label).interact(egui::Sense::click()).clicked() {
                    expanded.insert(*path, !is_open);
                }
            } else {
                match &entry.diff_state {
                    DiffState::Different(..)
                    | DiffState::Same(..)
                    | DiffState::Partial(..)
                    | DiffState::OnlyInFirst(..) => {
                        let filename = file_system_1.get_node(*path).unwrap().display_name();
                        if ui.label(filename).interact(egui::Sense::click()).clicked() {
                            on_row_item_clicked(
                                file_system_1,
                                file_system_2,
                                entry,
                                diff_tool_config,
                            );
                        }
                    }
                    DiffState::OnlyInSecond(..) => {}
                }
            }
        });
    });

    // Column 2
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui_custom_diff_state(ui, &entry.diff_state);
        });
    });

    // Column 3
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui.add_space((entry.depth as f32) * 16.0);

            if is_dir {
                let is_open = expanded.get(path).copied().unwrap_or(false);
                let openness = if is_open { 1.0 } else { 0.0 };
                let (_rect, response) =
                    ui.allocate_exact_size(egui::vec2(12.0, row_height), egui::Sense::click());
                egui::collapsing_header::paint_default_icon(ui, openness, &response);

                if response.clicked() {
                    expanded.insert(*path, !is_open);
                }

                let filename = file_system_1.get_node(*path).unwrap().display_name();
                let label = format!("📁 {}", filename);
                if ui.label(label).interact(egui::Sense::click()).clicked() {
                    expanded.insert(*path, !is_open);
                }
            } else {
                match &entry.diff_state {
                    DiffState::Different(..)
                    | DiffState::Same(..)
                    | DiffState::Partial(..)
                    | DiffState::OnlyInSecond(..) => {
                        let filename = file_system_1.get_node(*path).unwrap().display_name();
                        if ui.label(filename).interact(egui::Sense::click()).clicked() {
                            on_row_item_clicked(
                                file_system_1,
                                file_system_2,
                                entry,
                                diff_tool_config,
                            );
                        }
                    }
                    DiffState::OnlyInFirst(..) => {}
                }
            }
        });
    });
}

fn render_row_folder_tree_with_checkbox(
    hash_service: &mut HashService,
    file_system: &mut FileSystemModel,
    expanded: &mut HashMap<FsNodeId, bool>,
    selected: &mut HashMap<FsNodeId, bool>,
    row: &mut egui_extras::TableRow,
    entry: &VisibleRow,
    row_height: f32,
) {
    let path = &entry.path;
    let is_dir = entry.is_dir;

    // Column 1: Checkbox
    row.col(|ui| {
        ui.centered_and_justified(|ui| {
            let state = if is_dir {
                get_folder_selection_state(*path, file_system, selected)
            } else {
                if *selected.get(path).unwrap_or(&false) {
                    CheckboxSelectState::Checked
                } else {
                    CheckboxSelectState::Unchecked
                }
            };
            folder_state_ui_custom_checkbox(ui, file_system, selected, state, Some(*path));
        });
    });

    // Column 2: Name & Expand Icon
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui.add_space((entry.depth as f32) * 16.0);
            if is_dir {
                let is_open = expanded.get(path).copied().unwrap_or(false);
                let openness = if is_open { 1.0 } else { 0.0 };
                let (_rect, response) =
                    ui.allocate_exact_size(egui::vec2(12.0, row_height), egui::Sense::click());
                egui::collapsing_header::paint_default_icon(ui, openness, &response);

                if response.clicked() {
                    expanded.insert(*path, !is_open);
                }

                let filename = file_system.get_node(*path).unwrap().display_name();
                let label = format!("📁 {}", filename);
                if ui.label(label).interact(egui::Sense::click()).clicked() {
                    expanded.insert(*path, !is_open);
                }
            } else {
                let filename = file_system.get_node(*path).unwrap().display_name();
                ui.label(filename);
            }
        });
    });

    // Column 3: Hash
    row.col(|ui| {
        if !is_dir {
            let path = file_system.get_node(*path).unwrap();
            let hash_state = hash_service.get_hash(path.pathbuf());

            match hash_state {
                Some(hash_str) => {
                    let bg_color = hash_to_color(&hash_str);
                    egui::Frame::canvas(ui.style())
                        .fill(bg_color)
                        .corner_radius(3.0)
                        .inner_margin(egui::Margin::symmetric(4, 2))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(&hash_str)
                                    .monospace()
                                    .color(egui::Color32::BLACK),
                            );
                        });
                }
                None => {
                    hash_service.request(path.pathbuf());
                    ui.weak("pending...");
                }
            }
        } else {
            let snapshot = hash_service.snapshot();

            let subtree_files: Vec<_> = snapshot
                .hashes
                .iter()
                .filter(|(p, _)| p.starts_with(file_system.get_node(*path).unwrap().pathbuf()))
                .collect();

            let total = subtree_files.len();
            let hashed = subtree_files.iter().filter(|(_, h)| h.is_some()).count();
            let (progress, label) = (
                hashed as f32 / total as f32,
                format!("{}/{}", hashed, total),
            );

            ui.horizontal(|ui| {
                ui.add(
                    egui::ProgressBar::new(progress)
                        .show_percentage()
                        .desired_width(120.0),
                );

                if progress < 1.0 {
                    ui.add_space(4.0);
                    ui.weak(label);
                }
            });
        }
    });
}

fn get_folder_selection_state(
    path: FsNodeId,
    file_system: &FileSystemModel,
    selected: &HashMap<FsNodeId, bool>,
) -> CheckboxSelectState {
    let mut has_selected = false;
    let mut has_unselected = false;

    if *selected.get(&path).unwrap_or(&false) {
        has_selected = true;
    } else {
        has_unselected = true;
    }

    let node = file_system.get_node(path).unwrap();
    if let Some(children) = node.children() {
        for child_node_id in children {
            let is_selected = *selected.get(child_node_id).unwrap_or(&false);

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

pub fn folder_state_ui_custom_checkbox(
    ui: &mut egui::Ui,
    file_system: &mut FileSystemModel,
    selected: &mut HashMap<FsNodeId, bool>,
    state: CheckboxSelectState,
    path: Option<FsNodeId>,
) {
    let response = ui_custom_checkbox(ui, state.clone());

    if let Some(path) = path {
        if response.clicked() {
            let was_not_checked = state != CheckboxSelectState::Checked;

            recursive_selection(file_system, selected, path, was_not_checked);
        }
    }
}

pub fn recursive_selection(
    model: &FileSystemModel,
    selected: &mut HashMap<FsNodeId, bool>,
    node_id: FsNodeId,
    value: bool,
) {
    let node = model.get_node(node_id).unwrap();

    match &node.kind {
        FsNodeKind::File { path } => {
            selected.insert(node_id, value);
        }
        FsNodeKind::Dir { path, children } => {
            selected.insert(node_id, value);

            for &child_id in children {
                recursive_selection(model, selected, child_id, value);
            }
        }
    }
}

pub fn recursive_expand(
    expanded: &mut HashMap<FsNodeId, bool>,
    file_system: &FileSystemModel,
    node_id: FsNodeId,
) {
    let node = file_system.get_node(node_id).unwrap();

    // Only directories are expandable
    if let FsNodeKind::Dir { path, children } = &node.kind {
        // Compute relative path from root
        let root_path = &file_system.get_root().kind;

        expanded.insert(file_system.get_node_id(&file_system.get_root()), true);

        // Recursively expand children directories
        for &child_id in children {
            let child_node = file_system.get_node(child_id).unwrap();
            if let FsNodeKind::Dir { .. } = &child_node.kind {
                recursive_expand(expanded, file_system, child_id);
            }
        }
    }
}
