use eframe::egui::{self, Layout, PointerButton, TextBuffer};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use zhashdiff::{
    external_diff_tool::{DiffToolConfig, DiffToolDefaultArgs},
    fs::{FileSystemModel, FsNodeId},
    hash::HashService,
};

use crate::ui_egui::{
    panes::{DuplicateFilesPane, LogPane, Pane, PathDiffPane, TreeBehavior},
    popup::show_custom_popup,
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
    pub file_system_model_1: FileSystemModel,
    pub file_system_model_2: FileSystemModel,

    #[serde(skip)]
    pub expanded: HashMap<FsNodeId, bool>,
    #[serde(skip)]
    pub selected_1: HashMap<FsNodeId, bool>,
    #[serde(skip)]
    pub selected_2: HashMap<FsNodeId, bool>,

    #[serde(skip)]
    pub active_conflict_hash: Option<String>,

    #[serde(skip)]
    conflict_map: HashMap<String, Vec<PathBuf>>,
    #[serde(skip)]
    conflict_map_resolved: HashMap<String, PathBuf>,

    diff_config: DiffToolConfig,
}

#[derive(Debug, Serialize, Deserialize)]
enum AppState {
    Startup(AppStateCtx),
    Idle(AppStateCtx),
    Exit(),
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
            AppState::Exit() => panic!("Exit has no ctx"),
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

    diff_mode_path_diff: bool,

    open_external_diff_window: bool,
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
            diff_mode_path_diff: true,
            open_external_diff_window: false,
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

        let tile_duplicate_file = tiles.insert_pane(Pane::DuplicateFiles(DuplicateFilesPane::new(
            Some("Duplicate Files".to_string()),
        )));

        // let master_tile = tiles.insert_horizontal_tile(vec![tile_duplicate_file]);
        let master_tile = tiles.insert_horizontal_tile(vec![tile_path_diff, tile_duplicate_file]);
        tabs.push(tiles.insert_vertical_tile(vec![master_tile]));

        tiles.set_visible(tile_duplicate_file, false);
        let root = tiles.insert_tab_tile(tabs);

        egui_tiles::Tree::new("my_tree", root, tiles)
    }

    fn show_menu(&mut self, ui: &mut egui::Ui, app_ctx: &mut AppStateCtx) {
        ui.horizontal(|ui| {
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

                ui.menu_button("Options", |ui| {
                    if ui.button("Set external diff tool").clicked() {
                        self.open_external_diff_window = true;
                    }
                    if ui.button("Setup colors").clicked() {
                        log::error!("NYI");
                    }
                    if ui.button("Font size").clicked() {
                        log::error!("NYI");
                    }
                    if ui.button("Set Theme").clicked() {
                        log::error!("NYI");
                    }
                });

                let diff_mode_text = if self.diff_mode_path_diff {
                    "Change to Duplicate Diff"
                } else {
                    "Change to Path Diff"
                };
                ui.menu_button("Diff Mode", |ui| {
                    if ui.button(diff_mode_text).clicked() {
                        self.diff_mode_path_diff = !self.diff_mode_path_diff;
                    }
                });
            });
        });
        if self.open_external_diff_window {
            show_custom_popup(
                ui.ctx(),
                &mut self.open_external_diff_window,
                "Option - External Diff Tool",
                |ui| {
                    ui.label("Defaults: ");
                    ui.vertical(|ui| {
                        if ui.button("Tortoise").clicked() {
                            app_ctx.diff_config = DiffToolConfig::default_tortoise()
                        }
                    });

                    let mut text_edit = app_ctx.diff_config.exe_path.to_string_lossy();
                    ui.label("Path: ");
                    ui.text_edit_singleline(&mut text_edit);
                    app_ctx.diff_config.exe_path = PathBuf::from(text_edit.as_str());

                    let mut text_edit = app_ctx.diff_config.diff_path_1_args.clone();
                    ui.label("Path 1 args ({}): ");
                    ui.text_edit_singleline(&mut text_edit);
                    app_ctx.diff_config.diff_path_1_args = text_edit.to_string();

                    let mut text_edit = app_ctx.diff_config.diff_path_2_args.clone();
                    ui.label("Path 2 args ({}): ");
                    ui.text_edit_singleline(&mut text_edit);
                    app_ctx.diff_config.diff_path_2_args = text_edit.to_string();

                    let mut text_edit = app_ctx.diff_config.prefix_args.to_string();
                    ui.label("Prefix Args (\\n): ");
                    ui.text_edit_multiline(&mut text_edit);
                    app_ctx.diff_config.prefix_args =
                        DiffToolDefaultArgs::from_string(text_edit.as_str());

                    let mut text_edit = app_ctx.diff_config.suffix_args.to_string();
                    ui.label("Suffix args (\\n): ");
                    ui.text_edit_multiline(&mut text_edit);
                    app_ctx.diff_config.suffix_args =
                        DiffToolDefaultArgs::from_string(text_edit.as_str());
                },
            );
        }
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
                    file_system_1: &mut app_ctx.file_system_model_1,
                    expanded: &mut app_ctx.expanded,
                    selected_1: &mut app_ctx.selected_1,
                    active_conflict_hash: &mut app_ctx.active_conflict_hash,
                    conflict_map: &mut app_ctx.conflict_map,
                    conflict_map_resolved: &mut app_ctx.conflict_map_resolved,
                    diff_action_pressed: &mut diff_action_triggered,
                    file_system_2: &mut app_ctx.file_system_model_2,
                    selected_2: &mut app_ctx.selected_2,
                    diff_tool_config: &app_ctx.diff_config,
                };

                self.tree.ui(&mut behavior, ui);

                for (_tile_id, tile) in self.tree.tiles.iter() {
                    if let Tile::Pane(Pane::DuplicateFiles(_file_explorer)) = tile {
                        let out_ctx = behavior.create_duplicate_files_ctx();
                        let count_files_and_folders = out_ctx.file_system.total_files_and_folders();

                        let active = out_ctx.hash_service.count_active_hashes();
                        let total_pending = out_ctx.hash_service.count_hash_queue() + active;
                        let waiting = total_pending.saturating_sub(active);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                            "ZHashDiff - {} files/folders ({} active, {} queued)",
                            count_files_and_folders, active, waiting
                        )));

                        break;
                    }
                }
            });
        });
    }

    fn request_shutdown(&mut self) {
        self.state = Some(AppState::Exit());
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

    // Quick hacky funtion to determine if path_diff is open or duplicate_diff is open
    fn is_path_diff(&self) -> bool {
        for (tileid, tile) in self.tree.tiles.iter() {
            if let Tile::Pane(Pane::DuplicateFiles(_file_explorer)) = tile {
                if self.tree.is_visible(*tileid) {
                    return false;
                }
            } else if let Tile::Pane(Pane::PathDiff(_path_diff)) = tile {
                return true;
            }
        }
        panic!("PathDiff or DuplicateFiles should always be active");
    }
    fn set_path_diff_visible(&mut self, visible: bool) {
        let mut found_tile_id = None;
        for (tileid, tile) in self.tree.tiles.iter() {
            if let Tile::Pane(Pane::PathDiff(..)) = tile {
                found_tile_id = Some(tileid);
                break;
            }
        }
        self.tree
            .set_visible(*found_tile_id.expect("Should always find"), visible);
    }
    fn set_duplicate_files_visible(&mut self, visible: bool) {
        let mut found_tile_id = None;
        for (tileid, tile) in self.tree.tiles.iter() {
            if let Tile::Pane(Pane::DuplicateFiles(..)) = tile {
                found_tile_id = Some(tileid);
                break;
            }
        }
        self.tree
            .set_visible(*found_tile_id.expect("Should always find"), visible);
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
                if self.is_path_diff() != self.diff_mode_path_diff {
                    if self.is_path_diff() {
                        self.set_path_diff_visible(false);
                        self.set_duplicate_files_visible(true);
                    } else {
                        self.set_duplicate_files_visible(false);
                        self.set_path_diff_visible(true);
                    }
                }
                self.ui(ctx, frame, &mut state);
                // Handle folder dialogs
                if self.open_dir_window_1 {
                    self.open_dir_window_1 = false;
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        state.file_system_model_1 = FileSystemModel::new(&path);
                        state.expanded.clear();
                    }
                }
                if self.open_dir_window_2 {
                    self.open_dir_window_2 = false;
                    if let Some(path) = rfd::FileDialog::new().pick_folder() {
                        state.file_system_model_2 = FileSystemModel::new(&path);
                        state.expanded.clear();
                    }
                }
                self.process_ctx_inputs(ctx, frame);
                AppState::Idle(state)
            }
            AppState::Exit() => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                log::info!("send_viewport_cmd sent");

                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.centered_and_justified(|ui| {
                        ui.label("Exiting...");
                    });
                });
                AppState::Exit()
            }
        };
        self.state = Some(app_state);
    }
}
