use std::{
    collections::HashMap,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};

use eframe::egui::{self, ScrollArea, Ui};
use egui_extras::{Size, StripBuilder};
use serde::{Deserialize, Serialize};
use zhashdiff::{
    external_diff_tool::DiffToolConfig,
    fs::{FileSystem, FsEntry, FsPath},
    hash::{HashService, ResolveConflictsInput, execute_resolution, find_conflicts},
};

use crate::{
    logger::ui_log_window,
    ui_egui::{
        common::{CheckboxSelectState, hash_to_color},
        fs_tree::{
            draw_ui_folder_tree_with_checkbox, draw_ui_two_folder_tree_with_diff,
            folder_state_ui_custom_checkbox, recursive_expand,
        },
        popup,
    },
};
pub struct TreeBehavior<'a> {
    pub log_buffer: Arc<Mutex<Vec<String>>>,

    pub hash_service: &'a mut HashService,
    pub file_system: &'a mut FileSystem,
    pub file_system_2: &'a mut FileSystem,

    // User Interaction State
    pub expanded: &'a mut HashMap<PathBuf, bool>,
    pub selected: &'a mut HashMap<PathBuf, bool>,
    pub selected_2: &'a mut HashMap<PathBuf, bool>,

    // Diff Action State
    pub active_conflict_hash: &'a mut Option<String>,
    pub conflict_map: &'a mut HashMap<String, Vec<PathBuf>>,
    pub conflict_map_resolved: &'a mut HashMap<String, PathBuf>,
    pub diff_action_pressed: &'a mut bool,
    pub diff_tool_config: &'a DiffToolConfig,
}

impl TreeBehavior<'_> {
    pub fn create_path_diff_ctx(&mut self) -> PathDiffPaneCtx {
        PathDiffPaneCtx {
            hash_service: &mut self.hash_service,

            file_system_1: &mut self.file_system,
            file_system_2: &mut self.file_system_2,
            expanded: &mut self.expanded,
            selected_1: &mut self.selected,
            selected_2: &mut self.selected_2,
            diff_tool_config: &self.diff_tool_config,
        }
    }

    pub fn create_duplicate_files_ctx(&mut self) -> DuplicateFilesPaneCtx {
        DuplicateFilesPaneCtx {
            hash_service: &mut self.hash_service,
            file_system: &mut self.file_system,

            expanded: &mut self.expanded,
            selected: &mut self.selected,

            active_conflict_hash: &mut self.active_conflict_hash,
            conflict_map: &mut self.conflict_map,
            conflict_map_resolved: &mut self.conflict_map_resolved,
            diff_action_pressed: &mut self.diff_action_pressed,
        }
    }
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
        let response = match pane {
            Pane::Log(pane) => {
                let response = pane.ui(ui, &mut self.log_buffer);
                egui_tiles::UiResponse::from(response)
            }
            Pane::DuplicateFiles(pane) => {
                let mut ctx = self.create_duplicate_files_ctx();
                let response = pane.ui(ui, &mut ctx);
                egui_tiles::UiResponse::from(response)
            }
            Pane::PathDiff(path_diff_pane) => {
                let mut ctx = self.create_path_diff_ctx();
                let response = path_diff_pane.ui(ui, &mut ctx);
                egui_tiles::UiResponse::from(response)
            }
        };

        if ui
            .add(egui::Button::new("Drag me!").sense(egui::Sense::drag()))
            .drag_started()
        {
            egui_tiles::UiResponse::DragStarted
        } else {
            response
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum Pane {
    Log(LogPane),
    DuplicateFiles(DuplicateFilesPane),
    PathDiff(PathDiffPane),
}

impl Pane {
    pub fn title(&self) -> String {
        match self {
            Pane::Log(pane) => pane.title().into(),
            Pane::DuplicateFiles(p) => p.title(),
            Pane::PathDiff(p) => p.title(),
        }
    }
}

pub trait ZAppPane {
    fn title(&self) -> String {
        "Pane".to_string()
    }
}

#[derive(Serialize, Deserialize)]
pub struct PathDiffPane {
    pub title: Option<String>,

    #[serde(skip)]
    pub open_dir_window_1: bool,

    #[serde(skip)]
    pub open_dir_window_2: bool,
}

impl ZAppPane for PathDiffPane {
    fn title(&self) -> String {
        self.title.clone().unwrap_or(format!("Pane"))
    }
}

impl PathDiffPane {
    pub fn new(title: Option<String>) -> Self {
        Self {
            title,
            open_dir_window_1: false,
            open_dir_window_2: false,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut PathDiffPaneCtx) -> egui_tiles::UiResponse {
        ui.horizontal(|ui| {
            let is_anything_expanded = ctx
                .expanded
                .iter()
                .filter(|(k, _)| !k.as_os_str().is_empty()) // skip root
                .any(|(_, &v)| v);

            let button_text = if is_anything_expanded {
                "Collapse All"
            } else {
                "Expand All"
            };

            if ui.button(button_text).clicked() {
                if is_anything_expanded {
                    // Collapse all (not root)
                    for (key, value) in &mut ctx.expanded.iter_mut() {
                        // "" = root (relative)
                        if !key.as_os_str().is_empty() {
                            *value = false;
                        }
                    }
                } else {
                    // Expand all
                    recursive_expand(ctx.expanded, ctx.file_system_1, &ctx.file_system_1.root);
                    recursive_expand(ctx.expanded, ctx.file_system_2, &ctx.file_system_2.root);
                }
            }
        });

        ui.separator();

        // Table scroll area
        ScrollArea::vertical()
            .id_salt(&"path_diff_table")
            .show(ui, |ui| {
                if ctx.file_system_1.root.is_dir() {
                    draw_ui_two_folder_tree_with_diff(
                        ui,
                        &mut ctx.file_system_1.root.clone(),
                        &mut ctx.file_system_2.root.clone(),
                        &mut ctx.expanded,
                        &mut ctx.selected_1,
                        &mut ctx.file_system_1,
                        &mut ctx.file_system_2,
                        &mut self.open_dir_window_1,
                        &mut self.open_dir_window_2,
                        &ctx.diff_tool_config,
                    );
                } else {
                    ui.label("No root dir set...");
                    draw_ui_two_folder_tree_with_diff(
                        ui,
                        &mut ctx.file_system_1.root.clone(),
                        &mut ctx.file_system_2.root.clone(),
                        &mut ctx.expanded,
                        &mut ctx.selected_1,
                        &mut ctx.file_system_1,
                        &mut ctx.file_system_2,
                        &mut self.open_dir_window_1,
                        &mut self.open_dir_window_2,
                        &ctx.diff_tool_config,
                    );
                }
            });

        // Handle folder dialogs
        if self.open_dir_window_1 {
            self.open_dir_window_1 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                // ctx.file_system.root_dir_cache.clear();
                FileSystem::read_path_recursive_flatten(&path);
                ctx.file_system_1.root = path;
                ctx.expanded.clear();
            }
        }

        if self.open_dir_window_2 {
            self.open_dir_window_2 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                // ctx.file_system.root_dir_cache.clear();
                FileSystem::read_path_recursive_flatten(&path);
                ctx.file_system_2.root = path;
                ctx.expanded.clear();
            }
        }

        egui_tiles::UiResponse::None
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

pub struct PathDiffPaneCtx<'a> {
    pub hash_service: &'a mut HashService,
    pub file_system_1: &'a mut FileSystem,
    pub file_system_2: &'a mut FileSystem,

    // User Interaction State
    pub expanded: &'a mut HashMap<PathBuf, bool>,
    pub selected_1: &'a mut HashMap<PathBuf, bool>,
    pub selected_2: &'a mut HashMap<PathBuf, bool>,
    pub diff_tool_config: &'a DiffToolConfig,
}

pub struct DuplicateFilesPaneCtx<'a> {
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
                        recursive_expand(ctx.expanded, ctx.file_system, &ctx.file_system.root);
                    }
                }

                if ui.button("Request All Hash").clicked() {
                    let all_paths =
                        FileSystem::read_path_recursive_flatten(&ctx.file_system.root.clone());

                    for path in all_paths.entries {
                        if let FsEntry::File { path } = path.0 {
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
                    draw_ui_folder_tree_with_checkbox(
                        ui,
                        &ctx.file_system.root.clone(),
                        ctx.expanded,
                        ctx.selected,
                        ctx.file_system,
                        ctx.hash_service,
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

    fn ui_popups(&mut self, ui: &mut egui::Ui, ctx: &mut DuplicateFilesPaneCtx) {
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
                                                    ctx.file_system,
                                                    ctx.selected,
                                                    if *is_resolved {
                                                        CheckboxSelectState::Checked
                                                    } else {
                                                        CheckboxSelectState::Partial
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

    fn ui_conflict_details(&mut self, ui: &mut egui::Ui, ctx: &mut DuplicateFilesPaneCtx) {
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
