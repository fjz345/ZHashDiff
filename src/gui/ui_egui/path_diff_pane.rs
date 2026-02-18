use eframe::egui::{self, ScrollArea};
use serde::{Deserialize, Serialize};
use zhashdiff::{external_diff_tool::DiffToolConfig, fs::FileSystemModel, hash::HashService};

use crate::ui_egui::{
    fs_tree::{draw_ui_two_folder_tree_with_diff, recursive_expand},
    panes::{PathDiffView, ZAppPane},
};

pub struct PathDiffPaneCtx<'a, 'b> {
    pub hash_service: &'a mut HashService,
    pub path_diff_view: &'a mut PathDiffView<'b>,

    // User Interaction State
    pub diff_tool_config: &'a DiffToolConfig,
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
                .path_diff_view
                .expanded
                .iter()
                .filter(|(k, _)| {
                    if **k == ctx.path_diff_view.file_system_1.get_root_node_id()
                        || **k == ctx.path_diff_view.file_system_2.get_root_node_id()
                    {
                        return false;
                    }
                    let node = ctx.path_diff_view.file_system_1.get_node(**k);
                    if let Some(node) = node {
                        return true;
                    }
                    let node = ctx.path_diff_view.file_system_2.get_node(**k);
                    if let Some(node) = node {
                        return true;
                    }
                    return false;
                }) // skip root
                .any(|(_, &v)| v);

            let button_text = if is_anything_expanded {
                "Collapse All"
            } else {
                "Expand All"
            };

            if ui.button(button_text).clicked() {
                if is_anything_expanded {
                    // Collapse all (not root)
                    for (key, value) in &mut ctx.path_diff_view.expanded.iter_mut() {
                        // "" = root (relative)
                        if *key == ctx.path_diff_view.file_system_1.get_root_node_id() {
                            *value = false;
                        }
                    }
                } else {
                    // Expand all
                    recursive_expand(
                        ctx.path_diff_view.expanded,
                        ctx.path_diff_view.file_system_1,
                        ctx.path_diff_view.file_system_1.get_root_node_id(),
                    );
                    recursive_expand(
                        ctx.path_diff_view.expanded,
                        ctx.path_diff_view.file_system_2,
                        ctx.path_diff_view.file_system_2.get_root_node_id(),
                    );
                }
            }
        });

        ui.separator();

        // Table scroll area
        ScrollArea::vertical()
            .id_salt(&"path_diff_table")
            .show(ui, |ui| {
                if ctx.path_diff_view.file_system_1.get_root().is_dir() {
                    draw_ui_two_folder_tree_with_diff(
                        ui,
                        &mut ctx.path_diff_view.expanded,
                        &mut ctx.path_diff_view.selected,
                        &mut ctx.path_diff_view.file_system_1,
                        &mut ctx.path_diff_view.file_system_2,
                        &mut self.open_dir_window_1,
                        &mut self.open_dir_window_2,
                        &ctx.diff_tool_config,
                    );
                } else {
                    ui.label("No root dir set...");
                    draw_ui_two_folder_tree_with_diff(
                        ui,
                        &mut ctx.path_diff_view.expanded,
                        &mut ctx.path_diff_view.selected,
                        &mut ctx.path_diff_view.file_system_1,
                        &mut ctx.path_diff_view.file_system_2,
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
                // ctx.path_diff_view.file_system_1.get_root()_dir_cache.clear();
                *ctx.path_diff_view.file_system_1 = FileSystemModel::new(path);
                ctx.path_diff_view.expanded.clear();
            }
        }

        if self.open_dir_window_2 {
            self.open_dir_window_2 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                // ctx.path_diff_view.file_system_1.get_root()_dir_cache.clear();
                *ctx.path_diff_view.file_system_2 = FileSystemModel::new(path);
                ctx.path_diff_view.expanded.clear();
            }
        }

        egui_tiles::UiResponse::None
    }
}
