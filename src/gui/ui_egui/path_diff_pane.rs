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
        let has_file_system1 = ctx.path_diff_view.file_system_1.is_some();
        let has_file_system2 = ctx.path_diff_view.file_system_2.is_some();
        ui.horizontal(|ui| {
            let is_anything_expanded = ctx
                .path_diff_view
                .expanded
                .iter()
                .filter(|(k, _)| {
                    match (has_file_system1, has_file_system2) {
                        (true, true) => {
                            let file_system1 = ctx.path_diff_view.file_system_1.as_ref().unwrap();
                            let file_system2 = ctx.path_diff_view.file_system_2.as_ref().unwrap();
                            if **k == file_system1.get_root_node_id()
                                || **k == file_system2.get_root_node_id()
                            {
                                return false;
                            }
                            let node = file_system1.get_node(**k);
                            if let Some(node) = node {
                                return true;
                            }
                            let node = file_system2.get_node(**k);
                            if let Some(node) = node {
                                return true;
                            }
                            return false;
                        }
                        (true, false) => {
                            println!("todo!!");
                            return false;
                        }
                        (false, true) => {
                            println!("todo!!");
                            return false;
                        }
                        (false, false) => {
                            println!("todo!!");
                            return false;
                        }
                    };
                }) // skip root
                .any(|(_, &v)| v);

            let button_text = if is_anything_expanded {
                "Collapse All"
            } else {
                "Expand All"
            };

            if ui.button(button_text).clicked() {
                if is_anything_expanded {
                    match (has_file_system1, has_file_system2) {
                        (true, true) => {
                            let file_system1 = ctx.path_diff_view.file_system_1.as_ref().unwrap();
                            // Collapse all (not root)
                            for (key, value) in &mut ctx.path_diff_view.expanded.iter_mut() {
                                // "" = root (relative)
                                if *key == file_system1.get_root_node_id() {
                                    *value = false;
                                }
                            }
                        }
                        (true, false) => {
                            println!("todo!!");
                        }
                        (false, true) => {
                            println!("todo!!");
                        }
                        (false, false) => {
                            println!("todo!!");
                        }
                    }
                } else {
                    if has_file_system1 {
                        let root_id = ctx
                            .path_diff_view
                            .file_system_1
                            .as_ref()
                            .unwrap()
                            .get_root_node_id();
                        // Expand all
                        recursive_expand(
                            ctx.path_diff_view.expanded,
                            ctx.path_diff_view.file_system_1.as_mut().unwrap(),
                            root_id,
                        );
                    }
                    if has_file_system2 {
                        let root_id = ctx
                            .path_diff_view
                            .file_system_2
                            .as_ref()
                            .unwrap()
                            .get_root_node_id();
                        recursive_expand(
                            ctx.path_diff_view.expanded,
                            ctx.path_diff_view.file_system_2.as_mut().unwrap(),
                            root_id,
                        );
                    }
                }
            }
        });

        ui.separator();

        // Table scroll area
        ScrollArea::vertical()
            .id_salt(&"path_diff_table")
            .show(ui, |ui| {
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
            });

        // Handle folder dialogs
        if self.open_dir_window_1 {
            self.open_dir_window_1 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                // ctx.path_diff_view.file_system_1.get_root()_dir_cache.clear();
                *ctx.path_diff_view.file_system_1 = Some(FileSystemModel::new(path));
                ctx.path_diff_view.expanded.clear();
            }
        }

        if self.open_dir_window_2 {
            self.open_dir_window_2 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                // ctx.path_diff_view.file_system_1.get_root()_dir_cache.clear();
                *ctx.path_diff_view.file_system_2 = Some(FileSystemModel::new(path));
                ctx.path_diff_view.expanded.clear();
            }
        }

        egui_tiles::UiResponse::None
    }
}
