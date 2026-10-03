use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use serde::Deserialize;
use serde::Serialize;
use zcommon::hash::HashRepresentation;
use zcommon::hash::HashService;
use zcommon::ui_egui::common::show_custom_popup;
use zcommon::ui_egui::common::show_custom_popup_with_color;
use zhashdiff::conflict::ResolveConflictsInput;
use zhashdiff::conflict::execute_resolution;
use zhashdiff::conflict::group_duplicates;
use zhashdiff::fs::FileSystemModel;

use crate::ui_egui::fs_tree::FileSystemView;
use crate::ui_egui::fs_tree::draw_ui_folder_tree_with_checkbox;
use crate::ui_egui::panes::PathDiffView;
use crate::ui_egui::panes::ZAppPane;
use zcommon::ui_egui::common::CheckboxSelectState;
use zcommon::ui_egui::common::hash_to_color;
use zcommon::ui_egui::common::ui_custom_checkbox;

const MAX_CONCURRENT_HASHES: usize = 16;

struct CheckedFileHashes {
    hashed: Vec<(PathBuf, HashRepresentation)>,
    unhashed: usize,
}

/// Checked files of `view` paired with their hash; checked files without a hash yet
/// are only counted.
fn checked_file_hashes(
    view: &FileSystemView,
    hashes: &HashMap<PathBuf, Option<HashRepresentation>>,
) -> CheckedFileHashes {
    let mut checked = CheckedFileHashes {
        hashed: Vec::new(),
        unhashed: 0,
    };

    for node_id in view.file_system.iter_files() {
        if !view.selected.get(&node_id).copied().unwrap_or(false) {
            continue;
        }
        let path = view
            .file_system
            .get_node(node_id)
            .expect("iter_files yields valid node ids")
            .as_path()
            .as_ref()
            .to_path_buf();
        match hashes.get(&path) {
            Some(Some(hash)) => checked.hashed.push((path, hash.clone())),
            _ => checked.unhashed += 1,
        }
    }

    checked
}

#[derive(Serialize, Deserialize)]
pub struct DuplicateFilesPane {
    pub title: Option<String>,

    #[serde(skip)]
    open_diff_popup: bool,
    #[serde(skip)]
    pub open_dir_window: bool,
}

impl ZAppPane for DuplicateFilesPane {
    fn title(&self) -> String {
        self.title.clone().unwrap_or("File Explorer".into())
    }
}

pub struct DuplicateFilesPaneCtx<'a, 'b> {
    pub hash_service: &'a mut HashService,
    pub path_diff_view: &'a mut PathDiffView<'b>,

    // Diff Action State
    pub active_conflict_hash: &'a mut Option<String>,
    pub conflict_map: &'a mut HashMap<String, Vec<PathBuf>>,
    pub conflict_map_resolved: &'a mut HashMap<String, PathBuf>,
    #[allow(dead_code)]
    pub diff_action_pressed: &'a mut bool,
}

impl DuplicateFilesPane {
    pub fn new(title: Option<String>) -> Self {
        Self {
            title,
            open_diff_popup: false,
            open_dir_window: false,
        }
    }

    /// Diff button: groups the checked, already hashed files by hash and opens the
    /// conflicts window. Returns the notice logged about checked files that were left
    /// out because they are not hashed yet.
    fn diff_checked_files(
        &mut self,
        ctx: &mut DuplicateFilesPaneCtx,
        hashes: &HashMap<PathBuf, Option<HashRepresentation>>,
    ) -> Option<String> {
        let view = ctx.path_diff_view.file_system_1_view.as_ref()?;
        let checked = checked_file_hashes(view, hashes);

        let notice = (checked.unhashed > 0).then(|| {
            format!(
                "Diff left out {} checked files that are not hashed yet",
                checked.unhashed
            )
        });
        if let Some(notice) = &notice {
            log::warn!("{notice}");
        }

        let groups = group_duplicates(checked.hashed);
        log::info!("Diff found {} duplicate groups", groups.len());
        *ctx.conflict_map = groups.into_iter().collect();

        // Keepers from an earlier Diff survive only if they still belong to their group.
        let conflict_map = &*ctx.conflict_map;
        ctx.conflict_map_resolved.retain(|hash, keeper| {
            conflict_map
                .get(hash)
                .is_some_and(|paths| paths.contains(keeper))
        });
        *ctx.active_conflict_hash = None;
        self.open_diff_popup = true;

        notice
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut DuplicateFilesPaneCtx,
    ) -> egui_tiles::UiResponse {
        ui.vertical(|ui| {
            self.ui_popups(ui, ctx);

            ui.horizontal(|ui| {
                if ui.button("Open Folder").clicked() {
                    self.open_dir_window = true;
                }

                if let Some(fs_view) = &mut ctx.path_diff_view.file_system_1_view {
                    let nodes_considered_for_collapse =
                        &fs_view.file_system.get_root().children().unwrap().clone();
                    let is_anything_collapsed =
                        fs_view.is_anything_collapsed_slice(nodes_considered_for_collapse);
                    let button_text = if is_anything_collapsed {
                        "Collapse All"
                    } else {
                        "Expand All"
                    };

                    if ui.button(button_text).clicked() {
                        fs_view.recursive_collapse_slice(
                            nodes_considered_for_collapse,
                            !is_anything_collapsed,
                        );
                    }
                }

                if ui.button("Request All Hash").clicked() {
                    if let Some(file_system_view) = ctx.path_diff_view.file_system_1_view {
                        let file_system = &file_system_view.file_system;
                        let all_files: Vec<_> = file_system.iter_files().collect();

                        for node_id in all_files {
                            if let Some(node) = file_system.get_node(node_id) {
                                ctx.hash_service.request(node.as_path());
                            }
                        }
                    }
                }

                if ui.button("Clear Hashes").clicked() {
                    ctx.hash_service.clear();
                }

                ui.label("Concurrent Hashes");
                let mut slider_concurrent_hashes = ctx.hash_service.count_threads();
                let slider_response = ui.add(egui::Slider::new(
                    &mut slider_concurrent_hashes,
                    0..=MAX_CONCURRENT_HASHES,
                ));
                if slider_response.changed() {
                    ctx.hash_service.resize_workers(slider_concurrent_hashes);
                }
            });
        });

        ui.separator();

        let mut show_diff_button = false;
        egui::ScrollArea::vertical()
            .max_height(500.0)
            .show(ui, |ui| {
                if let Some(file_system_view) = ctx.path_diff_view.file_system_1_view {
                    draw_ui_folder_tree_with_checkbox(ui, file_system_view, ctx.hash_service);
                    show_diff_button = true;
                } else {
                    ui.label("No root dir set...");
                    if ui.button("Open Folder").clicked() {
                        self.open_dir_window = true;
                    }
                }
            });

        if show_diff_button {
            if ui.button("Diff").clicked() {
                let snapshot = ctx.hash_service.snapshot();
                self.diff_checked_files(ctx, &snapshot.hashes);
            }
        }

        if self.open_dir_window {
            self.open_dir_window = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                // ctx.path_diff_view.file_system_1.get_root()_dir_cache.clear();
                match FileSystemModel::new(path) {
                    Ok(new_model) => {
                        *ctx.path_diff_view.file_system_1_view =
                            Some(FileSystemView::new(Arc::new(new_model)));
                    }
                    Err(e) => log::error!("{e}"),
                }
            }
        }

        egui_tiles::UiResponse::None
    }

    fn ui_popups(&mut self, ui: &mut egui::Ui, ctx: &mut DuplicateFilesPaneCtx) {
        if self.open_diff_popup {
            let mut temp_show_diff_popup = self.open_diff_popup;
            let mut did_resolve = false;
            let mut deferred_hash_toggle: Option<String> = None;

            let mut conflicts: Vec<(String, Vec<std::path::PathBuf>, bool)> = ctx
                .conflict_map
                .iter()
                .map(|(hash, paths)| {
                    (
                        hash.clone(),
                        paths.clone(),
                        ctx.conflict_map_resolved.contains_key(hash),
                    )
                })
                .collect();
            conflicts.sort_by(|a, b| a.0.cmp(&b.0));

            let total_conflicts = conflicts.len();
            let resolved_count = conflicts
                .iter()
                .filter(|(_, _, is_resolved)| *is_resolved)
                .count();

            show_custom_popup(
                ui.ctx(),
                &mut temp_show_diff_popup,
                "Conflicts",
                true,
                |ui| {
                    ui.vertical(|ui| {
                        ui.label(format!("Resolved: {}/{}", resolved_count, total_conflicts));
                        ui.separator();

                        let row_height = 24.0;
                        let header_height = 30.0;
                        let table_height = ui.available_height() - 100.0;

                        egui::Frame::new()
                            .fill(egui::Color32::from_gray(25))
                            .show(ui, |ui| {
                                use egui_extras::{Column, TableBuilder};

                                TableBuilder::new(ui)
                                    .striped(true)
                                    .resizable(false)
                                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                                    .column(Column::exact(32.0)) // Checkbox
                                    .column(Column::exact(80.0)) // Hash (Short)
                                    .column(Column::remainder())
                                    .min_scrolled_height(100.0)
                                    .max_scroll_height(table_height)
                                    .header(header_height, |mut header| {
                                        header.col(|ui| {
                                            ui.centered_and_justified(|ui| {
                                                ui.label("✔");
                                            });
                                        });
                                        header.col(|ui| {
                                            ui.label("Hash ID");
                                        });
                                        header.col(|ui| {
                                            ui.label("Occurrences");
                                        });
                                    })
                                    .body(|body| {
                                        body.rows(row_height, total_conflicts, |mut row| {
                                            let index = row.index();
                                            let (hash, paths, is_resolved) = &conflicts[index];

                                            // Checkbox Column
                                            row.col(|ui| {
                                                let state = if *is_resolved {
                                                    CheckboxSelectState::Checked
                                                } else {
                                                    CheckboxSelectState::Unchecked
                                                };

                                                if ui_custom_checkbox(ui, state).clicked() {
                                                    deferred_hash_toggle = Some(hash.clone());
                                                }
                                            });

                                            // Hash Column
                                            row.col(|ui| {
                                                let color = hash_to_color(hash);
                                                let rect = egui::Frame::new()
                                                    .fill(color)
                                                    .corner_radius(4.0)
                                                    .inner_margin(2.0)
                                                    .show(ui, |ui| {
                                                        ui.label(
                                                            egui::RichText::new(&hash[0..8])
                                                                .color(egui::Color32::BLACK)
                                                                .strong(),
                                                        );
                                                    })
                                                    .response
                                                    .rect;

                                                if ui
                                                    .interact(
                                                        rect.expand(4.0),
                                                        ui.id().with(hash),
                                                        egui::Sense::click(),
                                                    )
                                                    .clicked()
                                                {
                                                    deferred_hash_toggle = Some(hash.clone());
                                                }
                                            });

                                            // Occurrences Column
                                            row.col(|ui| {
                                                let label_text = format!("{} files", paths.len());
                                                if ui.selectable_label(false, label_text).clicked()
                                                {
                                                    deferred_hash_toggle = Some(hash.clone());
                                                }
                                            });
                                        });
                                    });
                            });

                        ui.separator();

                        // Resolution Button
                        ui.add_enabled_ui(resolved_count > 0, |ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Resolve All Selected").clicked() {
                                        let resolution_input = ResolveConflictsInput {
                                            conflict_map: ctx.conflict_map.clone(),
                                            conflict_map_resolved: ctx
                                                .conflict_map_resolved
                                                .clone(),
                                        };

                                        let removed_files =
                                            execute_resolution(&resolution_input).removed_files;
                                        for path in removed_files {
                                            ctx.hash_service.remove(&path);
                                        }

                                        ctx.conflict_map.clear();
                                        ctx.conflict_map_resolved.clear();
                                        *ctx.active_conflict_hash = None;
                                        did_resolve = true;
                                    }
                                },
                            );
                        });
                    });
                },
            );

            if let Some(hash) = deferred_hash_toggle {
                if ctx.conflict_map_resolved.contains_key(&hash) {
                    ctx.conflict_map_resolved.remove(&hash);
                } else {
                    ctx.conflict_map_resolved
                        .insert(hash.clone(), PathBuf::new()); // Placeholder
                }
                *ctx.active_conflict_hash = Some(hash);
            }

            self.open_diff_popup = temp_show_diff_popup && !did_resolve;
        }

        self.ui_conflict_details(ui, ctx);
    }

    fn ui_conflict_details(&mut self, ui: &mut egui::Ui, ctx: &mut DuplicateFilesPaneCtx) {
        if let Some(selected_hash) = ctx.active_conflict_hash.clone() {
            let mut is_open = true;
            let mut temp_is_open = is_open;

            if let Some(value) = ctx.conflict_map.get(&selected_hash) {
                let hash_color = hash_to_color(&selected_hash);
                show_custom_popup_with_color(
                    ui.ctx(),
                    &mut temp_is_open,
                    &format!("Conflict Detail: {}", &selected_hash[0..8]),
                    hash_color,
                    |ui| {
                        ui.label(egui::RichText::new("Select the file you wish to keep:").strong());
                        ui.add_space(8.0);

                        let mut is_unresolved =
                            !ctx.conflict_map_resolved.contains_key(&selected_hash);
                        if ui
                            .radio_value(&mut is_unresolved, true, "Unresolved / None")
                            .clicked()
                        {
                            ctx.conflict_map_resolved.remove(&selected_hash);
                        }

                        ui.separator();

                        egui::ScrollArea::vertical()
                            .max_height(200.0)
                            .show(ui, |ui| {
                                for path in value {
                                    let is_this_path_selected =
                                        ctx.conflict_map_resolved.get(&selected_hash) == Some(path);
                                    if ui
                                        .selectable_label(
                                            is_this_path_selected,
                                            path.to_string_lossy(),
                                        )
                                        .clicked()
                                    {
                                        ctx.conflict_map_resolved
                                            .insert(selected_hash.clone(), path.clone());
                                    }
                                }
                            });

                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("Close").clicked() {
                                is_open = false;
                            }
                        });
                    },
                );
            }

            if !temp_is_open || !is_open {
                *ctx.active_conflict_hash = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_egui::fs_tree::VisibleRowTwoFolderDiff;
    use std::fs;
    use std::path::Path;
    use tempfile::{TempDir, tempdir};
    use zcommon::hash::hash_file_mmap;

    /// Owns everything a `DuplicateFilesPaneCtx` borrows.
    struct PaneState {
        hash_service: HashService,
        root_1: Option<PathBuf>,
        root_2: Option<PathBuf>,
        view_1: Option<FileSystemView>,
        view_2: Option<FileSystemView>,
        visible_rows: Option<Vec<VisibleRowTwoFolderDiff>>,
        active_conflict_hash: Option<String>,
        conflict_map: HashMap<String, Vec<PathBuf>>,
        conflict_map_resolved: HashMap<String, PathBuf>,
        diff_action_pressed: bool,
    }

    impl PaneState {
        fn new(view: FileSystemView) -> Self {
            Self {
                hash_service: HashService::new(0),
                root_1: None,
                root_2: None,
                view_1: Some(view),
                view_2: None,
                visible_rows: None,
                active_conflict_hash: None,
                conflict_map: HashMap::new(),
                conflict_map_resolved: HashMap::new(),
                diff_action_pressed: false,
            }
        }

        fn press_diff(
            &mut self,
            pane: &mut DuplicateFilesPane,
            hashes: &HashMap<PathBuf, Option<HashRepresentation>>,
        ) -> Option<String> {
            let mut path_diff_view = PathDiffView {
                file_system_1_root_path: &mut self.root_1,
                file_system_2_root_path: &mut self.root_2,
                file_system_1_view: &mut self.view_1,
                file_system_2_view: &mut self.view_2,
                visible_rows: &mut self.visible_rows,
            };
            let mut ctx = DuplicateFilesPaneCtx {
                hash_service: &mut self.hash_service,
                path_diff_view: &mut path_diff_view,
                active_conflict_hash: &mut self.active_conflict_hash,
                conflict_map: &mut self.conflict_map,
                conflict_map_resolved: &mut self.conflict_map_resolved,
                diff_action_pressed: &mut self.diff_action_pressed,
            };
            pane.diff_checked_files(&mut ctx, hashes)
        }
    }

    /// a.txt, b.txt, sub/c.txt and e.txt share content, d.txt is unique.
    /// Everything is checked except e.txt.
    fn duplicate_tree() -> (TempDir, FileSystemView) {
        let dir = tempdir().unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();
        for name in ["a.txt", "b.txt", "sub/c.txt", "e.txt"] {
            fs::write(dir.path().join(name), "same").unwrap();
        }
        fs::write(dir.path().join("d.txt"), "other").unwrap();

        let model = FileSystemModel::new(dir.path()).unwrap();
        let mut view = FileSystemView::new(Arc::new(model));
        view.recursive_selection(view.file_system.get_root_node_id(), true);
        let e_id = view
            .file_system
            .find_path(dir.path().join("e.txt"))
            .unwrap();
        view.selected.insert(e_id, false);
        (dir, view)
    }

    fn hashes_of(paths: &[&Path]) -> HashMap<PathBuf, Option<HashRepresentation>> {
        paths
            .iter()
            .map(|p| (p.to_path_buf(), Some(hash_file_mmap(p).unwrap())))
            .collect()
    }

    fn same_hash(dir: &Path) -> String {
        hash_file_mmap(dir.join("a.txt")).unwrap()
    }

    #[test]
    fn diff_opens_conflicts_window_with_groups_of_checked_hashed_files() {
        let (dir, view) = duplicate_tree();
        let root = dir.path();
        let hashes = hashes_of(&[
            &root.join("a.txt"),
            &root.join("b.txt"),
            &root.join("sub").join("c.txt"),
            &root.join("d.txt"),
            &root.join("e.txt"),
        ]);
        let mut state = PaneState::new(view);
        let mut pane = DuplicateFilesPane::new(None);

        let notice = state.press_diff(&mut pane, &hashes);

        assert_eq!(notice, None);
        assert!(pane.open_diff_popup);
        assert_eq!(
            state.conflict_map,
            HashMap::from([(
                same_hash(root),
                vec![
                    root.join("a.txt"),
                    root.join("b.txt"),
                    root.join("sub").join("c.txt")
                ],
            )])
        );
    }

    #[test]
    fn diff_reports_checked_files_that_are_not_hashed() {
        let (dir, view) = duplicate_tree();
        let root = dir.path();
        // sub/c.txt is queued but not hashed, d.txt was never requested, e.txt is unchecked.
        let mut hashes = hashes_of(&[&root.join("a.txt"), &root.join("b.txt")]);
        hashes.insert(root.join("sub").join("c.txt"), None);
        let mut state = PaneState::new(view);
        let mut pane = DuplicateFilesPane::new(None);

        let notice = state
            .press_diff(&mut pane, &hashes)
            .expect("unhashed checked files are reported");

        assert!(
            notice.contains(" 2 "),
            "notice doesn't name the count: {notice}"
        );
        assert!(pane.open_diff_popup);
        assert_eq!(
            state.conflict_map,
            HashMap::from([(
                same_hash(root),
                vec![root.join("a.txt"), root.join("b.txt")]
            )])
        );
    }

    #[test]
    fn diff_again_keeps_valid_keepers_only() {
        let (dir, view) = duplicate_tree();
        let root = dir.path();
        let hashes = hashes_of(&[&root.join("a.txt"), &root.join("b.txt")]);
        let mut state = PaneState::new(view);
        let mut pane = DuplicateFilesPane::new(None);
        state.press_diff(&mut pane, &hashes);

        let hash = same_hash(root);
        state
            .conflict_map_resolved
            .insert(hash.clone(), root.join("b.txt"));
        state
            .conflict_map_resolved
            .insert("gone".to_string(), PathBuf::new());
        state.active_conflict_hash = Some("gone".to_string());

        state.press_diff(&mut pane, &hashes);

        assert_eq!(
            state.conflict_map_resolved,
            HashMap::from([(hash, root.join("b.txt"))])
        );
        assert_eq!(state.active_conflict_hash, None);
    }
}
