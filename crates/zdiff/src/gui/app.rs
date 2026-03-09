use eframe::egui::{self, Layout, PointerButton};
use serde::{Deserialize, Serialize};
use zdiff::{lexer::{Lexer, RawToken, TokenKind}, myers::{backtrack, myers_diff_trace}, read_file_contents};
use std::{
    env, path::PathBuf
};

use eframe::{
    CreationContext,
    epaint::{Pos2, Vec2},
};
use egui_tiles::Tile;

use crate::ui_egui::{diff_pane::{DiffRow, FileDiffPane, FileDiffPaneCtx, FileDiffPaneOptions, build_diff_rows}, panes::{Pane, TreeBehavior}};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppStateCtx {
    file_1_path: Option<PathBuf>,
    file_2_path: Option<PathBuf>,
    
    // Reopen contents on startup
    #[serde(skip)]
    file_1_contents: Option<String>,
    #[serde(skip)]
    file_2_contents: Option<String>,

    // Myers
    #[serde(skip)]
    pub diff_rows: Option<Vec<DiffRow>>,
    #[serde(skip)]
    pub tokens_1: Option<Vec<RawToken>>,
    #[serde(skip)]
    pub tokens_2: Option<Vec<RawToken>>,

    pub diff_option: FileDiffPaneOptions,
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

#[derive(Serialize, Deserialize)]
pub struct ZApp {
    monitor_size: Vec2,
    scale_factor: f32,
    native_pixel_per_point: f32,
    // Option > Hack to avoid cloning state when matching &mut self.state in update loop
    state: Option<AppState>,
    tree: egui_tiles::Tree<Pane>,
}

const HARDCODED_MONITOR_SIZE: Vec2 = Vec2::new(2560.0, 1440.0);
impl ZApp {
    fn update_file_contents(&mut self, app_ctx: &mut AppStateCtx)
    {
        if app_ctx.file_1_contents.is_none()
        {
            if let Some(file_1_path) = &app_ctx.file_1_path
            {
                log::info!("Reading file 1: {}", file_1_path.display());
                match read_file_contents(file_1_path.clone())
                {
                    Ok(contents) => {app_ctx.file_1_contents = Some(contents);},
                    Err(e) => {log::error!("Failed to read file 1: {}", e);},
                };
            }
        }
        if app_ctx.file_2_contents.is_none()
        {
            if let Some(file_2_path) = &app_ctx.file_2_path
            {
                log::info!("Reading file 2: {}", file_2_path.display());
                match read_file_contents(file_2_path.clone())
                {
                    Ok(contents) => {app_ctx.file_2_contents = Some(contents);},
                    Err(e) => {log::error!("Failed to read file 2: {}", e);},
                };
            }
        }
    }

    pub fn request_init(&mut self) {
        if let Some(state) = &mut self.state
        {
            match state {
                 AppState::Startup(ctx) | AppState::Idle(ctx) => {
                    let args: Vec<String> = env::args().collect();
                    let p1 = args.get(1).cloned();
                    let p2 = args.get(2).cloned();

                    if let (Some(p1), Some(p2)) = (p1, p2) {
                        ctx.file_1_path = Some(PathBuf::from(p1));
                        ctx.file_2_path = Some(PathBuf::from(p2));

                        match read_file_contents(ctx.file_1_path.clone().unwrap_or_default())
                        {
                            Ok(contents) => {ctx.file_1_contents = Some(contents);},
                            Err(e) => {log::error!("Failed to read file 1: {}", e);},
                        };
                        match read_file_contents(ctx.file_2_path.clone().unwrap_or_default())
                        {
                            Ok(contents) => {ctx.file_2_contents = Some(contents);},
                            Err(e) => {log::error!("Failed to read file 2: {}", e);},
                        };
                    }
                }
                _ => {}
            }
        }
    }

    pub fn new(cc: &CreationContext<'_>) -> Self {
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

        let tile_path_diff = tiles.insert_pane(Pane::FileDiff(FileDiffPane::new(Some(
            "Path Diff".to_string(),
        ))));

        // let master_tile = tiles.insert_horizontal_tile(vec![tile_duplicate_file]);
        let master_tile = tiles.insert_horizontal_tile(vec![tile_path_diff]);
        tabs.push(tiles.insert_vertical_tile(vec![master_tile]));

        let root = tiles.insert_tab_tile(tabs);

        egui_tiles::Tree::new("my_tree", root, tiles)
    }

    fn show_menu(&mut self, ui: &mut egui::Ui, app_ctx: &mut AppStateCtx) {
        ui.horizontal(|ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.menu_button("File", |ui| {
                    if ui.button("Open Source").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            app_ctx.file_1_path = Some(path.clone());
                            app_ctx.file_1_contents = Some(read_file_contents(path).expect("Failed to read file"));
                        }
                    }
                    if ui.button("Open Target").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            app_ctx.file_2_path = Some(path.clone());
                            app_ctx.file_2_contents = Some(read_file_contents(path).expect("Failed to read file"));
                        }
                    }
                });

                ui.menu_button("Debug", |ui| {
                    if ui.button("Clear File Paths").clicked() {
                        app_ctx.file_1_path = None;
                        app_ctx.file_2_path = None;
                    }
                    if ui.button("Clear File contents").clicked() {
                        app_ctx.file_1_contents = None;
                        app_ctx.file_2_contents = None;
                    }
                    if ui.button("Clear Diff Rows").clicked() {
                        app_ctx.diff_rows = None;
                    }
                });
            });
        });
    }

    fn ui(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame, app_ctx: &mut AppStateCtx) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.show_menu(ui, app_ctx);

            ui.separator();

            ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                let diff_options_before  = app_ctx.diff_option.clone();
                let mut behavior = TreeBehavior {
                    ctx_file_diff: FileDiffPaneCtx {
                        diff_rows: app_ctx.diff_rows.as_ref(),
                        tokens_1: app_ctx.tokens_1.as_ref(),
                        tokens_2: app_ctx.tokens_2.as_ref(),
                        options: &mut app_ctx.diff_option,
                    },
                };

                self.tree.ui(&mut behavior, ui);

                // Invalidate diff_rows if options changed
                if diff_options_before != app_ctx.diff_option {
                    app_ctx.diff_rows = None;
                }

                for (_tile_id, tile) in self.tree.tiles.iter() {
                    if let Tile::Pane(Pane::FileDiff(..)) = tile {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                            "zdiff - {0}, {1}",
                            app_ctx.file_1_path.as_ref().map_or_else(|| "N/A", |p| p.file_name().unwrap_or_default().to_str().unwrap_or_default()),
                            app_ctx.file_2_path.as_ref().map_or_else(|| "N/A", |p| p.file_name().unwrap_or_default().to_str().unwrap_or_default()),
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

    fn update_diff_rows(&mut self, app_ctx: &mut AppStateCtx) {
        app_ctx.tokens_1 = Some(Lexer::new(app_ctx.file_1_contents.as_ref().unwrap()).collect());
        app_ctx.tokens_2 = Some(Lexer::new(app_ctx.file_2_contents.as_ref().unwrap()).collect());
        let tokens_1 = app_ctx.tokens_1.as_ref().unwrap();
        let tokens_2 = app_ctx.tokens_2.as_ref().unwrap();

        let lexer_1 = Lexer::new(app_ctx.file_1_contents.as_ref().unwrap());
        let lexer_2 = Lexer::new(app_ctx.file_2_contents.as_ref().unwrap());
        let ignore_ws = app_ctx.diff_option.ignore_whitespace;
        let cmp = |t1: &RawToken, t2: &RawToken| {
            if ignore_ws && t1.kind.is_whitespace() && t2.kind.is_whitespace() {
                return true;
            }
            t1.kind == t2.kind && lexer_1.token_value(t1) == lexer_2.token_value(t2)
        };

        let trace = myers_diff_trace(&tokens_1, &tokens_2, cmp);
        let diff_path = backtrack(trace, tokens_1.len() as i32, tokens_2.len() as i32);
        app_ctx.diff_rows = Some(build_diff_rows(&diff_path, tokens_1, tokens_2, &lexer_1, &lexer_2, &app_ctx.diff_option));
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
                if state.file_1_path.is_none()
                {
                    state.file_1_contents = None;
                }
                if state.file_2_path.is_none()
                {
                    state.file_2_contents = None;
                }
                if state.file_1_contents.is_none()
                {
                    state.diff_rows = None;
                }
                if state.file_2_contents.is_none()
                {
                    state.diff_rows = None;
                }
                self.update_file_contents(&mut state);
                if state.diff_rows.is_none() && state.file_1_contents.is_some() && state.file_2_contents.is_some(){
                    self.update_diff_rows(&mut state);
                }
                self.ui(ctx, frame, &mut state);
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
