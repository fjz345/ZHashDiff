use std::sync::Arc;

use eframe::egui::{self, ScrollArea};
use serde::{Deserialize, Serialize};
use zhashdiff::{external_diff_tool::DiffToolConfig, fs::FileSystemModel};

use crate::ui_egui::{
    fs_tree::{FileSystemView, draw_ui_two_folder_tree_with_diff},
    panes::{PathDiffView, ZAppPane},
};

pub struct PathDiffPaneCtx<'a, 'b> {
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
            let text_expand_all = "Expand All";
            let text_collapse_all = "Collapse All";
            match (&mut ctx.path_diff_view.file_system_1_view, &mut ctx.path_diff_view.file_system_2_view) 
            {
                (None, None) => {
                    let _ = ui.button(text_collapse_all);
                },
                (None, Some(fs_view)) | (Some(fs_view), None)  => {
                    let nodes_considered_for_collapse = &fs_view.file_system.get_root().children().unwrap().clone();
                    let is_anything_collapsed = fs_view.is_anything_collapsed_slice(nodes_considered_for_collapse);
                    let button_text = if is_anything_collapsed {
                        text_expand_all
                    } else {
                        text_collapse_all
                    };

                    if ui.button(button_text).clicked() {
                        fs_view.recursive_collapse_slice(nodes_considered_for_collapse, !is_anything_collapsed);
                    }
                },
                (Some(fs_1_view), Some(fs_2_view)) => {
                    let nodes_considered_for_collapse_1 = &fs_1_view.file_system.get_root().children().unwrap().clone();
                    let nodes_considered_for_collapse_2 = &fs_2_view.file_system.get_root().children().unwrap().clone();
                    let is_anything_collapsed = fs_1_view.is_anything_collapsed_slice(nodes_considered_for_collapse_1) || fs_2_view.is_anything_collapsed_slice(nodes_considered_for_collapse_2);
                    let button_text = if is_anything_collapsed {
                        text_expand_all
                    } else {
                        text_collapse_all
                    };

                    if ui.button(button_text).clicked() {
                        fs_1_view.recursive_collapse_slice(nodes_considered_for_collapse_1, !is_anything_collapsed);
                        fs_2_view.recursive_collapse_slice(nodes_considered_for_collapse_2, !is_anything_collapsed);
                    }
                },
            }
        });

        ui.separator();

        // Table scroll area
        ScrollArea::vertical()
            .id_salt(&"path_diff_table")
            .show(ui, |ui| {
                draw_ui_two_folder_tree_with_diff(
                    ui,
                    &mut ctx.path_diff_view.file_system_1_view,
                    &mut ctx.path_diff_view.file_system_2_view,
                    &mut self.open_dir_window_1,
                    &mut self.open_dir_window_2,
                    &ctx.diff_tool_config,
                );
            });

        // Handle folder dialogs
        if self.open_dir_window_1 {
            self.open_dir_window_1 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                *ctx.path_diff_view.file_system_1_view =
                    Some(FileSystemView::new(Arc::new(FileSystemModel::new(path))));
            }
        }
        if self.open_dir_window_2 {
            self.open_dir_window_2 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                *ctx.path_diff_view.file_system_2_view =
                    Some(FileSystemView::new(Arc::new(FileSystemModel::new(path))));
            }
        }

        egui_tiles::UiResponse::None
    }
}
