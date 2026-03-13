use eframe::egui::{self, Layout, PointerButton};
use serde::{Deserialize, Serialize};
use std::{
    env, io,
    path::{Path, PathBuf},
};
use zdiff::{
    diff_builder::{DiffBuilderOptions, DiffRow, build_diff_rows, build_single_file_rows},
    hash::hash_file,
    lexer::{Lexer, RawToken},
    myers::{myers_backtrack, myers_count_add_deletes, myers_diff_trace},
    read_file_contents,
};

use eframe::{
    CreationContext,
    epaint::{Pos2, Vec2},
};
use egui_tiles::Tile;

use crate::ui_egui::{
    diff_pane::{FileDiffPane, FileDiffPaneCtx},
    panes::{Pane, TreeBehavior},
};

#[derive(Debug, Default)]
pub struct DiffCtx {
    source_path: Option<PathBuf>,
    target_path: Option<PathBuf>,
    source_hash: Option<String>,
    target_hash: Option<String>,

    pub diff_option: DiffBuilderOptions,
    // Myers
    pub diff_rows: Vec<DiffRow>,
    pub num_add_deletes: (u32, u32),
}

#[derive(Debug, Default)]
pub struct CachedFile {
    path: PathBuf,
    hash: String,
    contents: String,
    tokens: Vec<RawToken>, // span to contents
}

impl CachedFile {
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let contents = read_file_contents(&path)?;
        let hash = hash_file(&path)?;
        let tokens = Lexer::new(&contents).collect();
        let path = path.as_ref().to_path_buf();
        Ok(Self {
            path,
            hash,
            contents,
            tokens,
        })
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AppStateCtx {
    file_path_1: Option<PathBuf>,
    file_path_2: Option<PathBuf>,
    #[serde(skip)]
    file_1: Option<CachedFile>,
    #[serde(skip)]
    file_2: Option<CachedFile>,

    #[serde(skip)]
    pub diff_ctx: Option<DiffCtx>,
    pub diff_options: DiffBuilderOptions,

    pub scroll_left: f32,
    pub scroll_right: f32,
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
    fn update_files(&mut self, app_ctx: &mut AppStateCtx) -> bool {
        let mut changed = false;

        if let Some(path) = &app_ctx.file_path_1 {
            if app_ctx.file_1.is_none()
                || app_ctx.file_1.as_ref().unwrap().path != *path
                || app_ctx.file_1.as_ref().unwrap().hash != hash_file(path).expect("failed to hash")
            {
                app_ctx.file_1 = CachedFile::new(path).ok();
                changed |= app_ctx.file_1.is_some()
            }
        }

        if let Some(path) = &app_ctx.file_path_2 {
            if app_ctx.file_2.is_none()
                || app_ctx.file_2.as_ref().unwrap().path != *path
                || app_ctx.file_2.as_ref().unwrap().hash != hash_file(path).expect("failed to hash")
            {
                app_ctx.file_2 = CachedFile::new(path).ok();
                changed |= app_ctx.file_2.is_some()
            }
        }

        changed
    }

    pub fn request_init(&mut self) {
        if let Some(state) = &mut self.state {
            match state {
                AppState::Startup(ctx) | AppState::Idle(ctx) => {
                    let args: Vec<String> = env::args().collect();
                    let p1 = args.get(1).cloned();
                    let p2 = args.get(2).cloned();

                    if let (Some(p1), Some(p2)) = (p1, p2) {
                        let new_file_1 = CachedFile::new(p1);
                        let new_file_2 = CachedFile::new(p2);
                        match new_file_1
                        {
                            Ok(c) => 
                            {
                                ctx.file_1 = Some(c);
                            },
                            Err(e) => log::error!("{e}"),
                        }
                        match new_file_2
                        {
                            Ok(c) => 
                            {
                                ctx.file_2 = Some(c);
                            },
                            Err(e) => log::error!("{e}"),
                        }
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
                            app_ctx.file_path_1 = Some(path.clone());
                        }
                    }
                    if ui.button("Open Target").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            app_ctx.file_path_2 = Some(path.clone());
                        }
                    }
                    if ui.button("Swap Source/Target").clicked() {
                        std::mem::swap(&mut app_ctx.file_1, &mut app_ctx.file_2);
                        std::mem::swap(&mut app_ctx.file_path_1, &mut app_ctx.file_path_2);
                        std::mem::swap(&mut app_ctx.scroll_left, &mut app_ctx.scroll_right);
                        app_ctx.diff_ctx = None;
                    }
                });

                ui.menu_button("Debug", |ui| {
                    if ui.button("Clear File Paths").clicked() {
                        app_ctx.file_path_1 = None;
                        app_ctx.file_path_2 = None;
                    }
                    if ui.button("Clear Cached Files").clicked() {
                        app_ctx.file_1 = None;
                        app_ctx.file_2 = None;
                    }
                    if ui.button("Clear Diff Rows").clicked() {
                        app_ctx.diff_ctx = None;
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
                let diff_options_before = app_ctx.diff_options.clone();
                let mut behavior = TreeBehavior {
                    ctx_file_diff: FileDiffPaneCtx {
                        path_1: app_ctx.file_path_1.as_ref(),
                        path_2: app_ctx.file_path_2.as_ref(),
                        scroll_left: &mut app_ctx.scroll_left,
                        scroll_right: &mut app_ctx.scroll_right,
                        diff_ctx: app_ctx.diff_ctx.as_ref(),
                        diff_options: &mut app_ctx.diff_options,
                    },
                };

                self.tree.ui(&mut behavior, ui);

                // Invalidate diff if options changed
                if diff_options_before != app_ctx.diff_options {
                    app_ctx.diff_ctx = None;
                }

                for (_tile_id, tile) in self.tree.tiles.iter() {
                    if let Tile::Pane(Pane::FileDiff(..)) = tile {
                        let source = app_ctx
                            .file_1
                            .as_ref()
                            .map_or_else(|| "N/A", |p| p.path.to_str().unwrap_or_default());
                        let target = app_ctx
                            .file_2
                            .as_ref()
                            .map_or_else(|| "N/A", |p| p.path.to_str().unwrap_or_default());
                        let total_adds = app_ctx
                            .diff_ctx
                            .as_ref()
                            .and_then(|f| Some(f.num_add_deletes))
                            .unwrap_or_default()
                            .0;
                        let total_deletes = app_ctx
                            .diff_ctx
                            .as_ref()
                            .and_then(|f| Some(f.num_add_deletes))
                            .unwrap_or_default()
                            .1;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                            "zdiff [+{}/-{}] - {}, {}",
                            total_adds, total_deletes, source, target
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

    fn update_diff_rows(&mut self, app_ctx: &mut AppStateCtx, options: &DiffBuilderOptions) {
        match (&app_ctx.file_1, &app_ctx.file_2) {
            (Some(c1), Some(c2)) => {
                let t1 = &c1.tokens;
                let t2 =  &c2.tokens;
                let lex1 = Lexer::new(&c1.contents);
                let lex2 = Lexer::new(&c2.contents);

                let ignore_ws = app_ctx
                    .diff_ctx
                    .as_ref()
                    .and_then(|f| Some(f.diff_option.clone()))
                    .unwrap_or_default()
                    .ignore_whitespace;
                let cmp = |a: &RawToken, b: &RawToken| {
                    if ignore_ws && a.kind.is_whitespace() && b.kind.is_whitespace() {
                        return true;
                    }
                    a.kind == b.kind && lex1.token_value(a) == lex2.token_value(b)
                };

                let trace = myers_diff_trace(t1, t2, cmp);
                let path = myers_backtrack(trace, t1.len() as i32, t2.len() as i32);

                let c1_hash = hash_file(&c1.path).expect("Hash failed");
                let c2_hash = hash_file(&c2.path).expect("Hash failed");
                app_ctx.diff_ctx = Some(DiffCtx {
                    diff_option: options.clone(),
                    diff_rows: build_diff_rows(&path, t1, t2, &lex1, &lex2, &options),
                    num_add_deletes: myers_count_add_deletes(&path),
                    source_hash: Some(c1_hash),
                    target_hash: Some(c2_hash),
                    source_path: Some(c1.path.clone()),
                    target_path: Some(c2.path.clone()),
                });
            }
            (Some(c1), None) => {
                let t1 = &app_ctx.file_1.as_ref().unwrap().tokens;
                let c1_hash = hash_file(&c1.path).expect("Hash failed");
                app_ctx.diff_ctx = Some(DiffCtx {
                    diff_option: options.clone(),
                    diff_rows: build_single_file_rows(t1, &Lexer::new(&c1.contents), &options, true),
                    num_add_deletes: (0, 0),
                    source_hash: None,
                    target_hash: Some(c1_hash),
                    source_path: None,
                    target_path: Some(c1.path.clone()),
                });
            }
            (None, Some(c2)) => {
                let t2 = &app_ctx.file_2.as_ref().unwrap().tokens;
                let c2_hash = hash_file(&c2.path).expect("Hash failed");
                app_ctx.diff_ctx = Some(DiffCtx {
                    diff_option: options.clone(),
                    diff_rows: build_single_file_rows(t2, &Lexer::new(&c2.contents), &options, false),
                    num_add_deletes: (0, 0),
                    source_hash: None,
                    target_hash: Some(c2_hash),
                    source_path: None,
                    target_path: Some(c2.path.clone()),
                });
            }
            (None, None) => {}
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
                let new_cached_file = self.update_files(&mut state);

                if new_cached_file {
                    state.diff_ctx.take();
                }

                if state.diff_ctx.is_none() {
                    let diff_options = state.diff_options.clone();
                    self.update_diff_rows(&mut state, &diff_options);
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
