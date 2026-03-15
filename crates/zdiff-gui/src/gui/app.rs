use eframe::egui::{self, Layout, PointerButton};
use serde::{Deserialize, Serialize};
use std::{
    default, env, io,
    ops::Range,
    path::{Path, PathBuf},
    result,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, channel},
    },
    thread::Thread,
};
use zcommon::{hash::hash_file, ui_egui::common::show_custom_popup};
use zdiff::{
    diff_builder::{CachedFile, DiffBuilderOptions, DiffRow, LineContent, build_diff_rows},
    diff_ir::{DiffOp, generate_ir},
    lexer::{Lexer, RawToken, RawTokenTrait, TokenKind},
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
    pub file_1_hash: String,
    pub file_2_hash: String,
    pub one_sided_diff_is_left: Option<bool>,
    pub diff_option: DiffBuilderOptions,
    pub precomputed_diffs: Vec<usize>, // list indicies of diff_rows of DiffOp != Equal from diff_rows
    // Myers
    pub diff_rows: Vec<DiffRow>,
    pub num_add_deletes: (u32, u32),
}

#[derive(Debug)]
#[cfg_attr(
    feature = "serde",
    serde(bound(serialize = "", deserialize = "T: RawTokenTrait"))
)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppStateCtx<T: RawTokenTrait> {
    #[cfg_attr(feature = "serde", serde(skip))]
    rx_file_path_1: Option<mpsc::Receiver<std::path::PathBuf>>,
    #[cfg_attr(feature = "serde", serde(skip))]
    rx_file_path_2: Option<mpsc::Receiver<std::path::PathBuf>>,

    file_path_1: Option<PathBuf>,
    file_path_2: Option<PathBuf>,
    #[cfg_attr(feature = "serde", serde(skip))]
    file_1: Option<Arc<CachedFile<T>>>,
    #[cfg_attr(feature = "serde", serde(skip))]
    file_2: Option<Arc<CachedFile<T>>>,

    pub diff_options: DiffBuilderOptions,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub diff_ctx: Option<DiffCtx>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub diff_ctx_invalidated: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub diff_ctx_in_progress: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    rx: Option<Receiver<DiffCtx>>,

    pub scroll_left: f32,
    pub scroll_right: f32,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub scroll_to_goto_row: Option<usize>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub goto_open: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub goto_input: String,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub find_open: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub find_input: String,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub scroll_to_find_row: Option<usize>,

    #[cfg_attr(feature = "serde", serde(skip))]
    pub diff_ctx_conflict_cursor: usize,
}

impl<T: RawTokenTrait> Default for AppStateCtx<T> {
    fn default() -> Self {
        Self {
            file_path_1: Default::default(),
            file_path_2: Default::default(),
            file_1: Default::default(),
            file_2: Default::default(),
            diff_options: Default::default(),
            diff_ctx: Default::default(),
            diff_ctx_in_progress: Default::default(),
            rx: Default::default(),
            scroll_left: Default::default(),
            scroll_right: Default::default(),
            diff_ctx_invalidated: Default::default(),
            scroll_to_goto_row: Default::default(),
            goto_open: Default::default(),
            find_open: Default::default(),
            goto_input: Default::default(),
            find_input: Default::default(),
            rx_file_path_1: Default::default(),
            rx_file_path_2: Default::default(),
            scroll_to_find_row: Default::default(),
            diff_ctx_conflict_cursor: Default::default(),
        }
    }
}

impl<T: RawTokenTrait> AppStateCtx<T> {
    fn precompute_diffs(diff_rows: &[DiffRow]) -> Vec<usize> {
        let precomputed_diffs: Vec<usize> = diff_rows
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| {
                let has_change = |content: &LineContent| match content {
                    LineContent::Code { tokens, .. } => {
                        tokens.iter().any(|(res, _)| res.operation != DiffOp::Equal)
                    }
                    LineContent::Void => false,
                };

                if has_change(&row.left) || has_change(&row.right) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();
        precomputed_diffs
    }

    fn update_diff_rows(
        file_1: Option<Arc<CachedFile<T>>>,
        file_2: Option<Arc<CachedFile<T>>>,
        options: &DiffBuilderOptions,
    ) -> DiffCtx {
        match (&file_1, &file_2) {
            (Some(c1), Some(c2)) => {
                let t1 = &c1.tokens;
                let t2 = &c2.tokens;
                let lex1 = Lexer::<T>::new(&c1.contents);
                let lex2 = Lexer::<T>::new(&c2.contents);

                let cmp = |a: &T, b: &T| {
                    if options.ignore_whitespace
                        && a.as_ref().kind.is_whitespace()
                        && b.as_ref().kind.is_whitespace()
                    {
                        return true;
                    }
                    a.as_ref().kind == b.as_ref().kind && lex1.token_value(a) == lex2.token_value(b)
                };

                let trace = myers_diff_trace(t1, t2, cmp);
                let path = myers_backtrack(trace, t1.len() as i32, t2.len() as i32);
                let diff_ir = generate_ir(&path);

                let c1_hash = hash_file(&c1.path).expect("Hash failed");
                let c2_hash = hash_file(&c2.path).expect("Hash failed");

                let diff_rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options);
                let precomputed_diffs = Self::precompute_diffs(&diff_rows);
                DiffCtx {
                    file_1_hash: c1_hash,
                    file_2_hash: c2_hash,
                    diff_option: options.clone(),
                    diff_rows,
                    num_add_deletes: myers_count_add_deletes(&path),
                    one_sided_diff_is_left: None,
                    precomputed_diffs,
                }
            }
            (Some(c1), None) => {
                let c2 = c1;
                let t1 = &c1.tokens;
                let t2 = &c2.tokens;
                let lex1 = Lexer::new(&c1.contents);
                let lex2 = Lexer::new(&c2.contents);

                let cmp = |a: &T, b: &T| {
                    if options.ignore_whitespace
                        && a.as_ref().kind.is_whitespace()
                        && b.as_ref().kind.is_whitespace()
                    {
                        return true;
                    }
                    a.as_ref().kind == b.as_ref().kind && lex1.token_value(a) == lex2.token_value(b)
                };

                let trace = myers_diff_trace(t1, t2, cmp);
                let path = myers_backtrack(trace, t1.len() as i32, t2.len() as i32);
                let diff_ir = generate_ir(&path);

                let c1_hash = hash_file(&c1.path).expect("Hash failed");
                let diff_rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options);
                let precomputed_diffs = Self::precompute_diffs(&diff_rows);
                DiffCtx {
                    file_1_hash: c1_hash.clone(),
                    file_2_hash: c1_hash,
                    diff_option: options.clone(),
                    diff_rows,
                    num_add_deletes: myers_count_add_deletes(&path),
                    one_sided_diff_is_left: Some(true),
                    precomputed_diffs,
                }
            }
            (None, Some(c2)) => {
                let c1 = c2;
                let t1 = &c1.tokens;
                let t2 = &c2.tokens;
                let lex1 = Lexer::new(&c1.contents);
                let lex2 = Lexer::new(&c2.contents);

                let cmp = |a: &T, b: &T| {
                    if options.ignore_whitespace
                        && a.as_ref().kind.is_whitespace()
                        && b.as_ref().kind.is_whitespace()
                    {
                        return true;
                    }
                    a.as_ref().kind == b.as_ref().kind && lex1.token_value(a) == lex2.token_value(b)
                };

                let trace = myers_diff_trace(t1, t2, cmp);
                let path = myers_backtrack(trace, t1.len() as i32, t2.len() as i32);
                let diff_ir = generate_ir(&path);

                let c2_hash = hash_file(&c2.path).expect("Hash failed");
                let diff_rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options);
                let precomputed_diffs = Self::precompute_diffs(&diff_rows);
                DiffCtx {
                    file_1_hash: c2_hash.clone(),
                    file_2_hash: c2_hash,
                    diff_option: options.clone(),
                    diff_rows,
                    num_add_deletes: myers_count_add_deletes(&path),
                    one_sided_diff_is_left: Some(false),
                    precomputed_diffs,
                }
            }
            (None, None) => {
                panic!("Only call this function with one of two files valid")
            }
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(bound(serialize = "", deserialize = "T: RawTokenTrait"))]
enum AppState<T: RawTokenTrait> {
    Startup(AppStateCtx<T>),
    Idle(AppStateCtx<T>),
    Exit(AppStateCtx<T>),
}

impl<T: RawTokenTrait> AppState<T> {
    fn variant_name(&self) -> &'static str {
        match self {
            Self::Startup(_) => "Startup",
            Self::Idle(_) => "Idle",
            Self::Exit(_) => "Exit",
        }
    }
}

impl<T: RawTokenTrait> Default for AppState<T> {
    fn default() -> Self {
        AppState::Startup(AppStateCtx::default())
    }
}

impl<T: RawTokenTrait> AppState<T> {
    fn into_ctx(self) -> AppStateCtx<T> {
        match self {
            AppState::Startup(ctx) | AppState::Idle(ctx) | AppState::Exit(ctx) => ctx,
        }
    }
    fn ctx_mut(&mut self) -> &mut AppStateCtx<T> {
        match self {
            AppState::Startup(ctx) | AppState::Idle(ctx) | AppState::Exit(ctx) => ctx,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(bound(serialize = "", deserialize = "T: RawTokenTrait"))]
pub struct ZApp<T: RawTokenTrait> {
    monitor_size: Vec2,
    scale_factor: f32,
    native_pixel_per_point: f32,
    // Option > Hack to avoid cloning state when matching &mut self.state in update loop
    state: Option<AppState<T>>,
    tree: egui_tiles::Tree<Pane>,
}

const HARDCODED_MONITOR_SIZE: Vec2 = Vec2::new(2560.0, 1440.0);
impl<'a, T: RawTokenTrait> ZApp<T> {
    fn update_source_target(&mut self, app_ctx: &mut AppStateCtx<T>) -> bool {
        let mut changed = false;

        // Want to allow this later, for now easier if cached file is never stale
        if app_ctx.file_path_1.is_none() {
            app_ctx.file_1.take();
        }
        if app_ctx.file_path_2.is_none() {
            app_ctx.file_2.take();
        }

        if let Some(path) = &app_ctx.file_path_1 {
            if app_ctx.file_1.is_none()
                || app_ctx.file_1.as_ref().unwrap().path != *path
                || app_ctx.file_1.as_ref().unwrap().hash != hash_file(path).expect("failed to hash")
            {
                if let Ok(r) = CachedFile::new(path) {
                    app_ctx.file_1 = Some(Arc::new(r));
                    changed |= app_ctx.file_1.is_some()
                }
            }
        }

        if let Some(path) = &app_ctx.file_path_2 {
            if app_ctx.file_2.is_none()
                || app_ctx.file_2.as_ref().unwrap().path != *path
                || app_ctx.file_2.as_ref().unwrap().hash != hash_file(path).expect("failed to hash")
            {
                if let Ok(r) = CachedFile::new(path) {
                    app_ctx.file_2 = Some(Arc::new(r));
                    changed |= app_ctx.file_2.is_some()
                }
            }
        }

        changed
    }

    pub fn request_init(&mut self) {
        log::info!(
            "Request init called with state: {}",
            self.state
                .as_ref()
                .and_then(|f| Some(f.variant_name()))
                .unwrap_or_default()
        );
        self.state = self
            .state
            .take()
            .map(|ctx| AppState::Startup(ctx.into_ctx()));
        if let Some(state) = &mut self.state {
            match state {
                AppState::Startup(ctx) | AppState::Idle(ctx) => {
                    let args: Vec<String> = env::args().collect();
                    let p1 = args.get(1).cloned();
                    let p2 = args.get(2).cloned();

                    if let (Some(p1), Some(p2)) = (p1, p2) {
                        let new_file_1 = CachedFile::new(p1);
                        let new_file_2 = CachedFile::new(p2);
                        match new_file_1 {
                            Ok(c) => {
                                ctx.file_1 = Some(Arc::new(c));
                            }
                            Err(e) => log::error!("{e}"),
                        }
                        match new_file_2 {
                            Ok(c) => {
                                ctx.file_2 = Some(Arc::new(c));
                            }
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

    fn show_menu(
        &self,
        ui: &mut egui::Ui,
        rx_file_path_1: &mut Option<mpsc::Receiver<PathBuf>>,
        rx_file_path_2: &mut Option<mpsc::Receiver<PathBuf>>,
        file_path_1: &mut Option<PathBuf>,
        file_path_2: &mut Option<PathBuf>,
        file_1: &mut Option<Arc<CachedFile<T>>>,
        file_2: &mut Option<Arc<CachedFile<T>>>,
        diff_ctx: &mut Option<DiffCtx>,
        diff_ctx_invalidated: &mut bool,
        find_open: &mut bool,
        goto_open: &mut bool,
    ) {
        let check_file_rx = |rx_opt: &mut Option<std::sync::mpsc::Receiver<std::path::PathBuf>>,
                             target: &mut Option<std::path::PathBuf>| {
            if let Some(rx) = rx_opt {
                match rx.try_recv() {
                    Ok(path) => *target = Some(path),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => *rx_opt = None,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
        };
        check_file_rx(rx_file_path_1, file_path_1);
        check_file_rx(rx_file_path_2, file_path_2);

        ui.horizontal(|ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.menu_button("File", |ui| {
                    if ui.button("Open Source").clicked() {
                        let (tx, rx) = mpsc::channel();
                        *rx_file_path_1 = Some(rx);
                        std::thread::spawn(move || {
                            if let Some(path) =
                                pollster::block_on(rfd::AsyncFileDialog::new().pick_file())
                            {
                                println!("File picker: {:?}", path.path());
                                match tx.send(path.path().to_path_buf()) {
                                    Ok(_) => {}
                                    Err(e) => log::error!("{e}"),
                                }
                            }
                        });
                    }
                    if ui.button("Open Target").clicked() {
                        if let Some(path) = rfd::FileDialog::new().pick_file() {
                            *file_path_2 = Some(path.clone());
                        }
                    }
                    // if ui.button("Swap Source/Target").clicked() {
                    // std::mem::swap(&mut file_1, &mut file_2);
                    // std::mem::swap(&mut file_path_1, &mut file_path_2);
                    // std::mem::swap(&mut scroll_left, &mut scroll_right);
                    // }
                    if ui.button("Find").clicked() {
                        *find_open = true;
                    }
                    if ui.button("Goto").clicked() {
                        *goto_open = true;
                    }
                });

                ui.menu_button("Debug", |ui| {
                    if ui.button("Clear File Paths").clicked() {
                        *file_path_1 = None;
                        *file_path_2 = None;
                        *diff_ctx = None;
                        *diff_ctx_invalidated = true;
                    }
                    if ui.button("Clear Cached Files").clicked() {
                        *file_1 = None;
                        *file_2 = None;
                        *diff_ctx = None;
                        *diff_ctx_invalidated = true;
                    }
                    if ui.button("Clear Diff Rows").clicked() {
                        *diff_ctx = None;
                        *diff_ctx_invalidated = true;
                    }
                });
            });
        });
    }

    fn ui(
        &mut self,
        ctx: &egui::Context,
        _frame: &mut eframe::Frame,
        app_ctx: &mut AppStateCtx<T>,
    ) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let AppStateCtx {
                file_path_1,
                file_path_2,
                scroll_left,
                scroll_right,
                diff_ctx,
                diff_options,
                file_1,
                file_2,
                rx,
                diff_ctx_in_progress,
                diff_ctx_invalidated,
                scroll_to_goto_row,
                scroll_to_find_row,
                goto_open,
                find_open,
                goto_input,
                find_input,
                rx_file_path_1,
                rx_file_path_2,
                diff_ctx_conflict_cursor,
            } = app_ctx;
            let diff_options_before = diff_options.clone();
            self.show_menu(
                ui,
                rx_file_path_1,
                rx_file_path_2,
                file_path_1,
                file_path_2,
                file_1,
                file_2,
                diff_ctx,
                diff_ctx_invalidated,
                find_open,
                goto_open,
            );

            let mut goto_window_open = *goto_open;
            show_custom_popup(ctx, &mut goto_window_open, "Goto", |ui| {
                goto_input.retain(|c| c.is_ascii_digit());
                let response = ui.add(
                    egui::TextEdit::singleline(goto_input)
                        .desired_width(40.0)
                        .hint_text("#"),
                );
                response.request_focus();
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Ok(line_number) = goto_input.parse::<usize>() {
                        log::info!("Goto to line: {}", line_number);
                        *goto_open = false;
                        *scroll_to_goto_row = goto_input.parse::<usize>().ok();
                        goto_input.clear();
                    }
                }
            });
            if !goto_window_open {
                *goto_open = goto_window_open;
            }
            let mut find_window_open = *find_open;
            show_custom_popup(ctx, &mut find_window_open, "Find", |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(find_input)
                        .desired_width(40.0)
                        .hint_text(""),
                );
                response.request_focus();
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    log::info!("Finding line: {}", find_input);
                    log::info!("Navigating to line: {}", find_input);
                    *find_open = false;
                    *scroll_to_find_row = None;
                    log::warn!("NYI - Find");
                    find_input.clear();
                }
            });
            if !find_window_open {
                *find_open = find_window_open;
            }

            let diff_ctx_ref = diff_ctx.as_ref();

            ui.separator();

            let mut scroll_to_row = scroll_to_goto_row.clone().or(scroll_to_find_row.clone());
            let mut behavior = TreeBehavior {
                ctx_file_diff: FileDiffPaneCtx {
                    diff_ctx: diff_ctx_ref,
                    scroll_left: scroll_left,
                    scroll_right: scroll_right,
                    diff_options: diff_options,
                    file_source: file_1.clone(),
                    file_target: file_2.clone(),
                    scroll_to_row: &mut scroll_to_row,
                },
            };

            ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                self.tree.ui(&mut behavior, ui);
            });

            // Invalidate diff if options changed
            if diff_options_before != *diff_options {
                app_ctx.diff_ctx_invalidated = true;
            }
            // Invalidate goto
            if scroll_to_row.is_none() {
                *scroll_to_goto_row = None;
                *scroll_to_find_row = None;
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
    }

    fn request_shutdown(&mut self) {
        if let Some(state) = self.state.take() {
            let ctx = state.into_ctx();
            self.state = Some(AppState::Exit(ctx));
        }
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

                if r.modifiers.ctrl && r.key_down(egui::Key::G) {
                    self.state
                        .as_mut()
                        .expect("State was not valid while processing inputs")
                        .ctx_mut()
                        .goto_open = true;
                }
                if r.modifiers.ctrl && r.key_down(egui::Key::F) {
                    self.state
                        .as_mut()
                        .expect("State was not valid while processing inputs")
                        .ctx_mut()
                        .find_open = true;
                }
                let ctx = self
                    .state
                    .as_mut()
                    .expect("State was not valid while processing inputs")
                    .ctx_mut();
                let max_idx = ctx
                    .diff_ctx
                    .as_ref()
                    .and_then(|f| Some(f.precomputed_diffs.len()))
                    .unwrap_or_default()
                    .saturating_sub(1);
                if r.modifiers.ctrl && r.key_pressed(egui::Key::Num1) {
                    ctx.diff_ctx_conflict_cursor = ctx.diff_ctx_conflict_cursor.saturating_sub(1);
                    log::info!("Conflict-- @{}", ctx.diff_ctx_conflict_cursor);
                }
                if r.modifiers.ctrl && r.key_pressed(egui::Key::Num2) {
                    ctx.diff_ctx_conflict_cursor = (ctx.diff_ctx_conflict_cursor + 1).min(max_idx);
                    log::info!("Conflict++ @{}", ctx.diff_ctx_conflict_cursor);
                }
            });
        }

        if user_quit {
            self.request_shutdown();
        }
    }
}

impl<T: RawTokenTrait> eframe::App for ZApp<T> {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        log::info!("SAVING...");

        #[cfg(feature = "serde")]
        if let Ok(json) = serde_json::to_string(self) {
            storage.set_string(eframe::APP_KEY, json);
        }
        log::info!("SAVED!");
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.process_ctx_inputs(ctx, frame);

        let current_state = self
            .state
            .take()
            .expect("State should always be valid during update");

        let next_state = match current_state {
            AppState::Startup(state_ctx) => {
                self.startup(ctx, frame);
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.centered_and_justified(|ui| ui.label("Loading..."));
                });
                AppState::Idle(state_ctx)
            }

            AppState::Idle(mut state) => {
                if self.update_source_target(&mut state) {
                    state.diff_ctx.take();
                    state.diff_ctx_invalidated = true;
                    log::info!("update_source_target return true, invalidating diff_ctx...");
                }
                state.diff_ctx_invalidated |= if let Some(diff_ctx) = &state.diff_ctx {
                    let hash_equal = match diff_ctx.one_sided_diff_is_left {
                        Some(is_left) => {
                            if is_left {
                                let file_1_hash = state
                                    .file_1
                                    .as_ref()
                                    .and_then(|f| Some(f.hash.clone()))
                                    .unwrap_or_default();
                                diff_ctx.file_1_hash == file_1_hash
                            } else {
                                let file_2_hash = state
                                    .file_2
                                    .as_ref()
                                    .and_then(|f| Some(f.hash.clone()))
                                    .unwrap_or_default();
                                diff_ctx.file_2_hash == file_2_hash
                            }
                        }
                        None => {
                            let file_1_hash = state
                                .file_1
                                .as_ref()
                                .and_then(|f| Some(f.hash.clone()))
                                .unwrap_or_default();
                            let file_2_hash = state
                                .file_2
                                .as_ref()
                                .and_then(|f| Some(f.hash.clone()))
                                .unwrap_or_default();
                            diff_ctx.file_1_hash == file_1_hash
                                && diff_ctx.file_2_hash == file_2_hash
                        }
                    };

                    let options_equal = diff_ctx.diff_option == state.diff_options;

                    if !hash_equal || !options_equal {
                        log::info!(
                            "diff_ctx invalidated!, reason: hash_equal: {}, options_equal: {}",
                            hash_equal,
                            options_equal
                        );
                    }
                    !hash_equal || !options_equal
                } else {
                    false
                };

                if state.diff_ctx_invalidated && !state.diff_ctx_in_progress {
                    state.diff_ctx_invalidated = false;
                    if state.file_1.is_some() || state.file_2.is_some() {
                        state.diff_ctx_in_progress = true;
                        let (tx, rx) = channel();
                        state.rx = Some(rx);

                        let f1 = state.file_1.clone();
                        let f2 = state.file_2.clone();
                        let opts = state.diff_options.clone();

                        std::thread::spawn(move || {
                            log::info!(
                                "Spawned thread for DiffCtx\nSource: {}, Target: {}",
                                f1.as_ref()
                                    .and_then(|f| Some(f.path.display().to_string()))
                                    .unwrap_or_default(),
                                f2.as_ref()
                                    .and_then(|f| Some(f.path.display().to_string()))
                                    .unwrap_or_default()
                            );
                            let result = AppStateCtx::update_diff_rows(f1, f2, &opts);
                            log::info!("Compute for DiffCtx complete!");
                            let _ = tx.send(result);
                        });
                    }
                }

                if state.diff_ctx_in_progress {
                    if let Some(rx) = &state.rx {
                        match rx.try_recv() {
                            Ok(r) => {
                                state.diff_ctx_in_progress = false;
                                state.diff_ctx = Some(r);
                                ctx.request_repaint();
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(e) => log::error!("Channel error: {e}"),
                        }
                    }
                }

                if let Some(diff_ctx) = state.diff_ctx.as_ref() {
                    let conflict_idx = diff_ctx.precomputed_diffs[state.diff_ctx_conflict_cursor];
                    state.scroll_to_goto_row = Some(conflict_idx);
                }

                self.ui(ctx, frame, &mut state);

                AppState::Idle(state)
            }

            AppState::Exit(state) => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                AppState::Exit(state)
            }
        };

        self.state = Some(next_state);
    }
}
