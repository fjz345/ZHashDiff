use std::path::PathBuf;

use eframe::egui::{self, RichText, ScrollArea};
use serde::{Deserialize, Serialize};
use zhashdiff::{
    external_diff_tool::DiffToolConfig,
    filter::{PathFilter, PatternList},
};

use crate::ui_egui::{
    fs_tree::{DiffState, FileSystemView, draw_ui_two_folder_tree_with_diff},
    panes::{PathDiffView, ZAppPane},
    tree_cursor::TreeCursor,
    tree_sort::TreeSort,
};

pub struct PathDiffPaneCtx<'a, 'b> {
    pub path_diff_view: &'a mut PathDiffView<'b>,

    // User Interaction State
    pub diff_tool_config: &'a DiffToolConfig,
    pub path_filter: &'a mut PathFilter,
    pub tree_sort: &'a mut TreeSort,
    /// A widget had keyboard focus when the frame started, so the tree's keys are not
    /// for the cursor.
    pub keyboard_taken: bool,
}

#[derive(Serialize, Deserialize)]
pub struct PathDiffPane {
    pub title: Option<String>,

    #[serde(skip)]
    pub open_dir_window_1: bool,

    #[serde(skip)]
    pub open_dir_window_2: bool,

    #[serde(skip)]
    cursor: TreeCursor,
    /// Roots of the views the cursor belongs to.
    #[serde(skip)]
    cursor_roots: (Option<PathBuf>, Option<PathBuf>),
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
            cursor: TreeCursor::default(),
            cursor_roots: (None, None),
        }
    }

    /// Opening another folder on either side clears the cursor. A rescan of the same roots
    /// keeps it.
    fn forget_cursor_of_other_roots(
        &mut self,
        file_system_1_view: Option<&FileSystemView>,
        file_system_2_view: Option<&FileSystemView>,
    ) {
        let root = |view: Option<&FileSystemView>| {
            view.map(|view| view.file_system.get_root().as_path().as_ref().to_path_buf())
        };
        let roots = (root(file_system_1_view), root(file_system_2_view));
        if roots != self.cursor_roots {
            self.cursor = TreeCursor::default();
            self.cursor_roots = roots;
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut PathDiffPaneCtx) -> egui_tiles::UiResponse {
        self.forget_cursor_of_other_roots(
            ctx.path_diff_view.file_system_1_view.as_ref(),
            ctx.path_diff_view.file_system_2_view.as_ref(),
        );

        ui.horizontal(|ui| {
            let views = [
                ctx.path_diff_view.file_system_1_view.as_mut(),
                ctx.path_diff_view.file_system_2_view.as_mut(),
            ];

            let is_anything_collapsed = views.iter().flatten().any(|v| {
                v.file_system
                    .get_root()
                    .children()
                    .map_or(false, |children| v.is_anything_collapsed_slice(children))
            });

            let button_text = if is_anything_collapsed {
                "Expand All"
            } else {
                "Collapse All"
            };

            // Folding doesn't show in flat mode.
            let foldable = !ctx.tree_sort.flat;
            if ui
                .add_enabled(foldable, egui::Button::new(button_text))
                .clicked()
            {
                for v in views.into_iter().flatten() {
                    if let Some(children) = v.file_system.get_root().children() {
                        v.recursive_collapse_slice(&children.clone(), !is_anything_collapsed);
                    }
                }
            }

            if ui
                .add_enabled(foldable, egui::Button::new("Expand Diffs Only"))
                .clicked()
            {
                let views = [
                    ctx.path_diff_view.file_system_1_view.as_mut(),
                    ctx.path_diff_view.file_system_2_view.as_mut(),
                ];
                for v in views.into_iter().flatten() {
                    if let Some(children) = v.file_system.get_root().children() {
                        v.recursive_collapse_slice(&children.clone(), false);
                    }
                }
                if let Some(rows) = ctx.path_diff_view.visible_rows.as_ref() {
                    for row in rows {
                        if let DiffState::Same(id1, id2) = row.diff_state {
                            if let Some(v1) = ctx.path_diff_view.file_system_1_view.as_mut() {
                                v1.collapsed.insert(id1, true);
                            }
                            if let Some(v2) = ctx.path_diff_view.file_system_2_view.as_mut() {
                                v2.collapsed.insert(id2, true);
                            }
                        }
                    }
                }
            }
        });

        ui.horizontal(|ui| {
            draw_filter_bar(ui, ctx.path_filter);
            ui.separator();
            // Like the filter, the app reorders the rows on the next update.
            ui.checkbox(&mut ctx.tree_sort.flat, "Flat")
                .on_hover_text("List all entries by depth, then by the sorted column");
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
                    &mut ctx.path_diff_view.visible_rows,
                    &mut self.open_dir_window_1,
                    &mut self.open_dir_window_2,
                    &ctx.diff_tool_config,
                    &mut self.cursor,
                    ctx.tree_sort,
                    ctx.keyboard_taken,
                );
            });

        // Handle folder dialogs
        if self.open_dir_window_1 {
            self.open_dir_window_1 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                *ctx.path_diff_view.file_system_1_root_path = Some(path);
            }
        }
        if self.open_dir_window_2 {
            self.open_dir_window_2 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                *ctx.path_diff_view.file_system_2_root_path = Some(path);
            }
        }

        egui_tiles::UiResponse::None
    }
}

/// Only edits the filter: the app rebuilds the rows on the next update when it changed, so the
/// table drawn below still has this frame's rows.
fn draw_filter_bar(ui: &mut egui::Ui, filter: &mut PathFilter) {
    ui.label("Blacklist:");
    draw_pattern_field(ui, &mut filter.blacklist, "*.obj, target/, .git");
    ui.label("Whitelist:");
    draw_pattern_field(ui, &mut filter.whitelist, "*.rs, *.toml");
    let has_text = *filter != PathFilter::default();
    if ui
        .add_enabled(has_text, egui::Button::new("Clear"))
        .clicked()
    {
        *filter = PathFilter::default();
        ui.ctx().request_repaint();
    }
    if filter.is_active() {
        ui.label(RichText::new("Filter active").color(ui.visuals().warn_fg_color));
    }
}

fn draw_pattern_field(ui: &mut egui::Ui, list: &mut PatternList, hint: &str) {
    let mut text = list.text().to_owned();
    let edit = egui::TextEdit::singleline(&mut text)
        .hint_text(hint)
        .desired_width(300.0);
    if ui.add(edit).changed() {
        *list = PatternList::new(text);
        ui.ctx().request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::PathDiffPane;
    use crate::ui_egui::{fs_tree::FileSystemView, tree_cursor::TreeCursor};
    use std::{path::Path, sync::Arc};
    use tempfile::tempdir;
    use zhashdiff::fs::FileSystemModel;

    fn load_view(root: &Path) -> FileSystemView {
        FileSystemView::new(Arc::new(FileSystemModel::new(root).unwrap()))
    }

    #[test]
    fn opening_another_folder_on_either_side_clears_the_cursor() {
        let (a, b, c) = (tempdir().unwrap(), tempdir().unwrap(), tempdir().unwrap());
        let mut pane = PathDiffPane::new(None);
        pane.forget_cursor_of_other_roots(Some(&load_view(a.path())), Some(&load_view(b.path())));

        pane.cursor = TreeCursor::at("x");
        pane.forget_cursor_of_other_roots(Some(&load_view(a.path())), Some(&load_view(c.path())));
        assert_eq!(pane.cursor, TreeCursor::default(), "right side changed");

        pane.cursor = TreeCursor::at("x");
        pane.forget_cursor_of_other_roots(Some(&load_view(b.path())), Some(&load_view(c.path())));
        assert_eq!(pane.cursor, TreeCursor::default(), "left side changed");

        pane.cursor = TreeCursor::at("x");
        pane.forget_cursor_of_other_roots(Some(&load_view(b.path())), None);
        assert_eq!(pane.cursor, TreeCursor::default(), "right side closed");
    }

    #[test]
    fn rescanning_the_same_folders_keeps_the_cursor() {
        let (a, b) = (tempdir().unwrap(), tempdir().unwrap());
        let mut pane = PathDiffPane::new(None);
        pane.forget_cursor_of_other_roots(Some(&load_view(a.path())), Some(&load_view(b.path())));
        pane.cursor = TreeCursor::at("x");

        // New models of the same roots, as a rescan makes.
        pane.forget_cursor_of_other_roots(Some(&load_view(a.path())), Some(&load_view(b.path())));
        assert_eq!(pane.cursor, TreeCursor::at("x"));
    }
}
