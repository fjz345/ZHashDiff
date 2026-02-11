use std::path::PathBuf;

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use zhashdiff::fs::FsEntry;

use crate::ui_egui::{common::hash_to_color, panes::FileExplorerPaneCtx};

pub fn draw_ui_folder_tree_with_checkbox(
    ui: &mut egui::Ui,
    ctx: &mut FileExplorerPaneCtx,
) -> egui::response::Response {
    let mut visible_rows = Vec::new();
    build_visible_rows(ctx, &ctx.file_system.root.clone(), 0, &mut visible_rows);
    let row_count = visible_rows.len();
    let available_width = ui.available_width();

    let response = egui::Frame::new()
        .fill(egui::Color32::from_gray(20))
        .inner_margin(0.0)
        .show(ui, |ui| {
            ui.set_max_width(available_width);
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
                                let state =
                                    get_folder_selection_state(ctx, &ctx.file_system.root.clone());
                                let root_path = ctx.file_system.root.clone();
                                folder_state_ui_custom_checkbox(ui, ctx, state, &root_path);
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
                        render_row(ctx, &mut row, entry, row_height);
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

fn build_visible_rows(
    ctx: &mut FileExplorerPaneCtx,
    current_path: &PathBuf,
    depth: usize,
    out: &mut Vec<VisibleRow>,
) {
    let fs_path = ctx.file_system.get(current_path);

    let has_files_deep = fs_path.has_files_deep;

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
        if is_dir && ctx.expanded.get(&path.clone()).copied().unwrap_or(false) {
            build_visible_rows(ctx, &path, depth + 1, out);
        }
    }
}

#[derive(PartialEq, Eq)]
pub enum FolderSelectState {
    None,
    All,
    Partial,
}

fn render_row(
    ctx: &mut FileExplorerPaneCtx,
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
                get_folder_selection_state(ctx, path)
            } else {
                if *ctx.selected.get(path).unwrap_or(&false) {
                    FolderSelectState::All
                } else {
                    FolderSelectState::None
                }
            };
            folder_state_ui_custom_checkbox(ui, ctx, state, path);
        });
    });

    // Column 2: Name & Expand Icon
    row.col(|ui| {
        ui.horizontal(|ui| {
            ui.add_space((entry.depth as f32) * 16.0);
            if is_dir {
                let is_open = ctx.expanded.get(path).copied().unwrap_or(false);
                let openness = if is_open { 1.0 } else { 0.0 };
                let (_rect, response) =
                    ui.allocate_exact_size(egui::vec2(12.0, row_height), egui::Sense::click());
                egui::collapsing_header::paint_default_icon(ui, openness, &response);

                if response.clicked() {
                    ctx.expanded.insert(path.clone(), !is_open);
                }

                let label = format!(
                    "📁 {}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
                if ui.label(label).interact(egui::Sense::click()).clicked() {
                    ctx.expanded.insert(path.clone(), !is_open);
                }
            } else {
                ui.label(path.file_name().unwrap_or_default().to_string_lossy());
            }
        });
    });

    // Column 3: Hash
    row.col(|ui| {
        if !is_dir {
            let hash_state = ctx.hash_service.get(path);

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
                    ctx.hash_service.request(path.clone());
                    ui.weak("pending...");
                }
            }
        } else {
            let snapshot = ctx.hash_service.snapshot();

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

pub fn folder_state_ui_custom_checkbox(
    ui: &mut egui::Ui,
    ctx: &mut FileExplorerPaneCtx,
    state: FolderSelectState,
    path: &PathBuf,
) {
    let icon_size = ui.spacing().icon_width;
    let icon_rect = egui::Vec2::splat(icon_size);

    let (rect, response) = ui.allocate_exact_size(ui.spacing().interact_size, egui::Sense::click());
    let visual_rect = egui::Rect::from_center_size(rect.center(), icon_rect);

    if ui.is_rect_visible(visual_rect) {
        let visuals = ui.style().interact(&response);
        let painter = ui.painter();
        let rounding = ui.visuals().widgets.active.corner_radius;

        // Background
        let bg_fill = if state != FolderSelectState::None {
            visuals.bg_fill
        } else {
            ui.visuals().gray_out(visuals.bg_fill)
        };
        painter.rect_filled(visual_rect, rounding, bg_fill);

        // Border
        painter.rect_stroke(
            visual_rect,
            rounding,
            visuals.bg_stroke,
            egui::StrokeKind::Middle,
        );

        let stroke = visuals.fg_stroke;
        match state {
            FolderSelectState::All => {
                let points = vec![
                    visual_rect.center() + egui::vec2(-icon_size * 0.25, 0.0),
                    visual_rect.center() + egui::vec2(-icon_size * 0.05, icon_size * 0.2),
                    visual_rect.center() + egui::vec2(icon_size * 0.3, -icon_size * 0.25),
                ];
                painter.add(egui::Shape::line(points, stroke));
            }
            FolderSelectState::Partial => {
                let dash_rect = egui::Rect::from_center_size(
                    visual_rect.center(),
                    egui::vec2(icon_size * 0.5, 2.0),
                );
                painter.rect_filled(dash_rect, 0.0, stroke.color);
            }
            FolderSelectState::None => {}
        }
    }

    if response.clicked() {
        let new_val = state != FolderSelectState::All;

        if path.is_dir() {
            recursive_selection(ctx, path, new_val);
        } else {
            ctx.selected.insert(path.clone(), new_val);
        }
    }
}

fn recursive_selection(ctx: &mut FileExplorerPaneCtx, path: &PathBuf, value: bool) {
    let fs_path = ctx.file_system.get(path);

    for entry in &fs_path.entries {
        match entry {
            FsEntry::File { path: p } => {
                ctx.selected.insert(p.clone(), value);
            }
            FsEntry::Dir { path: p } => {
                recursive_selection(ctx, p, value);
            }
        }
    }
}

fn get_folder_selection_state(ctx: &mut FileExplorerPaneCtx, path: &PathBuf) -> FolderSelectState {
    let fs_path = ctx.file_system.get(path);

    let mut has_selected = false;
    let mut has_unselected = false;

    for entry in &fs_path.entries {
        let (p, is_dir) = match entry {
            FsEntry::File { path: p } => (p, false),
            FsEntry::Dir { path: p } => (p, true),
        };

        let state = if is_dir {
            get_folder_selection_state(ctx, &p)
        } else {
            if *ctx.selected.get(p).unwrap_or(&false) {
                FolderSelectState::All
            } else {
                FolderSelectState::None
            }
        };

        match state {
            FolderSelectState::All => has_selected = true,
            FolderSelectState::None => has_unselected = true,
            FolderSelectState::Partial => {
                return FolderSelectState::Partial;
            }
        }

        if has_selected && has_unselected {
            return FolderSelectState::Partial;
        }
    }

    if has_selected {
        FolderSelectState::All
    } else {
        FolderSelectState::None
    }
}
