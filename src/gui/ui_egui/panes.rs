use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use eframe::egui::{self};
use serde::{Deserialize, Serialize};
use zhashdiff::{
    fs::{FileSystem, FsEntry},
    hash::{HashService, ResolveConflictsInput, execute_resolution, find_conflicts},
};

use crate::{
    logger::ui_log_window,
    ui_egui::{
        common::hash_to_color,
        fs_tree::{
            FolderSelectState, draw_ui_folder_tree_with_checkbox, folder_state_ui_custom_checkbox,
        },
        popup,
    },
};
pub struct TreeBehavior<'a> {
    pub log_buffer: Arc<Mutex<Vec<String>>>,

    pub file_explorer_ctx: FileExplorerPaneCtx<'a>,
}

impl egui_tiles::Behavior<Pane> for TreeBehavior<'_> {
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.title().into()
    }

    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile_id: egui_tiles::TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        match pane {
            Pane::Log(pane) => {
                let response = pane.ui(ui, &mut self.log_buffer);
                egui_tiles::UiResponse::from(response)
            }
            Pane::FileExplorer(pane) => {
                let mut ctx = FileExplorerPaneCtx {
                    hash_service: self.file_explorer_ctx.hash_service,
                    file_system: self.file_explorer_ctx.file_system,
                    expanded: self.file_explorer_ctx.expanded,
                    selected: self.file_explorer_ctx.selected,
                    active_conflict_hash: self.file_explorer_ctx.active_conflict_hash,
                    conflict_map: self.file_explorer_ctx.conflict_map,
                    conflict_map_resolved: self.file_explorer_ctx.conflict_map_resolved,
                    diff_action_pressed: self.file_explorer_ctx.diff_action_pressed,
                };
                let response = pane.ui(ui, &mut ctx);
                egui_tiles::UiResponse::from(response)
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum Pane {
    Log(LogPane),
    FileExplorer(DuplicateFilesPane),
}

impl Pane {
    pub fn title(&self) -> String {
        match self {
            Pane::Log(pane) => pane.title().into(),
            Pane::FileExplorer(p) => p.title(),
        }
    }
}

pub trait ZAppPane {
    fn title(&self) -> String {
        "Pane".to_string()
    }
}

#[derive(Serialize, Deserialize)]
pub struct LogPane {
    pub title: Option<String>,
    #[serde(default)]
    pub scroll_to_bottom: bool,
}
impl ZAppPane for LogPane {
    fn title(&self) -> String {
        self.title.clone().unwrap_or(format!("Pane"))
    }
}

impl LogPane {
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        log_buffer: &Arc<Mutex<Vec<String>>>,
    ) -> egui_tiles::UiResponse {
        ui_log_window(ui, log_buffer.clone(), &mut self.scroll_to_bottom);

        return egui_tiles::UiResponse::None;
    }
}

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

pub struct FileExplorerPaneCtx<'a> {
    pub hash_service: &'a mut HashService,
    pub file_system: &'a mut FileSystem,

    // User Interaction State
    pub expanded: &'a mut HashMap<PathBuf, bool>,
    pub selected: &'a mut HashMap<PathBuf, bool>,

    // Diff Action State
    pub active_conflict_hash: &'a mut Option<String>,
    pub conflict_map: &'a mut HashMap<String, Vec<PathBuf>>,
    pub conflict_map_resolved: &'a mut HashMap<String, PathBuf>,
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

    fn recursive_expand(ctx: &mut FileExplorerPaneCtx, path: &PathBuf) {
        ctx.expanded.insert(path.clone(), true);
        let path = ctx.file_system.get(path);
        for entry in path.entries.iter() {
            if let FsEntry::Dir { path } = entry {
                Self::recursive_expand(ctx, path);
            }
        }
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut FileExplorerPaneCtx,
    ) -> egui_tiles::UiResponse {
        ui.vertical(|ui| {
            self.ui_popups(ui, ctx);

            ui.horizontal(|ui| {
                if ui.button("Open Folder").clicked() {
                    self.open_dir_window = true;
                }

                let is_anything_expanded = !ctx.expanded.is_empty();
                let button_text = if is_anything_expanded {
                    "Collapse All"
                } else {
                    "Expand All"
                };

                if ui.button(button_text).clicked() {
                    if is_anything_expanded {
                        ctx.expanded.clear();
                    } else {
                        Self::recursive_expand(ctx, &ctx.file_system.root.clone());
                    }
                }

                if ui.button("Request All Hash").clicked() {
                    let all_paths =
                        FileSystem::read_path_recursive_flatten(&ctx.file_system.root.clone());

                    for path in all_paths.entries {
                        if let FsEntry::File { path } = path {
                            ctx.hash_service.request(path);
                        }
                    }
                }

                if ui.button("Clear Hashes").clicked() {
                    ctx.hash_service.clear();
                }

                // if ui.button("Reload Root Dir").clicked() {
                //     ctx.file_system
                //         .read_path_recursive_flatten(&ctx.file_system.root.clone());
                // }

                // if ui.button("Clear Cache").clicked() {
                //     ctx.file_system.root_dir_cache.clear();
                // }

                // let cache_text = if ctx.file_system.cache_enabled {
                //     "Disable Cache"
                // } else {
                //     "Enable Cache"
                // };
                // if ui.button(cache_text).clicked() {
                //     ctx.file_system.cache_enabled = !ctx.file_system.cache_enabled;
                // }

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
                if ctx.file_system.root.is_dir() {
                    draw_ui_folder_tree_with_checkbox(ui, ctx);
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
                let snapshot = ctx.hash_service.snapshot();
                *ctx.conflict_map = find_conflicts(&snapshot.hashes, &ctx.selected);
                *ctx.diff_action_pressed = true;
                self.open_diff_popup = true;
            }
        }

        if self.open_dir_window {
            self.open_dir_window = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                // ctx.file_system.root_dir_cache.clear();
                FileSystem::read_path_recursive_flatten(&path);
                ctx.file_system.root = path;
            }
        }

        egui_tiles::UiResponse::None
    }

    fn ui_popups(&mut self, ui: &mut egui::Ui, ctx: &mut FileExplorerPaneCtx) {
        if self.open_diff_popup {
            let mut temp_show_diff_popup: bool = self.open_diff_popup;
            let mut did_resolve = false;

            let mut conflicts: Vec<_> = ctx
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

            popup::show_custom_popup(ui.ctx(), &mut temp_show_diff_popup, "Conflicts", |ui| {
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
                        .inner_margin(0.0)
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

                                        row.col(|ui| {
                                            ui.centered_and_justified(|ui| {
                                                folder_state_ui_custom_checkbox(
                                                    ui,
                                                    ctx,
                                                    if *is_resolved {
                                                        FolderSelectState::All
                                                    } else {
                                                        FolderSelectState::Partial
                                                    },
                                                    &PathBuf::default(),
                                                );
                                            });
                                        });

                                        row.col(|ui| {
                                            let color = hash_to_color(hash);
                                            let response = egui::Frame::new()
                                                .fill(color)
                                                .corner_radius(4.0)
                                                .inner_margin(2.0)
                                                .show(ui, |ui| {
                                                    ui.monospace(
                                                        egui::RichText::new(&hash[0..8])
                                                            .color(egui::Color32::BLACK)
                                                            .strong(),
                                                    )
                                                })
                                                .response;

                                            if ui
                                                .interact(
                                                    response.rect.expand(10.0),
                                                    ui.id().with(hash),
                                                    egui::Sense::click(),
                                                )
                                                .clicked()
                                            {
                                                *ctx.active_conflict_hash = Some(hash.clone());
                                            }
                                        });

                                        row.col(|ui| {
                                            let label_text = format!("{} files", paths.len());
                                            if ui.selectable_label(false, label_text).clicked() {
                                                *ctx.active_conflict_hash = Some(hash.clone());
                                            }
                                        });
                                    });
                                });
                        });

                    ui.separator();

                    ui.add_enabled_ui(resolved_count > 0, |ui| {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Resolve All Selected").clicked() {
                                let resolution_input = ResolveConflictsInput {
                                    conflict_map: ctx.conflict_map.clone(),
                                    conflict_map_resolved: ctx.conflict_map_resolved.clone(),
                                };
                                let removed_files =
                                    execute_resolution(&resolution_input).removed_files;
                                if removed_files.len() > 0 {
                                    // HashService needs to remove hashes for deleted files
                                    for path in removed_files {
                                        ctx.hash_service.remove(&path);
                                    }
                                    // Directory view is now stale
                                    // ctx.file_system.root_dir_cache.clear();
                                    // Reset diff UI state
                                    ctx.conflict_map.clear();
                                    ctx.conflict_map_resolved.clear();
                                    *ctx.active_conflict_hash = None;
                                    self.open_diff_popup = false;
                                    did_resolve = true;
                                }
                            }
                        });
                    });
                });
            });

            self.open_diff_popup = temp_show_diff_popup;
            if did_resolve {
                self.open_diff_popup = false;
            }
        }

        self.ui_conflict_details(ui, ctx);
    }

    fn ui_conflict_details(&mut self, ui: &mut egui::Ui, ctx: &mut FileExplorerPaneCtx) {
        if let Some(selected_hash) = ctx.active_conflict_hash.clone() {
            let mut is_open = true;
            let mut temp_is_open = is_open;

            if let Some(value) = ctx.conflict_map.get(&selected_hash) {
                let hash_color = hash_to_color(&selected_hash);
                popup::show_custom_popup_with_color(
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
