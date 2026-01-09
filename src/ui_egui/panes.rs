use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, RwLock},
    thread::JoinHandle,
};

use eframe::egui::{self, Color32, Vec2, vec2};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};

use crate::{
    fs::{DirCache, FsEntry},
    logger::ui_log_window,
    pool::Pool,
    ui_egui::popup,
};
pub struct TreeBehavior {}

impl egui_tiles::Behavior<Pane> for TreeBehavior {
    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.title().into()
    }

    fn pane_ui(
        &mut self,
        ui: &mut egui::Ui,
        _tile_id: egui_tiles::TileId,
        pane: &mut Pane,
    ) -> egui_tiles::UiResponse {
        pane.ui(ui)
    }
}

#[derive(Serialize, Deserialize)]
pub enum Pane {
    Log(LogPane),
    FileExplorer(FileExplorerPane),
}

impl ZAppPane for Pane {
    fn title(&self) -> String {
        match self {
            Pane::Log(pane) => pane.title().into(),
            Pane::FileExplorer(p) => p.title(),
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> egui_tiles::UiResponse {
        match self {
            Pane::Log(pane) => pane.ui(ui),
            Pane::FileExplorer(p) => p.ui(ui),
        }
    }
}

pub trait ZAppPane {
    fn ui(&mut self, ui: &mut egui::Ui) -> egui_tiles::UiResponse;
    fn title(&self) -> String {
        "Pane".to_string()
    }
}

#[derive(Serialize, Deserialize)]
pub struct LogPane {
    pub title: Option<String>,
    pub log_buffer: Arc<Mutex<Vec<String>>>,
    pub scroll_to_bottom: bool, // to remove, LogPane variable
}
impl ZAppPane for LogPane {
    fn title(&self) -> String {
        self.title.clone().unwrap_or(format!("Pane"))
    }
    fn ui(&mut self, ui: &mut egui::Ui) -> egui_tiles::UiResponse {
        ui_log_window(ui, self.log_buffer.clone(), &mut self.scroll_to_bottom);
        return egui_tiles::UiResponse::None;
    }
}

const MAX_CONCURRENT_HASHES: usize = 100;
#[derive(Serialize, Deserialize)]
pub struct FileExplorerPane {
    pub title: Option<String>,

    pub root: PathBuf,

    #[serde(skip)]
    pub file_hashes: Arc<RwLock<HashMap<PathBuf, Option<String>>>>,

    #[serde(skip)]
    pub expanded: HashMap<PathBuf, bool>,
    #[serde(skip)]
    pub selected: HashMap<PathBuf, bool>,

    pub concurrent_hashes: usize,
    #[serde(skip)]
    pub hash_semaphore: Arc<Pool<Option<JoinHandle<()>>, MAX_CONCURRENT_HASHES>>,

    pub cache_enabled: bool,
    #[serde(skip)]
    pub root_dir_cache: HashMap<PathBuf, Arc<DirCache>>,

    #[serde(skip)]
    pub active_conflict_hash: Option<String>,
    #[serde(skip)]
    pub active_conflict_selected_path: Option<PathBuf>,

    #[serde(skip)]
    conflict_map: HashMap<String, Vec<PathBuf>>,
    #[serde(skip)]
    conflict_map_resolved: HashMap<String, PathBuf>,

    #[serde(skip)]
    open_diff_popup: bool,
    #[serde(skip)]
    pub open_dir_window: bool,
}

impl ZAppPane for FileExplorerPane {
    fn title(&self) -> String {
        self.title.clone().unwrap_or("File Explorer".into())
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> egui_tiles::UiResponse {
        ui.vertical(|ui| {
            self.ui_popups(ui);

            ui.horizontal(|ui| {
                if ui.button("Open Folder").clicked() {
                    self.open_dir_window = true;
                }

                let is_anything_expanded = !self.expanded.is_empty();
                let button_text = if is_anything_expanded {
                    "Collapse All"
                } else {
                    "Expand All"
                };

                if ui.button(button_text).clicked() {
                    if is_anything_expanded {
                        self.expanded.clear();
                    } else {
                        self.recursive_expand(&self.root.clone());
                    }
                }

                let cache_text = if self.cache_enabled {
                    "Disable Cache"
                } else {
                    "Enable Cache"
                };
                if ui.button(cache_text).clicked() {
                    self.cache_enabled = !self.cache_enabled;
                }

                if ui.button("Clear Hashes").clicked() {
                    self.clear_hash();
                }

                if ui.button("Request All Hash").clicked() {
                    for folder in self.root_dir_cache.values() {
                        if folder.has_files_deep {
                            for entries in &folder.entries {
                                self.request_hash(entries.path());
                            }
                        }
                    }
                }

                if ui.button("Clear Cache").clicked() {
                    self.root_dir_cache.clear();
                }
                if ui.button("Reload Root Dir").clicked() {
                    self.load_dir_recursive(&self.root.clone());
                }

                ui.label("Concurrent Hashes");
                ui.add(egui::Slider::new(
                    &mut self.concurrent_hashes,
                    0..=MAX_CONCURRENT_HASHES,
                ));
            });
        });

        ui.separator();

        egui::ScrollArea::vertical()
            .max_height(500.0)
            .show(ui, |ui| {
                if self.root.is_dir() {
                    self.ui_table(ui);
                } else {
                    ui.label("No root dir set...");
                }
            });

        if ui.button("Diff").clicked() {
            log::info!("Selected files for diff");
            self.conflict_map = self.get_conflicts_map();
            self.open_diff_popup = true;
        }

        if self.open_dir_window {
            self.open_dir_window = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                self.load_dir_recursive(&path);
                self.root = path;
            }
        }

        egui_tiles::UiResponse::None
    }
}

impl Default for FileExplorerPane {
    fn default() -> Self {
        Self {
            concurrent_hashes: 1,
            ..Default::default()
        }
    }
}

struct VisibleRow {
    path: PathBuf,
    is_dir: bool,
    depth: usize,
    parent_has_files: bool,
}

impl FileExplorerPane {
    pub fn new(title: Option<String>) -> Self {
        let new = FileExplorerPane {
            title,
            concurrent_hashes: 1,
            ..Default::default()
        };

        {
            let mut guard = new.file_hashes.write().unwrap();
            guard.retain(|_, value| value.is_some());
        }

        new
    }

    pub fn count_files_and_folders(&self) -> usize {
        if self.root.is_dir() {
            self.root_dir_cache.len() - 1
        } else {
            self.root_dir_cache.len()
        }
    }

    pub fn count_hash_queue(&self) -> usize {
        self.file_hashes
            .read()
            .unwrap()
            .values()
            .filter(|v| v.is_none())
            .count()
    }

    fn recursive_expand(&mut self, path: &PathBuf) {
        self.expanded.insert(path.clone(), true);

        if let Some(cache_entry) = self.root_dir_cache.get(path) {
            let subdirs: Vec<PathBuf> = cache_entry
                .entries
                .iter()
                .filter_map(|entry| match entry {
                    FsEntry::Dir { path: p } => Some(p.clone()),
                    _ => None,
                })
                .collect();

            for subdir in subdirs {
                self.recursive_expand(&subdir);
            }
        }
    }

    fn ui_popups(&mut self, ui: &mut egui::Ui) {
        if self.open_diff_popup {
            let mut temp_show_diff_popup = self.open_diff_popup;

            popup::show_custom_popup(
                ui.ctx(),
                &mut temp_show_diff_popup,
                "Conflicts Found",
                |ui| {
                    let mut sorted_hashes: Vec<_> = self.conflict_map.keys().cloned().collect();
                    sorted_hashes.sort();

                    if !sorted_hashes.is_empty() {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for hash in sorted_hashes {
                                let value = self.conflict_map[&hash].clone();

                                let is_resolved = self
                                    .conflict_map_resolved
                                    .iter()
                                    .find_map(|f| if *f.0 == hash { Some(f.1) } else { None })
                                    .is_some();

                                // Create a "row" that acts like a button
                                let response = ui
                                    .scope(|ui| {
                                        ui.horizontal(|ui| {
                                            self.ui_custom_checkbox(
                                                ui,
                                                match is_resolved {
                                                    true => FolderSelectState::All,
                                                    false => FolderSelectState::Partial,
                                                },
                                                &PathBuf::default(),
                                            );

                                            let color = Self::hash_to_color(&hash);

                                            egui::Frame::new()
                                                .fill(color)
                                                .corner_radius(4.0)
                                                .inner_margin(2.0)
                                                .show(ui, |ui| {
                                                    ui.monospace(
                                                        egui::RichText::new(format!(
                                                            "[{}]",
                                                            &hash[0..8]
                                                        ))
                                                        .color(Color32::BLACK)
                                                        .strong(),
                                                    );
                                                });

                                            ui.label(format!("{} duplicates", value.len()));
                                        });
                                    })
                                    .response;

                                let interact_response = ui.interact(
                                    response.rect,
                                    ui.id().with(&hash),
                                    egui::Sense::click(),
                                );
                                ui.painter().rect_filled(
                                    response.rect,
                                    0.0,
                                    ui.visuals().widgets.hovered.bg_fill.gamma_multiply(
                                        if interact_response.hovered() {
                                            0.1
                                        } else {
                                            0.0
                                        },
                                    ),
                                );

                                if interact_response.clicked() {
                                    log::info!(
                                        "Opening conflict detail popup for hash: {}",
                                        &hash[0..8]
                                    );
                                    self.active_conflict_hash = Some(hash.clone());
                                }
                            }
                        });
                    } else {
                        ui.label("No conflicts detected.");
                    }

                    let resolve_ready = self.conflict_map.len() > 0
                        && self.conflict_map.len() == self.conflict_map_resolved.len();
                    ui.add_enabled_ui(resolve_ready, |ui| {
                        if ui.button("Resolve!").clicked() {
                            log::info!("Resolving conflicts...");

                            for (hash, paths) in &self.conflict_map {
                                if let Some(path_to_keep) = self.conflict_map_resolved.get(hash) {
                                    for path in paths {
                                        if path != path_to_keep {
                                            match std::fs::remove_file(path) {
                                                Ok(_) => {
                                                    log::info!("Deleted duplicate: {:?}", path);
                                                    self.selected.remove(path);
                                                }
                                                Err(e) => {
                                                    log::error!(
                                                        "Failed to delete {:?}: {}",
                                                        path,
                                                        e
                                                    );
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            self.root_dir_cache.clear();
                            self.conflict_map.clear();
                            self.conflict_map_resolved.clear();
                            self.open_diff_popup = false;
                            self.active_conflict_hash = None;
                            self.active_conflict_selected_path = None;
                        }
                    });
                },
            );
            self.open_diff_popup = temp_show_diff_popup;
        }

        if let Some(selected_hash) = &self.active_conflict_hash {
            let mut is_open = true;
            let mut temp_is_open = is_open;
            let conflict_map = &self.conflict_map;

            if let Some(value) = conflict_map.get(selected_hash) {
                popup::show_custom_popup(
                    ui.ctx(),
                    &mut temp_is_open,
                    &format!("Conflict Detail: {}", &selected_hash[0..8]),
                    |ui| {
                        ui.label(egui::RichText::new("Select the file you wish to keep:").strong());
                        ui.add_space(8.0);

                        let mut is_unresolved =
                            !self.conflict_map_resolved.contains_key(selected_hash);
                        if ui
                            .radio_value(&mut is_unresolved, true, "Unresolved / None")
                            .clicked()
                        {
                            self.conflict_map_resolved.remove(selected_hash);
                        }

                        ui.separator();

                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for path in value {
                                let mut is_this_path_selected =
                                    self.conflict_map_resolved.get(selected_hash) == Some(path);

                                if ui
                                    .radio_value(
                                        &mut is_this_path_selected,
                                        true,
                                        path.to_string_lossy(),
                                    )
                                    .clicked()
                                {
                                    self.conflict_map_resolved
                                        .insert(selected_hash.clone(), path.clone());

                                    self.active_conflict_selected_path = Some(path.clone());
                                }
                            }
                        });

                        ui.separator();

                        ui.horizontal(|ui| {
                            if ui.button("Confirm & Close").clicked() {
                                is_open = false;
                            }

                            if ui.button("Cancel").clicked() {
                                is_open = false;
                            }
                        });
                    },
                );
            }

            is_open &= temp_is_open;
            if !is_open {
                self.active_conflict_hash = None;
                self.active_conflict_selected_path = None;
            }
        }
    }

    fn ui_table(&mut self, ui: &mut egui::Ui) {
        let mut visible_rows = Vec::new();
        self.build_visible_rows(&self.root.clone(), 0, &mut visible_rows);
        let row_count = visible_rows.len();
        let available_width = ui.available_width();

        egui::Frame::new()
            .fill(egui::Color32::from_gray(20))
            .inner_margin(0.0) // Set to 0 to prevent the table from shifting right
            .show(ui, |ui| {
                ui.set_max_width(available_width);
                let row_height = ui.text_style_height(&egui::TextStyle::Body);
                let row_height_header = ui.text_style_height(&egui::TextStyle::Heading);

                // Calculate exact width for a 64-char monospace hash + padding
                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                let dummy_hash = "321e84925aecc55ef828a41db03f0ccece66c7a6cd2a31975bcc5d029712db81";
                let galley = ui.painter().layout_no_wrap(
                    dummy_hash.into(),
                    font_id,
                    egui::Color32::PLACEHOLDER,
                );
                let min_hash_width = galley.size().x + 20.0; // Adding margin/padding

                TableBuilder::new(ui)
                    .striped(true)
                    .resizable(true)
                    .auto_shrink([false, true])
                    .column(Column::exact(32.0)) // Increased from 24.0 to prevent culling
                    .column(Column::remainder().at_least(100.0))
                    .column(
                        Column::initial(min_hash_width)
                            .at_least(min_hash_width)
                            .resizable(false),
                    )
                    .header(row_height_header, |mut header| {
                        header.col(|ui| {
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.centered_and_justified(|ui| {
                                        // Treat the root path like a folder
                                        let state = self.get_folder_selection_state(&self.root);
                                        let root_path = self.root.clone();
                                        self.ui_custom_checkbox(ui, state, &root_path);
                                    });
                                },
                            );
                        });
                        header.col(|ui| {
                            ui.with_layout(
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    ui.label("Name");
                                },
                            );
                        });
                        header.col(|ui| {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label("Hash");
                                },
                            );
                        });
                    })
                    .body(|body| {
                        body.rows(row_height, row_count, |mut row| {
                            let entry = &visible_rows[row.index()];
                            self.render_row(&mut row, entry, row_height);
                        });
                    });
            });
    }

    fn build_visible_rows(
        &mut self,
        current_path: &PathBuf,
        depth: usize,
        out: &mut Vec<VisibleRow>,
    ) {
        let dir_cache = if let Some(cache) = self.root_dir_cache.get(current_path) {
            cache.clone()
        } else {
            self.load_dir_recursive(current_path).clone()
        };

        let has_files_deep = dir_cache.has_files_deep;

        for entry in &dir_cache.entries {
            let (path, is_dir) = match entry {
                FsEntry::Dir { path } => (path, true),
                FsEntry::File { path } => (path, false),
            };

            out.push(VisibleRow {
                path: path.clone(),
                is_dir,
                depth,
                parent_has_files: has_files_deep,
            });

            if is_dir && self.expanded.get(path).copied().unwrap_or(false) {
                self.build_visible_rows(path, depth + 1, out);
            }
        }
    }

    fn render_row(&mut self, row: &mut egui_extras::TableRow, entry: &VisibleRow, row_height: f32) {
        let path = &entry.path;
        let is_dir = entry.is_dir;

        // Column 1: Checkbox
        row.col(|ui| {
            ui.centered_and_justified(|ui| {
                if entry.parent_has_files {
                    let state = if is_dir {
                        self.get_folder_selection_state(path)
                    } else {
                        if *self.selected.get(path).unwrap_or(&false) {
                            FolderSelectState::All
                        } else {
                            FolderSelectState::None
                        }
                    };
                    self.ui_custom_checkbox(ui, state, path);
                }
            });
        });

        // Column 2: Name & Expand Icon
        row.col(|ui| {
            ui.horizontal(|ui| {
                ui.add_space((entry.depth as f32) * 16.0);
                if is_dir {
                    let is_open = self.expanded.get(path).copied().unwrap_or(false);
                    let openness = if is_open { 1.0 } else { 0.0 };
                    let (_rect, response) =
                        ui.allocate_exact_size(egui::vec2(12.0, row_height), egui::Sense::click());
                    egui::collapsing_header::paint_default_icon(ui, openness, &response);

                    if response.clicked() {
                        self.expanded.insert(path.clone(), !is_open);
                    }

                    let label = format!(
                        "📁 {}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    );
                    if ui.label(label).interact(egui::Sense::click()).clicked() {
                        self.expanded.insert(path.clone(), !is_open);
                    }
                } else {
                    ui.label(path.file_name().unwrap_or_default().to_string_lossy());
                }
            });
        });

        // Column 3: Hashing
        row.col(|ui| {
            if !is_dir {
                let hash_state = {
                    let lock = self.file_hashes.read().unwrap();
                    lock.get(path).cloned()
                };

                match hash_state {
                    Some(Some(hash_str)) => {
                        let bg_color = Self::hash_to_color(&hash_str);
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
                        // This triggers the background worker
                        self.request_hash(&path.clone());
                        ui.weak("pending...");
                    }
                }
            } else {
                if let Some(cache) = self.root_dir_cache.get(path) {
                    let hashing_count = {
                        let file_hashes = self.file_hashes.read().unwrap();
                        file_hashes
                            .iter()
                            .filter(|(p, hash)| p.starts_with(path) && hash.is_none())
                            .count()
                    };
                    if hashing_count == 0 {
                        ui.weak("hashing complete!");
                    } else {
                        ui.weak(format!("hashing... {} files", hashing_count));
                    }
                }
            }
        });
    }

    fn ui_custom_checkbox(&mut self, ui: &mut egui::Ui, state: FolderSelectState, path: &PathBuf) {
        let icon_size = ui.spacing().icon_width;
        let icon_rect = egui::Vec2::splat(icon_size);

        let (rect, response) =
            ui.allocate_exact_size(ui.spacing().interact_size, egui::Sense::click());
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
                self.recursive_selection(path, new_val);
            } else {
                self.selected.insert(path.clone(), new_val);
            }
        }
    }

    fn recursive_selection(&mut self, path: &PathBuf, value: bool) {
        let cache = self.load_dir(path);

        for entry in &cache.entries {
            match entry {
                FsEntry::File { path: p } => {
                    self.selected.insert(p.clone(), value);
                }
                FsEntry::Dir { path: p } => {
                    self.recursive_selection(p, value);
                }
            }
        }
    }

    fn has_files_recursive(&self, path: &PathBuf) -> bool {
        if path.is_file() {
            return true;
        }
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                let p = entry.path();
                if self.has_files_recursive(&p) {
                    return true;
                }
            }
        }
        false
    }

    fn get_folder_selection_state(&self, path: &PathBuf) -> FolderSelectState {
        let cache = match self.root_dir_cache.get(path) {
            Some(c) => c,
            None => return FolderSelectState::None,
        };

        let mut has_selected = false;
        let mut has_unselected = false;

        for entry in &cache.entries {
            let (p, is_dir) = match entry {
                FsEntry::File { path: p } => (p, false),
                FsEntry::Dir { path: p } => (p, true),
            };

            let state = if is_dir {
                // Check if directory has files before recursing to save cycles
                if self
                    .root_dir_cache
                    .get(p)
                    .map_or(false, |c| c.has_files_deep)
                {
                    self.get_folder_selection_state(p)
                } else {
                    FolderSelectState::None
                }
            } else {
                if *self.selected.get(p).unwrap_or(&false) {
                    FolderSelectState::All
                } else {
                    FolderSelectState::None
                }
            };

            match state {
                FolderSelectState::All => has_selected = true,
                FolderSelectState::None => has_unselected = true,
                FolderSelectState::Partial => {
                    return FolderSelectState::Partial; // Early exit
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

    pub fn clear_hash(&mut self) {
        self.file_hashes.write().unwrap().clear();
        self.hash_semaphore.clear();
    }

    fn request_hash(&self, path: &PathBuf) {
        // Fast path
        {
            let read_guard = self.file_hashes.read().unwrap();
            if read_guard.contains_key(path) {
                return;
            }
        }

        let mut write_guard = self.file_hashes.write().expect("Lock failed");
        // Already queued
        if write_guard.contains_key(path) {
            return;
        }
        // Limit by user
        if self.concurrent_hashes != 0 && self.hash_semaphore.len() >= self.concurrent_hashes {
            return;
        }
        // Hard limit
        let handle = match self.hash_semaphore.allocate() {
            Some(h) => h,
            None => return,
        };

        write_guard.insert(path.clone(), None);
        drop(write_guard);

        let file_hashes = self.file_hashes.clone();
        let path_clone = path.clone();

        std::thread::spawn(move || {
            let _permit = handle;

            let hash = match crate::hash::hash_file(&path_clone.to_string_lossy()) {
                Ok(h) => Some(h),
                Err(_) => Some("error".to_string()),
            };

            if let Ok(mut w) = file_hashes.write() {
                w.insert(path_clone, hash);
            }
        });
    }

    pub fn get_conflicts_map(&self) -> HashMap<String, Vec<PathBuf>> {
        let mut groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
        let hashes = self.file_hashes.read().unwrap();

        // Instead of collect_files (disk I/O), iterate over the CACHED hashes
        for (path, hash_option) in hashes.iter() {
            if let Some(hash_str) = hash_option {
                if hash_str != "error" {
                    groups
                        .entry(hash_str.clone())
                        .or_default()
                        .push(path.clone());
                }
            }
        }

        groups.retain(|_, members| members.len() > 1);
        groups
    }

    fn hash_to_color(hash: &str) -> egui::Color32 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // 1. Strict Anchors
        let hue_digit = hash
            .chars()
            .next()
            .and_then(|c| c.to_digit(16))
            .unwrap_or(0) as f32;
        let shade_digit = hash
            .chars()
            .nth(1)
            .and_then(|c| c.to_digit(16))
            .unwrap_or(0) as f32;

        // 2. Generate Volatile Jitter
        // We use a hasher to turn the whole string into 3 different "noise" values
        let mut hasher = DefaultHasher::new();
        hash.hash(&mut hasher);
        let h = hasher.finish();

        // Create three distinct jitter values from different bits of the hash
        let jitter_h = ((h & 0xFF) as f32 / 255.0) - 0.5; // -0.5 to 0.5
        let jitter_s = (((h >> 8) & 0xFF) as f32 / 255.0) - 0.5; // -0.5 to 0.5
        let jitter_v = (((h >> 16) & 0xFF) as f32 / 255.0) - 0.5; // -0.5 to 0.5

        // 3. Apply Logic
        // HUE: Purely first letter (16 steps around the wheel)
        // We don't add jitter here to keep the "color group" perfectly consistent
        let hue = hue_digit / 16.0;

        // SATURATION & VALUE: Driven by 2nd letter, shaken by Jitter
        // Base ranges: Sat (0.4-0.8), Val (0.6-0.9)
        let s_base = 0.4 + (shade_digit / 16.0) * 0.4;
        let v_base = 0.6 + (1.0 - (shade_digit / 16.0)) * 0.3;

        // The Jitter is "volatile" because it can shift the shade by up to 20%
        let saturation = (s_base + jitter_s * 0.2).clamp(0.3, 0.95);
        let value = (v_base + jitter_v * 0.2).clamp(0.4, 0.95);

        egui::Color32::from(egui::ecolor::Hsva::new(hue, saturation, value, 1.0))
    }

    fn load_dir(&mut self, path: &PathBuf) -> Arc<DirCache> {
        if self.cache_enabled {
            if let Some(cache) = self.root_dir_cache.get(path) {
                return Arc::clone(cache);
            }
        }

        let mut entries = vec![];
        if let Ok(read_dir) = std::fs::read_dir(path) {
            for entry in read_dir.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    entries.push(FsEntry::Dir { path: p });
                } else {
                    entries.push(FsEntry::File { path: p });
                }
            }
        }

        let new_cache = Arc::new(DirCache {
            entries,
            has_files_deep: self.has_files_recursive(path),
        });

        self.root_dir_cache
            .insert(path.clone(), Arc::clone(&new_cache));
        new_cache
    }

    fn load_dir_recursive(&mut self, path: &PathBuf) -> Arc<DirCache> {
        let dir_cache = self.load_dir(path);

        if dir_cache.has_files_deep {
            for entry in dir_cache.entries.iter() {
                self.load_dir(&entry.path());
            }
        }

        dir_cache
    }
}

#[derive(PartialEq, Eq)]
enum FolderSelectState {
    None,
    All,
    Partial,
}
