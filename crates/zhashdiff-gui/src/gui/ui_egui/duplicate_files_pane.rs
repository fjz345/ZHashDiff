use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use eframe::egui;
use serde::Deserialize;
use serde::Serialize;
use zcommon::hash::HashService;
use zcommon::ui_egui::common::show_custom_popup;
use zcommon::ui_egui::common::show_custom_popup_with_color;
use zhashdiff::conflict::ResolveConflictsInput;
use zhashdiff::conflict::execute_resolution;
use zhashdiff::fs::FileSystemModel;

use crate::ui_egui::fs_tree::FileSystemView;
use crate::ui_egui::fs_tree::draw_ui_folder_tree_with_checkbox;
use crate::ui_egui::panes::PathDiffView;
use crate::ui_egui::panes::ZAppPane;
use zcommon::ui_egui::common::CheckboxSelectState;
use zcommon::ui_egui::common::hash_to_color;
use zcommon::ui_egui::common::ui_custom_checkbox;

const MAX_CONCURRENT_HASHES: usize = 16;
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
                log::info!("Selected files for diff");
                todo!();
                // let snapshot = ctx.hash_service.snapshot();
                // *ctx.conflict_map = find_conflicts(&snapshot.hashes, &ctx.path_diff_view.selected);
                // *ctx.diff_action_pressed = true;
                // self.open_diff_popup = true;
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
            let resolved_count = ctx.conflict_map_resolved.len();

            show_custom_popup(ui.ctx(), &mut temp_show_diff_popup, "Conflicts", |ui| {
                ui.vertical(|ui| {
                    ui.label(format!(
                        "Conflicts: ({}/{})",
                        resolved_count, total_conflicts
                    ));
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
                                            if ui.selectable_label(false, label_text).clicked() {
                                                deferred_hash_toggle = Some(hash.clone());
                                            }
                                        });
                                    });
                                });
                        });

                    ui.separator();

                    // Resolution Button
                    ui.add_enabled_ui(resolved_count > 0, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Resolve All Selected").clicked() {
                                let resolution_input = ResolveConflictsInput {
                                    conflict_map: ctx.conflict_map.clone(),
                                    conflict_map_resolved: ctx.conflict_map_resolved.clone(),
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
                        });
                    });
                });
            });

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
