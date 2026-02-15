use eframe::egui::{self, Layout, PointerButton, menu};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use zhashdiff::{fs::FileSystem, hash::HashService};

use crate::ui_egui::panes::{
    DuplicateFilesPane, DuplicateFilesPaneCtx, LogPane, Pane, PathDiffPane, PathDiffPaneCtx,
    TreeBehavior,
};
use eframe::{
    CreationContext,
    epaint::{Pos2, Vec2},
};
use egui_tiles::Tile;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppStateCtx {
    #[serde(skip)]
    pub hash_service: HashService,
    pub file_system: FileSystem,
    pub file_system_2: FileSystem,

    #[serde(skip)]
    pub expanded: HashMap<PathBuf, bool>,
    #[serde(skip)]
    pub selected: HashMap<PathBuf, bool>,
    #[serde(skip)]
    pub expanded_2: HashMap<PathBuf, bool>,
    #[serde(skip)]
    pub selected_2: HashMap<PathBuf, bool>,

    #[serde(skip)]
    pub active_conflict_hash: Option<String>,

    #[serde(skip)]
    conflict_map: HashMap<String, Vec<PathBuf>>,
    #[serde(skip)]
    conflict_map_resolved: HashMap<String, PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
enum AppState {
    Startup(AppStateCtx),
    Idle(AppStateCtx),
    Exit(AppStateCtx),
}

impl Default for AppState {
    fn default() -> Self {
        AppState::Startup(AppStateCtx::default())
    }
}

impl AppState {
    fn into_ctx(self) -> AppStateCtx {
        match self {
            AppState::Startup(ctx) => ctx,
            AppState::Idle(ctx) => ctx,
            AppState::Exit(ctx) => ctx,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct ZApp {
    monitor_size: Vec2,
    scale_factor: f32,
    native_pixel_per_point: f32,
    // Option > Hack to avoid cloning state when matching &mut self.state in update loop
    state: Option<AppState>,
    tree: egui_tiles::Tree<Pane>,
    #[serde(skip)]
    log_buffer: Arc<Mutex<Vec<String>>>,

    open_dir_window_1: bool,
    open_dir_window_2: bool,
}

const HARDCODED_MONITOR_SIZE: Vec2 = Vec2::new(2560.0, 1440.0);
impl ZApp {
    pub fn request_init(&mut self) {
        // if self.state.ctx().hash_service.is_none() {
        // self.state.ctx().hash_service = HashService::default();
        // }
    }

    pub fn new(cc: &CreationContext<'_>, log_buffer: Arc<Mutex<Vec<String>>>) -> Self {
        // Can not get window screen size from CreationContext
        let monitor_size = HARDCODED_MONITOR_SIZE;
        const RESOLUTION_REF: f32 = 1080.0;
        let scale_factor: f32 = monitor_size.x.min(monitor_size.y) / RESOLUTION_REF;

        let native_pixel_per_point = cc.egui_ctx.native_pixels_per_point().unwrap_or(1.0);

        Self {
            monitor_size: monitor_size,
            scale_factor: scale_factor,
            native_pixel_per_point: native_pixel_per_point,
            state: Some(AppState::default()),
            tree: Self::create_tree(),
            log_buffer: log_buffer,
            open_dir_window_1: false,
            open_dir_window_2: false,
        }
    }

    fn startup(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let visuals: egui::Visuals = egui::Visuals::dark();
        ctx.set_visuals(visuals);
        log::info!("pixels_per_point{:?}", ctx.pixels_per_point());
        log::info!("native_pixels_per_point{:?}", ctx.native_pixels_per_point());
        ctx.set_pixels_per_point(self.scale_factor); // Maybe mult native_pixels_per_point?
        // ctx.set_debug_on_hover(true);

        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }

    fn create_tree() -> egui_tiles::Tree<Pane> {
        let mut tiles = egui_tiles::Tiles::default();

        let mut tabs = vec![];

        let tile_console = tiles.insert_pane(Pane::Log(LogPane {
            title: Some("Log".to_string()),
            scroll_to_bottom: true,
        }));

        let tile_path_diff = tiles.insert_pane(Pane::PathDiff(PathDiffPane::new(Some(
            "Path Diff".to_string(),
        ))));

        let tile_file_explorer = tiles.insert_pane(Pane::DuplicateFiles(DuplicateFilesPane::new(
            Some("Duplicate Files".to_string()),
        )));

        // let master_tile = tiles.insert_horizontal_tile(vec![tile_file_explorer]);
        let master_tile = tiles.insert_horizontal_tile(vec![tile_path_diff]);
        tabs.push(tiles.insert_vertical_tile(vec![master_tile]));

        let root = tiles.insert_tab_tile(tabs);

        egui_tiles::Tree::new("my_tree", root, tiles)
    }

    fn show_menu(&mut self, ui: &mut egui::Ui, app_ctx: &mut AppStateCtx) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            ui.menu_button("File", |ui| {
                if ui.button("Open Folder 1").clicked() {
                    self.open_dir_window_1 = true;
                }
                if ui.button("Open Folder 2").clicked() {
                    self.open_dir_window_2 = true;
                }
            });
        });
    }

    fn ui(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame, app_ctx: &mut AppStateCtx) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.show_menu(ui, app_ctx);
            ui.separator();

            ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                let mut diff_action_triggered = false;
                let mut behavior = TreeBehavior {
                    log_buffer: self.log_buffer.clone(),
                    hash_service: &mut app_ctx.hash_service,
                    file_system: &mut app_ctx.file_system,
                    expanded: &mut app_ctx.expanded,
                    selected: &mut app_ctx.selected,
                    active_conflict_hash: &mut app_ctx.active_conflict_hash,
                    conflict_map: &mut app_ctx.conflict_map,
                    conflict_map_resolved: &mut app_ctx.conflict_map_resolved,
                    diff_action_pressed: &mut diff_action_triggered,
                    file_system_2: &mut app_ctx.file_system_2,
                    selected_2: &mut app_ctx.selected_2,
                };

                self.tree.ui(&mut behavior, ui);

                for (_tile_id, tile) in self.tree.tiles.iter() {
                    if let Tile::Pane(Pane::DuplicateFiles(_file_explorer)) = tile {
                        let out_ctx = behavior.create_duplicate_files_ctx();
                        let count_files =
                            out_ctx.file_system.count_files(&out_ctx.file_system.root);

                        let count = if out_ctx.file_system.root.is_dir() {
                            count_files - 1
                        } else {
                            count_files
                        };

                        let active = out_ctx.hash_service.count_active_hashes();
                        let total_pending = out_ctx.hash_service.count_hash_queue() + active;
                        let waiting = total_pending.saturating_sub(active);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                            "ZHashDiff - {} files/folders ({} active, {} queued)",
                            count, active, waiting
                        )));

                        break;
                    }
                }
            });
        });

        // Handle folder dialogs
        if self.open_dir_window_1 {
            self.open_dir_window_1 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                // ctx.file_system.root_dir_cache.clear();
                FileSystem::read_path_recursive_flatten(&path);
                app_ctx.file_system.root = path;
                app_ctx.expanded.clear();
            }
        }
        if self.open_dir_window_2 {
            self.open_dir_window_2 = false;
            if let Some(path) = rfd::FileDialog::new().pick_folder() {
                // ctx.file_system.root_dir_cache.clear();
                FileSystem::read_path_recursive_flatten(&path);
                app_ctx.file_system.root = path;
                app_ctx.expanded.clear();
            }
        }
    }

    fn request_shutdown(&mut self) {
        let state = self
            .state
            .take()
            .expect("state was invalid during shutdown");
        self.state = Some(AppState::Exit(state.into_ctx()));
    }

    fn process_ctx_inputs(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let mut user_quit: bool = false;
        {
            let _input_ctx = ctx.input(|r| {
                // Esc
                if r.key_down(egui::Key::Escape) {
                    user_quit = true;
                }

                // DoubleLeftClick
                if r.pointer.button_double_clicked(PointerButton::Primary) {
                    let mouse_pos = r.pointer.interact_pos().unwrap();
                    log::info!("double click @({},{})", mouse_pos.x, mouse_pos.y);
                }

                if r.pointer.button_clicked(PointerButton::Middle) {
                    let mouse_pos: Pos2 = r.pointer.interact_pos().unwrap();

                    log::info!("middle click @({},{})", mouse_pos.x, mouse_pos.y);
                }
            });
        }

        if user_quit {
            self.request_shutdown();
        }
    }
}

impl eframe::App for ZApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        log::info!("SAVING...");

        #[cfg(feature = "serde")]
        if let Ok(json) = serde_json::to_string(self) {
            log::debug!("SAVED with state: {:?}", self.state);
            storage.set_string(eframe::APP_KEY, json);
        }
        log::info!("SAVED!");
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let mut app_state = self
            .state
            .take()
            .expect("state should be valid before and after update()");
        app_state = match app_state {
            AppState::Startup(state_ctx) => {
                self.startup(ctx, frame);
                log::info!("STARTUP COMPLETE, TRANSITIONING TO IDLE");
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label("Loading...");
                    });
                });
                AppState::Idle(state_ctx)
            }
            AppState::Idle(mut state) => {
                self.ui(ctx, frame, &mut state);
                self.process_ctx_inputs(ctx, frame);
                AppState::Idle(state)
            }
            AppState::Exit(state) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                log::info!("send_viewport_cmd sent");

                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label("Exiting...");
                    });
                });
                AppState::Exit(state)
            }
        };
        self.state = Some(app_state);
    }
}
