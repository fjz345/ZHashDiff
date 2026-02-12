use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use eframe::egui::{self, RichText};
use egui_extras::{Column, TableBuilder};
use zhashdiff::{
    fs::{FileSystem, FsEntry},
    hash::{HashService, hash_file},
};

use crate::ui_egui::common::{CheckboxSelectState, hash_to_color, ui_custom_checkbox};

pub fn draw_ui_folder_tree_with_checkbox(
    ui: &mut egui::Ui,
    root: &PathBuf,
    expanded: &mut HashMap<PathBuf, bool>,
    selected: &mut HashMap<PathBuf, bool>,
    file_system: &mut FileSystem,
    hash_service: &mut HashService,
) -> egui::response::Response {
    let mut visible_rows = Vec::new();
    build_expanded_rows(
        expanded,
        file_system,
        hash_service,
        root,
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
                .id_salt(root)
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
                                let state = get_folder_selection_state(&root, selected);
                                folder_state_ui_custom_checkbox(
                                    ui,
                                    file_system,
                                    selected,
                                    state,
                                    &root,
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
    root_1: &mut PathBuf,
    root_2: &mut PathBuf,
    expanded: &mut HashMap<PathBuf, bool>,
    selected: &mut HashMap<PathBuf, bool>,
    file_system_1: &mut FileSystem,
    file_system_2: &mut FileSystem,
    open_dir_window_1: &mut bool,
    open_dir_window_2: &mut bool,
) -> egui::response::Response {
    let mut visible_rows = Vec::new();
    build_two_folder_diff_rows(expanded, file_system_1, file_system_2, &mut visible_rows);
    let row_count = visible_rows.len();

    let available_height = ui.available_height();

    let response = egui::Frame::new()
        .fill(egui::Color32::from_gray(20))
        .inner_margin(0.0)
        .show(ui, |ui| {
            ui.set_min_height(available_height);
            let row_height = ui.text_style_height(&egui::TextStyle::Body);
            let row_height_header = ui.text_style_height(&egui::TextStyle::Heading);

            TableBuilder::new(ui)
                .id_salt(root_1.clone())
                .striped(true)
                .resizable(true)
                .auto_shrink([false, true])
                .column(Column::remainder().at_least(500.0))
                .column(Column::auto().at_least(7.0))
                .column(Column::remainder().at_least(500.0))
                .header(row_height_header, |mut header| {
                    // COLUMN 0 (Folder 1)
                    header.col(|ui| {
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

                            let mut text = root_1.to_string_lossy().to_string(); // adapt to your actual root 1
                            ui.add_sized(
                                [available, row_height_header],
                                egui::TextEdit::singleline(&mut text),
                            );

                            ui.spacing();
                        });
                    });

                    // COLUMN 1 (Diff icon header)
                    header.col(|ui| {
                        ui.centered_and_justified(|ui| {
                            ui.label("≠");
                        });
                    });

                    // COLUMN 2 (Folder 2)
                    header.col(|ui| {
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

                            let mut text = root_2.to_string_lossy().to_string(); // adapt to your actual root 2
                            ui.add_sized(
                                [available, row_height_header],
                                egui::TextEdit::singleline(&mut text),
                            );

                            ui.spacing();
                        });
                    });
                })
                .body(|body| {
                    body.rows(row_height, row_count, |mut row| {
                        let entry = &visible_rows[row.index()];
                        render_row_folder_tree_diff_column(expanded, &mut row, entry, row_height);
                    });
                });
        });

    response.response
}

struct VisibleRow {
    path: PathBuf,
    is_dir: bool,
    depth: usize,
}

pub enum DiffState {
    Different,
    Same,
    OnlyInFirst,
    OnlyInSecond,
}

pub fn ui_custom_diff_state(ui: &mut egui::Ui, state: &DiffState) -> egui::response::Response {
    match state {
        DiffState::Same => ui.label(RichText::new("=").color(egui::Color32::GREEN)),
        DiffState::Different => ui.label(RichText::new("≠").color(egui::Color32::YELLOW)),
        DiffState::OnlyInFirst => ui.label(RichText::new("−").color(egui::Color32::RED)),
        DiffState::OnlyInSecond => ui.label(RichText::new("+").color(egui::Color32::RED)),
    }
}

struct VisibleRowTwoFolderDiff {
    path: PathBuf,
    is_dir: bool,
    depth: usize,
    diff_state: DiffState,
}

fn build_expanded_rows(
    expanded: &mut HashMap<PathBuf, bool>,
    file_system: &mut FileSystem,
    hash_service: &mut HashService,
    current_path: &PathBuf,
    depth: usize,
    out: &mut Vec<VisibleRow>,
) {
    let fs_path = file_system.get(current_path);

    for entry in &fs_path.entries {
        let (path, is_dir) = match entry {
            FsEntry::Dir { path } => (path, true),
            FsEntry::File { path } => (path, false),
        };

        out.push(VisibleRow {
            path: path.clone(),
            is_dir,
            depth,
        });
        if is_dir && expanded.get(&path.clone()).copied().unwrap_or(false) {
            build_expanded_rows(expanded, file_system, hash_service, &path, depth + 1, out);
        }
    }
}
/// Check if a given path is visible based on expanded folders
fn is_visible(expanded: &HashMap<PathBuf, bool>, path: &Path) -> bool {
    let mut ancestor = path.parent();

    while let Some(p) = ancestor {
        if let Some(&is_expanded) = expanded.get(p) {
            if !is_expanded {
                return false;
            }
        }
        ancestor = p.parent();
    }

    true
}

fn build_two_folder_diff_rows(
    expanded: &mut HashMap<PathBuf, bool>,
    file_system_1: &mut FileSystem,
    file_system_2: &mut FileSystem,
    out: &mut Vec<VisibleRowTwoFolderDiff>,
) {
    out.clear();

    let root1 = file_system_1.root.clone();
    let root2 = file_system_2.root.clone();

    let fs_path_1 = file_system_1.get(&root1);
    let fs_path_2 = file_system_2.get(&root2);

    let fs_path_1_flat = FileSystem::read_path_recursive_flatten(fs_path_1.root.path_buf());
    let fs_path_2_flat = FileSystem::read_path_recursive_flatten(fs_path_2.root.path_buf());

    // Use BTreeMap for stable ordering
    let mut entries_map: BTreeMap<PathBuf, (Option<(&FsEntry, usize)>, Option<(&FsEntry, usize)>)> =
        BTreeMap::new();

    // LEFT
    for (entry, depth) in &fs_path_1_flat.entries {
        let rel = entry.relative_path_buf(&root1);
        entries_map.insert(rel.clone(), (Some((entry, *depth)), None));

        // Register folders in expanded map if missing
        if matches!(entry, FsEntry::Dir { .. }) {
            expanded.entry(rel).or_insert(false);
        }
    }

    // RIGHT
    for (entry, depth) in &fs_path_2_flat.entries {
        let rel = entry.relative_path_buf(&root2);
        entries_map
            .entry(rel.clone())
            .and_modify(|e| e.1 = Some((entry, *depth)))
            .or_insert((None, Some((entry, *depth))));

        // Register folders in expanded map if missing
        if matches!(entry, FsEntry::Dir { .. }) {
            expanded.entry(rel).or_insert(false);
        }
    }

    // BUILD ROWS
    for (rel_path, (left, right)) in entries_map {
        // Skip entry if any parent folder is collapsed
        if !is_visible(expanded, &rel_path) {
            continue;
        }

        let depth = left
            .map(|(_, d)| d)
            .or_else(|| right.map(|(_, d)| d))
            .unwrap_or(0);

        let is_dir = left
            .map(|(e, _)| matches!(e, FsEntry::Dir { .. }))
            .or_else(|| right.map(|(e, _)| matches!(e, FsEntry::Dir { .. })))
            .unwrap_or(false);

        let diff_state = match (left, right) {
            (Some((l, _)), Some((r, _))) => {
                if matches!(l, FsEntry::Dir { .. }) {
                    DiffState::Same
                } else if hash_file(&l.path().to_string_lossy()).unwrap()
                    == hash_file(&r.path().to_string_lossy()).unwrap()
                {
                    DiffState::Same
                } else {
                    DiffState::Different
                }
            }
            (Some(_), None) => DiffState::OnlyInFirst,
            (None, Some(_)) => DiffState::OnlyInSecond,
            (None, None) => continue,
        };

        out.push(VisibleRowTwoFolderDiff {
            path: rel_path,
            is_dir,
            depth,
            diff_state,
        });
    }
}

fn render_row_folder_tree_diff_column(
    expanded: &mut HashMap<PathBuf, bool>,
    row: &mut egui_extras::TableRow,
    entry: &VisibleRowTwoFolderDiff,
    row_height: f32,
) {
    let path = &entry.path;
    let is_dir = entry.is_dir;

    // Column 2: Name & Expand Icon
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui.add_space((entry.depth as f32) * 16.0);
            match &entry.diff_state {
                DiffState::Different | DiffState::Same | DiffState::OnlyInFirst => {
                    if is_dir {
                        let is_open = expanded.get(path).copied().unwrap_or(false);
                        let openness = if is_open { 1.0 } else { 0.0 };
                        let (_rect, response) = ui.allocate_exact_size(
                            egui::vec2(12.0, row_height),
                            egui::Sense::click(),
                        );
                        egui::collapsing_header::paint_default_icon(ui, openness, &response);

                        if response.clicked() {
                            expanded.insert(path.clone(), !is_open);
                        }

                        let label = format!(
                            "📁 {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        );
                        if ui.label(label).interact(egui::Sense::click()).clicked() {
                            expanded.insert(path.clone(), !is_open);
                        }
                    } else {
                        ui.label(path.file_name().unwrap_or_default().to_string_lossy());
                    }
                }
                DiffState::OnlyInSecond => {}
            }
        });
    });

    // Column 2: Name & Expand Icon
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui_custom_diff_state(ui, &entry.diff_state);
        });
    });

    // Column 2: Name & Expand Icon
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui.add_space((entry.depth as f32) * 16.0);

            match &entry.diff_state {
                DiffState::Different | DiffState::Same | DiffState::OnlyInSecond => {
                    if is_dir {
                        let is_open = expanded.get(path).copied().unwrap_or(false);
                        let openness = if is_open { 1.0 } else { 0.0 };
                        let (_rect, response) = ui.allocate_exact_size(
                            egui::vec2(12.0, row_height),
                            egui::Sense::click(),
                        );
                        egui::collapsing_header::paint_default_icon(ui, openness, &response);

                        if response.clicked() {
                            expanded.insert(path.clone(), !is_open);
                        }

                        let label = format!(
                            "📁 {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        );
                        if ui.label(label).interact(egui::Sense::click()).clicked() {
                            expanded.insert(path.clone(), !is_open);
                        }
                    } else {
                        ui.label(path.file_name().unwrap_or_default().to_string_lossy());
                    }
                }
                DiffState::OnlyInFirst => {}
            }
        });
    });
}

fn render_row_folder_tree_with_checkbox(
    hash_service: &mut HashService,
    file_system: &mut FileSystem,
    expanded: &mut HashMap<PathBuf, bool>,
    selected: &mut HashMap<PathBuf, bool>,
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
                get_folder_selection_state(path, selected)
            } else {
                if *selected.get(path).unwrap_or(&false) {
                    CheckboxSelectState::Checked
                } else {
                    CheckboxSelectState::Unchecked
                }
            };
            folder_state_ui_custom_checkbox(ui, file_system, selected, state, path);
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
                    expanded.insert(path.clone(), !is_open);
                }

                let label = format!(
                    "📁 {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
                if ui.label(label).interact(egui::Sense::click()).clicked() {
                    expanded.insert(path.clone(), !is_open);
                }
            } else {
                ui.label(path.file_name().unwrap_or_default().to_string_lossy());
            }
        });
    });

    // Column 3: Hash
    row.col(|ui| {
        if !is_dir {
            let hash_state = hash_service.get(path);

            match hash_state {
                Some(Some(hash_str)) => {
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
                Some(None) => {
                    ui.weak("hashing...");
                }
                None => {
                    hash_service.request(path.clone());
                    ui.weak("pending...");
                }
            }
        } else {
            let snapshot = hash_service.snapshot();

            let subtree_files: Vec<_> = snapshot
                .hashes
                .iter()
                .filter(|(p, _)| p.starts_with(path))
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
    path: &PathBuf,
    selected: &HashMap<PathBuf, bool>,
) -> CheckboxSelectState {
    let flatten_entries = FileSystem::read_path_recursive_flatten(path);

    let mut has_selected = false;
    let mut has_unselected = false;

    // Check the folder itself first
    if *selected.get(path).unwrap_or(&false) {
        has_selected = true;
    } else {
        has_unselected = true;
    }

    // Check all flattened children
    for entry in &flatten_entries.entries {
        let p = match &entry.0 {
            FsEntry::File { path } => path,
            FsEntry::Dir { path } => path,
        };

        let is_selected = *selected.get(p).unwrap_or(&false);

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

    if has_selected {
        CheckboxSelectState::Checked
    } else {
        CheckboxSelectState::Unchecked
    }
}

pub fn folder_state_ui_custom_checkbox(
    ui: &mut egui::Ui,
    file_system: &mut FileSystem,
    selected: &mut HashMap<PathBuf, bool>,
    state: CheckboxSelectState,
    path: &PathBuf,
) {
    let response = ui_custom_checkbox(ui, state.clone());

    if response.clicked() {
        let was_not_checked = state != CheckboxSelectState::Checked;

        recursive_selection(file_system, selected, path, was_not_checked);
    }
}

fn recursive_selection(
    file_system: &mut FileSystem,
    selected: &mut HashMap<PathBuf, bool>,
    path: &PathBuf,
    value: bool,
) {
    let fs_path = file_system.get(path);
    selected.insert(fs_path.root.path_buf().clone(), value);

    for entry in &fs_path.entries {
        match entry {
            FsEntry::File { path: p } => {
                selected.insert(p.clone(), value);
            }
            FsEntry::Dir { path: p } => {
                recursive_selection(file_system, selected, p, value);
            }
        }
    }
}

pub fn recursive_expand(
    expanded: &mut HashMap<PathBuf, bool>,
    file_system: &FileSystem,
    path: &PathBuf, // current folder in recursion
) {
    let root = &file_system.root;
    // Compute relative path from root
    let rel_path = if path == root {
        PathBuf::from("") // root itself
    } else {
        path.strip_prefix(root).unwrap().to_path_buf()
    };

    // Mark this folder as expanded
    expanded.insert(rel_path.clone(), true);

    let fs_path = file_system.get(path);
    for entry in &fs_path.entries {
        if let FsEntry::Dir { path: child_path } = entry {
            recursive_expand(expanded, file_system, child_path);
        }
    }
}
