use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    path::PathBuf,
    rc::Rc,
    str::FromStr,
    sync::{Arc, Mutex, RwLock},
    thread,
};

use eframe::{
    egui::{self, Color32},
    epaint::tessellator::Path,
};
use egui_extras::{Column, TableBuilder};
use serde::{Deserialize, Serialize};

use crate::{
    fs::{DirCache, FsEntry},
    logger::ui_log_window,
    ui_egui::{app::ZColorPickerAppContext, popup},
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
    fn update_ctx(&mut self, new_ctx: Rc<RefCell<ZColorPickerAppContext>>) {
        match self {
            Pane::Log(pane) => pane.update_ctx(new_ctx),
            Pane::FileExplorer(p) => p.update_ctx(new_ctx),
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
    fn update_ctx(&mut self, new_ctx: Rc<RefCell<ZColorPickerAppContext>>);
    fn title(&self) -> String {
        "Pane".to_string()
    }
    fn post_draw(&mut self, ui: &mut egui::Ui) -> egui_tiles::UiResponse {
        let color = egui::epaint::Hsva::new(0.103 as f32, 0.5, 0.5, 1.0);
        ui.painter().rect_filled(ui.max_rect(), 0.0, color);
        let dragged = ui
            .allocate_rect(ui.max_rect(), egui::Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::Grab)
            .dragged();
        if dragged {
            egui_tiles::UiResponse::DragStarted
        } else {
            egui_tiles::UiResponse::None
        }
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

    fn update_ctx(&mut self, _new_ctx: Rc<RefCell<ZColorPickerAppContext>>) {}
}

#[derive(Serialize, Deserialize, Default)]
pub struct FileExplorerPane {
    pub title: Option<String>,

    pub root: PathBuf,
    pub expanded: HashMap<PathBuf, bool>,

    pub selected: HashMap<PathBuf, bool>,
    pub file_hashes: Arc<RwLock<HashMap<PathBuf, Option<String>>>>,

    #[serde(skip)]
    pub cache: HashMap<PathBuf, DirCache>,
    #[serde(skip)]
    pub selected_conflict_path: Option<PathBuf>,
    #[serde(skip)]
    pub active_conflict_hash: Option<String>,
    #[serde(skip)]
    show_diff_popup: bool,
    #[serde(skip)]
    conflict_map: HashMap<String, (Vec<PathBuf>, bool)>,
    #[serde(skip)]
    conflict_map_resolved: HashMap<String, PathBuf>,
    #[serde(skip)]
    pub open_path_dialog: bool,
}

impl ZAppPane for FileExplorerPane {
    fn title(&self) -> String {
        self.title.clone().unwrap_or("File Explorer".into())
    }

    fn ui(&mut self, ui: &mut egui::Ui) -> egui_tiles::UiResponse {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.vertical(|ui| {
                self.ui_popups(ui);

                if ui.button("Open Folder").clicked() {
                    self.open_path_dialog = true;
                }

                if self.open_path_dialog {
                    self.open_path_dialog = false;

                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        self.root = path;
                    }
                }

                self.ui_table(ui, &self.root.clone());

                if ui.button("Diff").clicked() {
                    log::info!("Selected files for diff");

                    self.conflict_map = self.get_conflicts_map();
                    self.show_diff_popup = true;
                }
            });
        });

        egui_tiles::UiResponse::None
    }

    fn update_ctx(&mut self, _new_ctx: Rc<RefCell<ZColorPickerAppContext>>) {}
}

impl FileExplorerPane {
    pub fn new(title: Option<String>) -> Self {
        FileExplorerPane {
            title,
            ..Default::default()
        }
    }

    fn ui_popups(&mut self, ui: &mut egui::Ui) {
        if self.show_diff_popup {
            let mut temp_show_diff_popup = self.show_diff_popup;

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
                                            // 1. Visual-only Radio/Checkbox
                                            self.ui_custom_checkbox(
                                                ui,
                                                match is_resolved {
                                                    true => FolderSelectState::All,
                                                    false => FolderSelectState::Partial,
                                                },
                                                &PathBuf::default(),
                                            );

                                            // 2. Hash with Color-coded Background
                                            let color = Self::hash_to_color(&hash);

                                            // We use a Frame to draw a "pill" or "tag" behind the hash
                                            egui::Frame::new()
                                                .fill(color) // Soften the background
                                                .corner_radius(4.0)
                                                .inner_margin(2.0)
                                                .show(ui, |ui| {
                                                    ui.monospace(
                                                        egui::RichText::new(format!(
                                                            "[{}]",
                                                            &hash[0..8]
                                                        ))
                                                        .color(Color32::BLACK) // Keep text readable
                                                        .strong(),
                                                    );
                                                });

                                            // 3. Duplicate Count
                                            ui.label(format!("{} duplicates", value.0.len()));
                                        });
                                    })
                                    .response;

                                // Make the whole row area interactive
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
                        if ui.button("Resolve!").clicked() && resolve_ready {
                            log::info!("Resolving conflicts...");

                            for (hash, (paths, _)) in &self.conflict_map {
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

                            self.conflict_map.clear();
                            self.conflict_map_resolved.clear();
                            self.show_diff_popup = false;

                            self.load_dir(&self.root.clone());
                        }
                    });
                },
            );
            self.show_diff_popup = temp_show_diff_popup;
        }

        if let Some(ref selected_hash) = self.active_conflict_hash {
            let mut is_open = true;
            let mut temp_is_open = is_open;
            let conflict_map = self.get_conflicts_map();

            if let Some(value) = conflict_map.get(selected_hash) {
                popup::show_custom_popup(
                    ui.ctx(),
                    &mut temp_is_open,
                    &format!("Conflict Detail: {}", &selected_hash[0..8]),
                    |ui| {
                        ui.label(egui::RichText::new("Select the file you wish to keep:").strong());
                        ui.add_space(8.0);

                        // --- 1. THE "NOT RESOLVED" OPTION ---
                        // We check if the map currently has NO entry for this hash
                        let mut is_unresolved =
                            !self.conflict_map_resolved.contains_key(selected_hash);
                        if ui
                            .radio_value(&mut is_unresolved, true, "Unresolved / None")
                            .clicked()
                        {
                            self.conflict_map_resolved.remove(selected_hash);
                            // Also mark the boolean in conflict_map as false
                            if let Some(entry) = self.conflict_map.get_mut(selected_hash) {
                                entry.1 = false;
                            }
                        }

                        ui.separator();

                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for path in &value.0 {
                                // Check if this specific path is the one currently in the resolved map
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
                                    // Update the resolved map
                                    self.conflict_map_resolved
                                        .insert(selected_hash.clone(), path.clone());

                                    // Update the "resolved" status bit in your conflict_map
                                    if let Some(entry) = self.conflict_map.get_mut(selected_hash) {
                                        entry.1 = true;
                                    }

                                    // Set the temporary tracking variable used by the confirm button
                                    self.selected_conflict_path = Some(path.clone());
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
                self.selected_conflict_path = None;
            }
        }
    }

    fn ui_table(&mut self, ui: &mut egui::Ui, _path: &PathBuf) {
        let available_width = ui.available_width();

        egui::Frame::none()
            .fill(egui::Color32::from_gray(20))
            .inner_margin(0.0) // Set to 0 to prevent the table from shifting right
            .show(ui, |ui| {
                ui.set_max_width(available_width);
                self.ui_dir(ui, &self.root.clone());
            });
    }

    fn ui_dir(&mut self, ui: &mut egui::Ui, path: &PathBuf) {
        let row_height = ui.text_style_height(&egui::TextStyle::Body);
        let row_height_header = ui.text_style_height(&egui::TextStyle::Heading);

        // Calculate exact width for a 64-char monospace hash + padding
        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
        let dummy_hash = "321e84925aecc55ef828a41db03f0ccece66c7a6cd2a31975bcc5d029712db81";
        let galley =
            ui.painter()
                .layout_no_wrap(dummy_hash.into(), font_id, egui::Color32::TEMPORARY_COLOR);
        let min_hash_width = galley.size().x + 20.0; // Adding margin/padding

        TableBuilder::new(ui)
            .striped(true)
            .resizable(true)
            .auto_shrink([false, true])
            .column(Column::exact(32.0)) // Increased from 24.0 to prevent culling
            .column(Column::remainder().at_least(100.0))
            // HASH: Fixed minimum size, anchored to the right edge.
            .column(
                Column::initial(min_hash_width)
                    .at_least(min_hash_width)
                    .resizable(false),
            )
            // Inside ui_dir(ui, path)
            .header(row_height_header, |mut header| {
                header.col(|ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.centered_and_justified(|ui| {
                            // Treat the root path exactly like a folder
                            let state = self.get_folder_selection_state(&self.root);
                            let root_path = self.root.clone();
                            self.ui_custom_checkbox(ui, state, &root_path);
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
            .body(|mut body| {
                self.render_tree_level(&mut body, path.clone(), 0, row_height);
            });
    }

    fn render_tree_level(
        &mut self,
        body: &mut egui_extras::TableBody,
        parent_path: PathBuf,
        depth: usize,
        row_height: f32,
    ) {
        self.load_dir(&parent_path);

        let entries = self
            .cache
            .get(&parent_path)
            .map(|c| c.entries.clone())
            .unwrap_or_default();

        for entry in entries {
            let entry_path = match &entry {
                FsEntry::Dir { path } => path.clone(),
                FsEntry::File { path } => path.clone(),
            };
            let is_dir = matches!(entry, FsEntry::Dir { .. });

            body.row(row_height, |mut row| {
                row.col(|ui| {
                    ui.centered_and_justified(|ui| {
                        // Direct disk check is safer than cache check if cache is lazily populated
                        let has_files = if is_dir {
                            self.has_files_recursive(&entry_path)
                        } else {
                            true // It's a file, so it's a "file"
                        };

                        if has_files {
                            let state = if is_dir {
                                self.get_folder_selection_state(&entry_path)
                            } else {
                                if *self.selected.get(&entry_path).unwrap_or(&false) {
                                    FolderSelectState::All
                                } else {
                                    FolderSelectState::None
                                }
                            };
                            self.ui_custom_checkbox(ui, state, &entry_path);
                        }
                    });
                });

                // COLUMN 2: Name & Toggle
                row.col(|ui| {
                    ui.horizontal(|ui| {
                        ui.add_space((depth as f32) * 16.0);
                        if is_dir {
                            let is_open = self.expanded.get(&entry_path).copied().unwrap_or(false);
                            let openness = if is_open { 1.0 } else { 0.0 };
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(12.0, row_height),
                                egui::Sense::click(),
                            );

                            egui::collapsing_header::paint_default_icon(ui, openness, &response);

                            if response.clicked() {
                                self.expanded.insert(entry_path.clone(), !is_open);
                            }

                            let label_text = format!(
                                "📁 {}",
                                entry_path.file_name().unwrap_or_default().to_string_lossy()
                            );
                            if ui
                                .label(label_text)
                                .interact(egui::Sense::click())
                                .clicked()
                            {
                                self.expanded.insert(entry_path.clone(), !is_open);
                            }
                        } else {
                            ui.label(entry_path.file_name().unwrap_or_default().to_string_lossy());
                        }
                    });
                });

                // COLUMN 3: Hash
                row.col(|ui| {
                    if !is_dir {
                        let hash_state = self.file_hashes.read().unwrap().get(&entry_path).cloned();
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
                                                .color(egui::Color32::BLACK), // Contrast text
                                        );
                                    });
                            }
                            Some(None) => {
                                ui.weak("hashing...");
                            }
                            None => {
                                self.request_hash(entry_path.clone());
                                ui.weak("pending...");
                            }
                        }
                    }
                });
            });

            if is_dir && self.expanded.get(&entry_path).copied().unwrap_or(false) {
                self.render_tree_level(body, entry_path, depth + 1, row_height);
            }
        }
    }

    fn ui_custom_checkbox(&mut self, ui: &mut egui::Ui, state: FolderSelectState, path: &PathBuf) {
        // 1. Use icon_width for the visual box size to match standard checkboxes
        let icon_size = ui.spacing().icon_width;
        let icon_rect = egui::Vec2::splat(icon_size);

        // 2. Allocate the full interactive area but center the visual icon within it
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
                    // Standard checkmark
                    let mut points = vec![
                        visual_rect.center() + egui::vec2(-icon_size * 0.25, 0.0),
                        visual_rect.center() + egui::vec2(-icon_size * 0.05, icon_size * 0.2),
                        visual_rect.center() + egui::vec2(icon_size * 0.3, -icon_size * 0.25),
                    ];
                    painter.add(egui::Shape::line(points, stroke));
                }
                FolderSelectState::Partial => {
                    // Centered Dash
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
            if path.is_dir() || path == &self.root {
                self.set_selection_recursive(path, new_val);
            } else {
                self.selected.insert(path.clone(), new_val);
            }
        }
    }

    /// Recursively sets the selection state for a folder and all its contents
    fn set_selection_recursive(&mut self, path: &PathBuf, value: bool) {
        // We must ensure the directory is loaded to know what's inside
        self.load_dir(path);

        if let Some(cache) = self.cache.get(path).cloned() {
            for entry in &cache.entries {
                match entry {
                    FsEntry::File { path: p } => {
                        self.selected.insert(p.clone(), value);
                    }
                    FsEntry::Dir { path: p } => {
                        self.set_selection_recursive(p, value);
                    }
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
                if p.is_file() || self.has_files_recursive(&p) {
                    return true;
                }
            }
        }
        false
    }

    /// Determines if a folder is empty, all selected, or partially selected
    fn get_folder_selection_state(&self, path: &PathBuf) -> FolderSelectState {
        let cache = match self.cache.get(path) {
            Some(c) => c,
            None => return FolderSelectState::None,
        };

        if cache.entries.is_empty() {
            return FolderSelectState::None;
        }

        let mut has_selected = false;
        let mut has_unselected = false;

        // Note: This only checks one level deep for simplicity in this helper,
        // but for a truly intuitive UI, it should recurse.
        for entry in &cache.entries {
            match entry {
                FsEntry::File { path: p } => {
                    if *self.selected.get(p).unwrap_or(&false) {
                        has_selected = true;
                    } else {
                        has_unselected = true;
                    }
                }
                FsEntry::Dir { path: p } => match self.get_folder_selection_state(p) {
                    FolderSelectState::All => has_selected = true,
                    FolderSelectState::None => has_unselected = true,
                    FolderSelectState::Partial => {
                        has_selected = true;
                        has_unselected = true;
                    }
                },
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

    fn request_hash(&self, path: PathBuf) {
        let mut w = self
            .file_hashes
            .write()
            .expect("Failed to lock for writing");

        // IMPORTANT: If we are already hashing this or it's done, do nothing
        if w.contains_key(&path) {
            return;
        }

        // Mark as "None" (meaning: Hashing in progress)
        w.insert(path.clone(), None);

        let file_hashes = self.file_hashes.clone();
        std::thread::spawn(move || {
            // Actual file I/O and hashing
            let hash = match crate::hash::hash_file(&path.to_string_lossy()) {
                Ok(h) => Some(h),
                Err(_) => Some("error".to_string()),
            };

            if let Ok(mut w) = file_hashes.write() {
                w.insert(path, hash);
            }
        });
    }

    /// Returns a list of vectors, where each sub-vector contains paths to files
    /// that are both selected and have identical hashes.
    pub fn get_conflicts_map(&self) -> HashMap<String, (Vec<PathBuf>, bool)> {
        let mut groups: HashMap<String, (Vec<PathBuf>, bool)> = HashMap::new();
        let hashes = self.file_hashes.read().unwrap();

        // Helper closure for recursive walking
        fn collect_files(path: &PathBuf, all_files: &mut Vec<PathBuf>) {
            if path.is_file() {
                all_files.push(path.clone());
            } else if path.is_dir() {
                if let Ok(entries) = std::fs::read_dir(path) {
                    for entry in entries.flatten() {
                        collect_files(&entry.path(), all_files);
                    }
                }
            }
        }

        let mut all_discovered_paths = Vec::new();
        collect_files(&self.root, &mut all_discovered_paths);

        for path in all_discovered_paths {
            // We only care about files that have been hashed
            if let Some(Some(hash_str)) = hashes.get(&path) {
                groups.entry(hash_str.clone()).or_default().0.push(path);
            }
        }

        // Retain only groups that actually have duplicates
        groups.retain(|_, members| members.0.len() > 1);
        groups
    }

    fn hash_to_color(hash: &str) -> egui::Color32 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        hash.hash(&mut hasher);
        let h = hasher.finish();

        // Map the 64-bit hash to a float between 0.0 and 1.0 for Hue
        let hue = (h % 360) as f32 / 360.0;

        // We use a high Value (brightness) so that black text remains readable
        // and a moderate Saturation so colors aren't too aggressive.
        egui::Color32::from(egui::ecolor::Hsva::new(hue, 0.5, 0.8, 1.0))
    }

    fn load_dir(&mut self, path: &PathBuf) {
        // if self.cache.contains_key(path) {
        //     return;
        // }

        let mut entries = vec![];
        if let Ok(read_dir) = fs::read_dir(path) {
            for entry in read_dir.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    entries.push(FsEntry::Dir { path: p });
                } else {
                    entries.push(FsEntry::File { path: p });
                }
            }
        }

        self.cache.insert(
            path.clone(),
            DirCache {
                entries,
                has_files_deep: self.has_files_recursive(&path),
            },
        );
    }
}

#[derive(PartialEq, Eq)]
enum FolderSelectState {
    None,
    All,
    Partial,
}
