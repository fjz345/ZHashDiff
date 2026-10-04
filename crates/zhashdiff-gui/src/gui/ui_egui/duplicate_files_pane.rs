use std::collections::HashMap;
use std::collections::HashSet;
use std::fmt::Display;
use std::io;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use serde::Deserialize;
use serde::Serialize;
use zcommon::hash::HashRepresentation;
use zcommon::hash::HashService;
use zcommon::ui_egui::common::show_custom_popup;
use zcommon::ui_egui::common::show_custom_popup_with_color;
use zhashdiff::conflict::ResolutionSummary;
use zhashdiff::conflict::execute_resolution;
use zhashdiff::conflict::group_duplicates;
use zhashdiff::conflict::plan_resolution;
use zhashdiff::conflict::recycle;
use zhashdiff::filter::PathFilter;
use zhashdiff::fs::FileSystemModel;
use zhashdiff::fs::FsNodeId;

use crate::ui_egui::fs_tree::FileSystemView;
use crate::ui_egui::fs_tree::PENDING_DELETION_COLOR;
use crate::ui_egui::fs_tree::draw_ui_folder_tree_with_checkbox;
use crate::ui_egui::fs_tree::tree_hidden;
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
/// are only counted. Files the filter hides are left out even when checked and hashed.
fn checked_file_hashes(
    view: &FileSystemView,
    hidden: &[bool],
    hashes: &HashMap<PathBuf, Option<HashRepresentation>>,
) -> CheckedFileHashes {
    let mut checked = CheckedFileHashes {
        hashed: Vec::new(),
        unhashed: 0,
    };

    for node_id in view.file_system.iter_files() {
        if hidden[node_id] || !view.selected.get(&node_id).copied().unwrap_or(false) {
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

/// The files "Request All Hash" queues: every file the filter leaves visible.
fn files_to_hash(view: &FileSystemView, hidden: &[bool]) -> Vec<PathBuf> {
    view.file_system
        .iter_files()
        .filter(|&node_id| !hidden[node_id])
        .map(|node_id| {
            view.file_system
                .get_node(node_id)
                .expect("iter_files yields valid node ids")
                .as_path()
                .as_ref()
                .to_path_buf()
        })
        .collect()
}

fn view_root(view: Option<&FileSystemView>) -> Option<PathBuf> {
    view.map(|view| view.file_system.get_root().as_path().as_ref().to_path_buf())
}

/// The group's keeper, if it has one that is a member of the group.
fn keeper_of<'a>(
    paths: &[PathBuf],
    keepers: &'a HashMap<String, PathBuf>,
    hash: &str,
) -> Option<&'a PathBuf> {
    keepers.get(hash).filter(|keeper| paths.contains(keeper))
}

/// Files a resolve would delete. Taken from the resolve's own plan so the marks can't
/// drift from what is removed.
fn pending_deletions(
    conflict_map: &HashMap<String, Vec<PathBuf>>,
    keepers: &HashMap<String, PathBuf>,
) -> HashSet<PathBuf> {
    plan_resolution(conflict_map, keepers)
        .deletions
        .into_iter()
        .collect()
}

/// A fresh scan of the view's folder that keeps the checked and collapsed state of
/// every path still present.
fn rescan(view: &FileSystemView) -> io::Result<FileSystemView> {
    let node_path = |model: &FileSystemModel, id: FsNodeId| {
        model
            .get_node(id)
            .expect("iter_nodes yields valid node ids")
            .as_path()
            .as_ref()
            .to_path_buf()
    };
    let old_model = &view.file_system;
    let old_ids: HashMap<PathBuf, FsNodeId> = old_model
        .iter_nodes()
        .map(|id| (node_path(old_model, id), id))
        .collect();

    let model = FileSystemModel::new(old_model.get_root().as_path())?;
    let mut rescanned = FileSystemView::new(Arc::new(model));
    for id in rescanned.file_system.iter_nodes() {
        let Some(old_id) = old_ids.get(&node_path(&rescanned.file_system, id)) else {
            continue;
        };
        if let Some(&selected) = view.selected.get(old_id) {
            rescanned.selected.insert(id, selected);
        }
        if let Some(&collapsed) = view.collapsed.get(old_id) {
            rescanned.collapsed.insert(id, collapsed);
        }
    }
    Ok(rescanned)
}

/// The only way keepers are stored, so a group is either without a keeper or kept by
/// one of its own files. `None` clears the group's keeper.
fn set_keeper(
    conflict_map: &HashMap<String, Vec<PathBuf>>,
    keepers: &mut HashMap<String, PathBuf>,
    hash: &str,
    keeper: Option<&Path>,
) {
    let Some(keeper) = keeper else {
        keepers.remove(hash);
        return;
    };
    let is_member = conflict_map
        .get(hash)
        .is_some_and(|paths| paths.iter().any(|path| path == keeper));
    if !is_member {
        log::error!("Not keeping {keeper:?}: it is not a file of conflict {hash}");
        return;
    }
    keepers.insert(hash.to_string(), keeper.to_path_buf());
}

fn can_resolve(
    conflict_map: &HashMap<String, Vec<PathBuf>>,
    keepers: &HashMap<String, PathBuf>,
) -> bool {
    conflict_map
        .iter()
        .any(|(hash, paths)| keeper_of(paths, keepers, hash).is_some())
}

#[derive(Serialize, Deserialize)]
pub struct DuplicateFilesPane {
    pub title: Option<String>,

    #[serde(skip)]
    open_diff_popup: bool,
    #[serde(skip)]
    pub open_dir_window: bool,
    /// Root of the folder the conflict state was computed for.
    #[serde(skip)]
    conflicts_root: Option<PathBuf>,
    /// Filter the conflict state was computed with.
    #[serde(skip)]
    conflicts_filter: PathFilter,
    /// Outcome of the last Resolve, shown until closed.
    #[serde(skip)]
    resolution_summary: Option<ResolutionSummary>,
}

impl ZAppPane for DuplicateFilesPane {
    fn title(&self) -> String {
        self.title.clone().unwrap_or("File Explorer".into())
    }
}

pub struct DuplicateFilesPaneCtx<'a, 'b> {
    pub hash_service: &'a mut HashService,
    pub path_diff_view: &'a mut PathDiffView<'b>,
    pub path_filter: &'a PathFilter,

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
            conflicts_root: None,
            conflicts_filter: PathFilter::default(),
            resolution_summary: None,
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
        let checked = checked_file_hashes(view, &tree_hidden(view, ctx.path_filter), hashes);

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
        self.conflicts_root = view_root(Some(view));
        self.conflicts_filter = ctx.path_filter.clone();
        self.open_diff_popup = true;

        notice
    }

    /// Resolve All Selected: removes every non-keeper of the groups with a keeper through
    /// `delete`, then forgets the conflicts and rescans the folder.
    fn resolve_selected<E: Display>(
        &mut self,
        ctx: &mut DuplicateFilesPaneCtx,
        delete: impl FnMut(&Path) -> Result<(), E>,
    ) {
        let plan = plan_resolution(ctx.conflict_map, ctx.conflict_map_resolved);
        let summary = execute_resolution(&plan, delete);

        for path in &summary.removed {
            ctx.hash_service.remove(path);
        }
        ctx.conflict_map.clear();
        ctx.conflict_map_resolved.clear();
        *ctx.active_conflict_hash = None;
        self.open_diff_popup = false;

        if let Some(view) = ctx.path_diff_view.file_system_1_view.as_mut() {
            match rescan(view) {
                Ok(rescanned) => {
                    *view = rescanned;
                    // The two-folder rows hold node ids of the old tree.
                    *ctx.path_diff_view.visible_rows = None;
                }
                Err(e) => log::error!("Failed to rescan the folder after resolving: {e}"),
            }
        }

        self.resolution_summary = Some(summary);
    }

    /// Any click on a conflicts table row opens its detail window. Keepers are only
    /// chosen there, so a row is never checked without a keeper.
    fn click_conflict_row(&mut self, ctx: &mut DuplicateFilesPaneCtx, hash: String) {
        *ctx.active_conflict_hash = Some(hash);
    }

    /// Conflicts, keepers and the pending marks derived from them refer to the folder
    /// and filter they were computed for; drop them once a different folder (or none) is
    /// open, or the filter changed, so a resolve can't recycle a file the user no longer sees.
    fn forget_stale_conflicts(&mut self, ctx: &mut DuplicateFilesPaneCtx) {
        let root = view_root(ctx.path_diff_view.file_system_1_view.as_ref());
        if root != self.conflicts_root || *ctx.path_filter != self.conflicts_filter {
            ctx.conflict_map.clear();
            ctx.conflict_map_resolved.clear();
            *ctx.active_conflict_hash = None;
            self.conflicts_root = root;
            self.conflicts_filter = ctx.path_filter.clone();
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut DuplicateFilesPaneCtx,
    ) -> egui_tiles::UiResponse {
        self.forget_stale_conflicts(ctx);

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
                        let hidden = tree_hidden(file_system_view, ctx.path_filter);
                        for path in files_to_hash(file_system_view, &hidden) {
                            ctx.hash_service.request(path);
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

        let pending_deletion = pending_deletions(ctx.conflict_map, ctx.conflict_map_resolved);
        let mut show_diff_button = false;
        egui::ScrollArea::vertical()
            .max_height(500.0)
            .show(ui, |ui| {
                if let Some(file_system_view) = ctx.path_diff_view.file_system_1_view {
                    // Here rather than once per frame: a resolve above may have rescanned
                    // the tree, and the ids are per model.
                    let hidden = tree_hidden(file_system_view, ctx.path_filter);
                    draw_ui_folder_tree_with_checkbox(
                        ui,
                        file_system_view,
                        ctx.hash_service,
                        &pending_deletion,
                        &hidden,
                    );
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
                // The app loads the view from the root path; a view set here directly
                // would be replaced on the next frame.
                *ctx.path_diff_view.file_system_1_root_path = Some(path);
            }
        }

        egui_tiles::UiResponse::None
    }

    fn ui_popups(&mut self, ui: &mut egui::Ui, ctx: &mut DuplicateFilesPaneCtx) {
        if self.open_diff_popup {
            let mut temp_show_diff_popup = self.open_diff_popup;
            let mut did_resolve = false;
            let mut deferred_row_click: Option<String> = None;

            let mut conflicts: Vec<(String, Vec<std::path::PathBuf>, bool)> = ctx
                .conflict_map
                .iter()
                .map(|(hash, paths)| {
                    (
                        hash.clone(),
                        paths.clone(),
                        keeper_of(paths, ctx.conflict_map_resolved, hash).is_some(),
                    )
                })
                .collect();
            conflicts.sort_by(|a, b| a.0.cmp(&b.0));

            let total_conflicts = conflicts.len();
            let resolved_count = conflicts
                .iter()
                .filter(|(_, _, is_resolved)| *is_resolved)
                .count();
            let resolve_enabled = can_resolve(ctx.conflict_map, ctx.conflict_map_resolved);
            let active_hash = ctx.active_conflict_hash.clone();

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
                                            let is_active = active_hash.as_ref() == Some(hash);
                                            // Must precede the columns, which read it.
                                            row.set_selected(is_active);

                                            // Checkbox Column
                                            row.col(|ui| {
                                                let state = if *is_resolved {
                                                    CheckboxSelectState::Checked
                                                } else {
                                                    CheckboxSelectState::Unchecked
                                                };

                                                if ui_custom_checkbox(ui, state).clicked() {
                                                    deferred_row_click = Some(hash.clone());
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
                                                    deferred_row_click = Some(hash.clone());
                                                }
                                            });

                                            // Occurrences Column
                                            row.col(|ui| {
                                                let label_text = format!("{} files", paths.len());
                                                if ui
                                                    .selectable_label(is_active, label_text)
                                                    .clicked()
                                                {
                                                    deferred_row_click = Some(hash.clone());
                                                }
                                            });
                                        });
                                    });
                            });

                        ui.separator();

                        // Resolution Button
                        ui.add_enabled_ui(resolve_enabled, |ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if ui.button("Resolve All Selected").clicked() {
                                        did_resolve = true;
                                    }
                                },
                            );
                        });
                    });
                },
            );

            if let Some(hash) = deferred_row_click {
                self.click_conflict_row(ctx, hash);
            }

            self.open_diff_popup = temp_show_diff_popup;
            if did_resolve {
                self.resolve_selected(ctx, recycle);
            }
        }

        self.ui_conflict_details(ui, ctx);
        self.ui_resolution_summary(ui);
    }

    fn ui_resolution_summary(&mut self, ui: &mut egui::Ui) {
        let Some(summary) = &self.resolution_summary else {
            return;
        };
        let mut is_open = true;
        let mut close_clicked = false;
        show_custom_popup(ui.ctx(), &mut is_open, "Resolve Summary", true, |ui| {
            ui.label(format!(
                "Recycled {} files, {} failed",
                summary.removed.len(),
                summary.failed.len()
            ));
            if !summary.failed.is_empty() {
                ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for (path, error) in &summary.failed {
                            ui.label(
                                egui::RichText::new(format!("{}: {error}", path.display()))
                                    .color(ui.visuals().error_fg_color),
                            );
                        }
                    });
            }
            ui.separator();
            if ui.button("Close").clicked() {
                close_clicked = true;
            }
        });
        if !is_open || close_clicked {
            self.resolution_summary = None;
        }
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

                        let keeper =
                            keeper_of(value, ctx.conflict_map_resolved, &selected_hash).cloned();
                        let mut is_unresolved = keeper.is_none();
                        if ui
                            .radio_value(&mut is_unresolved, true, "Unresolved / None")
                            .clicked()
                        {
                            set_keeper(
                                ctx.conflict_map,
                                ctx.conflict_map_resolved,
                                &selected_hash,
                                None,
                            );
                        }

                        ui.separator();

                        egui::ScrollArea::vertical()
                            .max_height(200.0)
                            .show(ui, |ui| {
                                for path in value {
                                    let is_keeper = keeper.as_ref() == Some(path);
                                    let mut text = egui::RichText::new(path.to_string_lossy());
                                    if keeper.is_some() && !is_keeper {
                                        text = text.color(PENDING_DELETION_COLOR);
                                    }
                                    if ui.selectable_label(is_keeper, text).clicked() {
                                        set_keeper(
                                            ctx.conflict_map,
                                            ctx.conflict_map_resolved,
                                            &selected_hash,
                                            Some(path),
                                        );
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
    use std::io;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::{TempDir, tempdir};
    use zcommon::hash::hash_file_mmap;
    use zhashdiff::filter::PatternList;
    use zhashdiff::fs::FileSystemModel;

    /// Owns everything a `DuplicateFilesPaneCtx` borrows.
    struct PaneState {
        hash_service: HashService,
        root_1: Option<PathBuf>,
        root_2: Option<PathBuf>,
        view_1: Option<FileSystemView>,
        view_2: Option<FileSystemView>,
        visible_rows: Option<Vec<VisibleRowTwoFolderDiff>>,
        path_filter: PathFilter,
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
                path_filter: PathFilter::default(),
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
            self.with_ctx(|ctx| pane.diff_checked_files(ctx, hashes))
        }

        fn with_ctx<R>(&mut self, f: impl FnOnce(&mut DuplicateFilesPaneCtx) -> R) -> R {
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
                path_filter: &self.path_filter,
                active_conflict_hash: &mut self.active_conflict_hash,
                conflict_map: &mut self.conflict_map,
                conflict_map_resolved: &mut self.conflict_map_resolved,
                diff_action_pressed: &mut self.diff_action_pressed,
            };
            f(&mut ctx)
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

    fn groups(groups: &[(&str, &[&str])]) -> HashMap<String, Vec<PathBuf>> {
        groups
            .iter()
            .map(|(hash, paths)| (hash.to_string(), paths.iter().map(PathBuf::from).collect()))
            .collect()
    }

    fn keepers(keepers: &[(&str, &str)]) -> HashMap<String, PathBuf> {
        keepers
            .iter()
            .map(|(hash, path)| (hash.to_string(), PathBuf::from(path)))
            .collect()
    }

    fn paths(paths: &[&str]) -> HashSet<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn pending_deletions_without_keepers_is_empty() {
        let conflict_map = groups(&[("h1", &["a", "b"]), ("h2", &["c", "d", "e"])]);

        assert!(pending_deletions(&conflict_map, &HashMap::new()).is_empty());
    }

    #[test]
    fn pending_deletions_marks_every_member_but_the_keeper() {
        let conflict_map = groups(&[("h1", &["a", "b", "c"]), ("h2", &["d", "e"])]);

        assert_eq!(
            pending_deletions(&conflict_map, &keepers(&[("h1", "b")])),
            paths(&["a", "c"])
        );
        assert_eq!(
            pending_deletions(&conflict_map, &keepers(&[("h1", "b"), ("h2", "e")])),
            paths(&["a", "c", "d"])
        );
    }

    #[test]
    fn pending_deletions_follow_a_changed_keeper() {
        let conflict_map = groups(&[("h", &["a", "b", "c"])]);
        let mut keepers = HashMap::new();

        set_keeper(&conflict_map, &mut keepers, "h", Some(Path::new("a")));
        assert_eq!(
            pending_deletions(&conflict_map, &keepers),
            paths(&["b", "c"])
        );

        set_keeper(&conflict_map, &mut keepers, "h", Some(Path::new("c")));
        assert_eq!(
            pending_deletions(&conflict_map, &keepers),
            paths(&["a", "b"])
        );

        set_keeper(&conflict_map, &mut keepers, "h", None);
        assert!(pending_deletions(&conflict_map, &keepers).is_empty());
    }

    #[test]
    fn pending_deletions_ignore_a_keeper_outside_its_group() {
        let conflict_map = groups(&[("h", &["a", "b"])]);

        assert!(pending_deletions(&conflict_map, &keepers(&[("h", "")])).is_empty());
        assert!(pending_deletions(&conflict_map, &keepers(&[("other", "a")])).is_empty());
    }

    #[test]
    fn set_keeper_only_stores_members_of_the_group() {
        let conflict_map = groups(&[("h", &["a", "b"])]);
        let mut keepers = keepers(&[("h", "a")]);

        set_keeper(&conflict_map, &mut keepers, "h", Some(Path::new("")));
        set_keeper(
            &conflict_map,
            &mut keepers,
            "h",
            Some(Path::new("elsewhere")),
        );
        set_keeper(&conflict_map, &mut keepers, "unknown", Some(Path::new("a")));
        assert_eq!(keepers, self::keepers(&[("h", "a")]));

        set_keeper(&conflict_map, &mut keepers, "h", Some(Path::new("b")));
        assert_eq!(keepers, self::keepers(&[("h", "b")]));
    }

    #[test]
    fn resolve_needs_a_group_with_a_keeper() {
        let conflict_map = groups(&[("h1", &["a", "b"]), ("h2", &["c", "d"])]);

        assert!(!can_resolve(&conflict_map, &HashMap::new()));
        assert!(!can_resolve(&conflict_map, &keepers(&[("gone", "a")])));
        assert!(!can_resolve(&conflict_map, &keepers(&[("h1", "")])));
        assert!(can_resolve(&conflict_map, &keepers(&[("h2", "d")])));
    }

    #[test]
    fn clicking_a_conflict_row_opens_its_detail_without_storing_a_keeper() {
        let (dir, view) = duplicate_tree();
        let root = dir.path();
        let hashes = hashes_of(&[
            &root.join("a.txt"),
            &root.join("b.txt"),
            &root.join("d.txt"),
        ]);
        let mut state = PaneState::new(view);
        let mut pane = DuplicateFilesPane::new(None);
        state.press_diff(&mut pane, &hashes);
        let hash = same_hash(root);

        state.with_ctx(|ctx| pane.click_conflict_row(ctx, hash.clone()));

        assert_eq!(state.active_conflict_hash, Some(hash.clone()));
        assert!(state.conflict_map_resolved.is_empty());

        // A resolved group keeps its keeper; it is changed from the detail window only.
        state
            .conflict_map_resolved
            .insert(hash.clone(), root.join("b.txt"));
        state.active_conflict_hash = None;
        state.with_ctx(|ctx| pane.click_conflict_row(ctx, hash.clone()));

        assert_eq!(state.active_conflict_hash, Some(hash.clone()));
        assert_eq!(
            state.conflict_map_resolved,
            HashMap::from([(hash, root.join("b.txt"))])
        );
    }

    #[test]
    fn opening_a_different_folder_clears_conflicts_keepers_and_marks() {
        let (dir, view) = duplicate_tree();
        let root = dir.path();
        let hashes = hashes_of(&[&root.join("a.txt"), &root.join("b.txt")]);
        let mut state = PaneState::new(view);
        let mut pane = DuplicateFilesPane::new(None);
        state.press_diff(&mut pane, &hashes);
        let hash = same_hash(root);
        state
            .conflict_map_resolved
            .insert(hash.clone(), root.join("a.txt"));
        state.active_conflict_hash = Some(hash.clone());

        // A frame with the same folder keeps everything.
        state.with_ctx(|ctx| pane.forget_stale_conflicts(ctx));
        assert_eq!(
            state.conflict_map_resolved,
            HashMap::from([(hash.clone(), root.join("a.txt"))])
        );
        assert_eq!(state.active_conflict_hash, Some(hash));
        assert!(!pending_deletions(&state.conflict_map, &state.conflict_map_resolved).is_empty());

        let (_other_dir, other_view) = duplicate_tree();
        state.view_1 = Some(other_view);
        state.with_ctx(|ctx| pane.forget_stale_conflicts(ctx));

        assert!(state.conflict_map.is_empty());
        assert!(state.conflict_map_resolved.is_empty());
        assert_eq!(state.active_conflict_hash, None);
        assert!(pending_deletions(&state.conflict_map, &state.conflict_map_resolved).is_empty());
    }

    fn blacklist(text: &str) -> PathFilter {
        PathFilter {
            blacklist: PatternList::new(text),
            ..Default::default()
        }
    }

    #[test]
    fn request_all_hash_queues_only_the_files_the_filter_leaves_visible() {
        let (dir, _) = duplicate_tree();
        let root = dir.path();
        fs::write(root.join("notes.md"), "md").unwrap();
        let view = FileSystemView::new(Arc::new(FileSystemModel::new(root).unwrap()));
        let filter = PathFilter {
            blacklist: PatternList::new("b.txt"),
            whitelist: PatternList::new("*.txt"),
        };

        let mut queued = files_to_hash(&view, &tree_hidden(&view, &filter));
        queued.sort();

        assert_eq!(
            queued,
            [
                root.join("a.txt"),
                root.join("d.txt"),
                root.join("e.txt"),
                root.join("sub").join("c.txt"),
            ]
        );
    }

    #[test]
    fn a_hidden_file_is_left_out_of_the_groups_even_when_checked_and_hashed() {
        let dir = tempdir().unwrap();
        let (kept, hidden) = (dir.path().join("a.txt"), dir.path().join("a.obj"));
        fs::write(&kept, "same").unwrap();
        fs::write(&hidden, "same").unwrap();
        let mut view = FileSystemView::new(Arc::new(FileSystemModel::new(dir.path()).unwrap()));
        view.recursive_selection(view.file_system.get_root_node_id(), true);
        let hashes = hashes_of(&[&kept, &hidden]);
        let mut state = PaneState::new(view);
        let mut pane = DuplicateFilesPane::new(None);

        state.press_diff(&mut pane, &hashes);
        assert_eq!(
            state.conflict_map.len(),
            1,
            "without a filter they are duplicates"
        );

        state.path_filter = blacklist("*.obj");
        state.press_diff(&mut pane, &hashes);

        assert!(state.conflict_map.is_empty());
    }

    #[test]
    fn changing_the_filter_clears_conflicts_keepers_and_marks() {
        let (dir, view) = duplicate_tree();
        let root = dir.path();
        let hashes = hashes_of(&[&root.join("a.txt"), &root.join("b.txt")]);
        let mut state = PaneState::new(view);
        let mut pane = DuplicateFilesPane::new(None);
        state.path_filter = blacklist("*.obj");
        state.press_diff(&mut pane, &hashes);
        let hash = same_hash(root);
        state
            .conflict_map_resolved
            .insert(hash.clone(), root.join("a.txt"));
        state.active_conflict_hash = Some(hash.clone());

        // A frame with the same filter keeps everything.
        state.with_ctx(|ctx| pane.forget_stale_conflicts(ctx));
        assert!(!pending_deletions(&state.conflict_map, &state.conflict_map_resolved).is_empty());
        assert_eq!(state.active_conflict_hash, Some(hash));

        // b.txt, which a resolve would recycle, is hidden now.
        state.path_filter = blacklist("*.obj, b.txt");
        state.with_ctx(|ctx| pane.forget_stale_conflicts(ctx));

        assert!(state.conflict_map.is_empty());
        assert!(state.conflict_map_resolved.is_empty());
        assert_eq!(state.active_conflict_hash, None);
        assert!(pending_deletions(&state.conflict_map, &state.conflict_map_resolved).is_empty());
    }

    /// Removes files for real, except `failing`, whose removal fails and leaves it in place.
    fn remove_except(failing: &Path) -> impl FnMut(&Path) -> io::Result<()> + '_ {
        move |path| {
            if path == failing {
                Err(io::Error::other("file is in use"))
            } else {
                fs::remove_file(path)
            }
        }
    }

    #[test]
    fn resolve_removes_non_keepers_and_a_later_diff_no_longer_sees_them() {
        let (dir, view) = duplicate_tree();
        let root = dir.path();
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        let c = root.join("sub").join("c.txt");
        let hashes = hashes_of(&[&a, &b, &c, &root.join("d.txt")]);
        let mut state = PaneState::new(view);
        for path in [&a, &b, &c] {
            state.hash_service.request(path);
        }
        let mut pane = DuplicateFilesPane::new(None);
        state.press_diff(&mut pane, &hashes);
        let hash = same_hash(root);
        set_keeper(
            &state.conflict_map,
            &mut state.conflict_map_resolved,
            &hash,
            Some(&a),
        );
        state.active_conflict_hash = Some(hash.clone());

        state.with_ctx(|ctx| pane.resolve_selected(ctx, remove_except(&c)));

        let summary = pane
            .resolution_summary
            .as_ref()
            .expect("resolve leaves a summary to show");
        assert_eq!(summary.removed, vec![b.clone()]);
        assert_eq!(
            summary.failed,
            vec![(c.clone(), "file is in use".to_string())]
        );
        assert!(a.exists());
        assert!(!b.exists());
        assert!(c.exists());

        assert!(!pane.open_diff_popup);
        assert!(state.conflict_map.is_empty());
        assert!(state.conflict_map_resolved.is_empty());
        assert_eq!(state.active_conflict_hash, None);

        let known_hashes = state.hash_service.snapshot().hashes;
        assert!(!known_hashes.contains_key(&b));
        assert!(known_hashes.contains_key(&a));
        assert!(
            known_hashes.contains_key(&c),
            "a failed file keeps its hash"
        );

        let view = state.view_1.as_ref().unwrap();
        assert_eq!(view.file_system.find_path(&b), None);
        assert!(view.file_system.find_path(&c).is_some());

        // b's stale hash is still passed in: only the tree decides what is diffed.
        state.press_diff(&mut pane, &hashes);
        assert_eq!(state.conflict_map, HashMap::from([(hash, vec![a, c])]));
    }

    #[test]
    fn resolve_keeps_the_checked_and_collapsed_state_of_the_rescanned_tree() {
        let (dir, view) = duplicate_tree();
        let root = dir.path();
        let a = root.join("a.txt");
        let hashes = hashes_of(&[&a, &root.join("b.txt")]);
        let mut state = PaneState::new(view);
        let view = state.view_1.as_mut().unwrap();
        let sub_id = view.file_system.find_path(root.join("sub")).unwrap();
        view.collapsed.insert(sub_id, true);
        // Rows of the two-folder diff hold node ids of the old tree.
        state.visible_rows = Some(Vec::new());
        let mut pane = DuplicateFilesPane::new(None);
        state.press_diff(&mut pane, &hashes);
        set_keeper(
            &state.conflict_map,
            &mut state.conflict_map_resolved,
            &same_hash(root),
            Some(&a),
        );

        state.with_ctx(|ctx| pane.resolve_selected(ctx, remove_except(Path::new(""))));

        assert!(state.visible_rows.is_none());
        let view = state.view_1.as_ref().unwrap();
        let id_of = |path: PathBuf| view.file_system.find_path(path).unwrap();
        assert_eq!(view.file_system.find_path(root.join("b.txt")), None);
        assert!(view.is_collapsed(id_of(root.join("sub"))));
        assert_eq!(view.selected.get(&id_of(a.clone())), Some(&true));
        assert_eq!(view.selected.get(&id_of(root.join("d.txt"))), Some(&true));
        assert_eq!(view.selected.get(&id_of(root.join("e.txt"))), Some(&false));
    }
}
