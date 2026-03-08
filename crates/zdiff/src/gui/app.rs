use eframe::egui::{self, Layout, PointerButton, TextBuffer};
use serde::{Deserialize, Serialize};
use zdiff::{diff_ir::DiffIR, lexer::{Lexer, RawToken}, myers::{backtrack, myers_diff_trace}, read_file_contents};
use std::{
    path::PathBuf,
    sync::{Arc},
};

use eframe::{
    CreationContext,
    epaint::{Pos2, Vec2},
};
use egui_tiles::Tile;

use crate::{app, ui_egui::{diff_pane::{FileDiffPane, FileDiffPaneCtx}, panes::{Pane, TreeBehavior}}};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppStateCtx {
    file_1_name: Option<String>,
    file_2_name: Option<String>,
    file_1: Option<String>,
    file_2: Option<String>,

    // Myers
    #[serde(skip)]
    pub diff_path: Option<Vec<(i32, i32)>>,
    #[serde(skip)]
    pub tokens_1: Option<Vec<RawToken>>,
    #[serde(skip)]
    pub tokens_2: Option<Vec<RawToken>>,
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
}

const HARDCODED_MONITOR_SIZE: Vec2 = Vec2::new(2560.0, 1440.0);
impl ZApp {
    pub fn request_init(&mut self) {
        // if self.state.ctx().hash_service.is_none() {
        // self.state.ctx().hash_service = HashService::default();
        // }
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
                    if ui.button("Open File 1").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            app_ctx.file_1_name = Some(path.file_name().unwrap_or_default().to_string_lossy().into_owned());
                            app_ctx.file_1 = Some(read_file_contents(path).expect("Failed to read file"));
                        }
                    }
                    if ui.button("Open File 2").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            app_ctx.file_2_name = Some(path.file_name().unwrap_or_default().to_string_lossy().into_owned());
                            app_ctx.file_2 = Some(read_file_contents(path).expect("Failed to read file"));
                        }
                    }
                });
            });
        });
    }

    fn ui(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame, app_ctx: &mut AppStateCtx) {
        egui::CentralPanel::default().show(ctx, |ui| {
            self.show_menu(ui, app_ctx);

            ui.separator();

            ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                let mut behavior = TreeBehavior {
                    ctx_file_diff: FileDiffPaneCtx {
                        file_1: app_ctx.file_1.as_ref(),
                        file_2: app_ctx.file_2.as_ref(),
                        file_1_name: app_ctx.file_1_name.as_ref(),
                        file_2_name: app_ctx.file_2_name.as_ref(),
                        diff_path: app_ctx.diff_path.as_ref(),
                        tokens_1: app_ctx.tokens_1.as_ref(),
                        tokens_2: app_ctx.tokens_2.as_ref(),
                    },
                };

                self.tree.ui(&mut behavior, ui);

                for (_tile_id, tile) in self.tree.tiles.iter() {
                    if let Tile::Pane(Pane::FileDiff(..)) = tile {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                            "zdiff - {0}, {1}",
                            behavior.ctx_file_diff.file_1_name.as_ref().map_or_else(|| "N/A", |s| s),
                            behavior.ctx_file_diff.file_2_name.as_ref().map_or_else(|| "N/A", |s| s),
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

    fn update_diff_path(&mut self, app_ctx: &mut AppStateCtx) {
        app_ctx.tokens_1 = Some(Lexer::new(app_ctx.file_1.as_ref().unwrap()).collect());
        app_ctx.tokens_2 = Some(Lexer::new(app_ctx.file_2.as_ref().unwrap()).collect());
        let tokens_1 = app_ctx.tokens_1.as_ref().unwrap();
        let tokens_2 = app_ctx.tokens_2.as_ref().unwrap();

        let lexer_1 = Lexer::new(app_ctx.file_1.as_ref().unwrap());
        let lexer_2 = Lexer::new(app_ctx.file_2.as_ref().unwrap());
        let cmp = |t1: &RawToken, t2: &RawToken| {
            t1.kind == t2.kind && lexer_1.token_value(t1) == lexer_2.token_value(t2)
        };

        let trace = myers_diff_trace(&tokens_1, &tokens_2, cmp);
        app_ctx.diff_path = Some(backtrack(trace, tokens_1.len() as i32, tokens_2.len() as i32));
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
                if let (Some(_file_1), Some(_file_2)) = (state.file_1.as_ref(), state.file_2.as_ref()) {
                    self.update_diff_path(&mut state);
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
