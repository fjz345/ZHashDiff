use eframe::egui::{self, Layout, PointerButton};
use serde::{Deserialize, Serialize};
use std::{
    env,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, channel},
    },
};
use zcommon::{hash::hash_contents, ui_egui::common::show_custom_popup};
use zdiff::{
    diff_builder::{DiffBuilderOptions, DiffRow, LineContent, build_diff_rows},
    diff_ir::{DiffIR, DiffOp},
    lexer::{
        LEXER_MODE_DEFAULT, LEXER_MODE_GREEDY, LEXER_MODE_NEWLINE, LEXER_MODE_TOKENIZE, RawToken,
    },
    myers::{MyersDiffAlgorithm, myers_count_add_deletes, myers_diff_path},
    universal_path::UniversalPath,
};

use eframe::{
    CreationContext,
    epaint::{Pos2, Vec2},
};
use egui_tiles::Tile;

use crate::{
    clamped_cursor::ClampedCursor,
    diff_ctx::{DiffCtx, DiffProcessor, UpdateDiffRowsInput},
    file::FileProcessor,
    keybindings::{Keybindings, Shortcut, ui_keybindings},
    p4::{P4Command, get_p4_config, ui_p4config, update_p4_config},
    ui_egui::{
        diff_pane::{FileDiffPane, FileDiffPaneCtx},
        panes::{Pane, TreeBehavior},
    },
};

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppStateCtx {
    pub file_1: FileProcessor,
    pub file_2: FileProcessor,
    pub diffpane_file_1_buffer: String,
    pub diffpane_file_2_buffer: String,

    pub diff_lexer_mode: u8,
    pub diff_options: DiffBuilderOptions,

    #[cfg_attr(feature = "serde", serde(skip), serde(default))]
    pub diff_processor: DiffProcessor,

    #[cfg_attr(feature = "serde", serde(skip))]
    pub myers_diff_algorithm: MyersDiffAlgorithm,

    pub scroll_left: f32,
    pub scroll_right: f32,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub scroll_to_rows: Option<(usize, Option<usize>)>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub goto_open: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub goto_input: String,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub find_open: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub find_input: String,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub find_found_lines_1: Vec<usize>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub find_found_lines_2: Vec<usize>,

    // ### Keybindings
    pub keybindings: Keybindings,
}

impl Default for AppStateCtx {
    fn default() -> Self {
        Self {
            file_1: Default::default(),
            file_2: Default::default(),
            diffpane_file_1_buffer: Default::default(),
            diffpane_file_2_buffer: Default::default(),
            diff_options: Default::default(),
            diff_processor: Default::default(),
            scroll_left: Default::default(),
            scroll_right: Default::default(),
            scroll_to_rows: Default::default(),
            goto_open: Default::default(),
            find_open: Default::default(),
            goto_input: Default::default(),
            find_input: Default::default(),
            diff_lexer_mode: LEXER_MODE_DEFAULT,
            find_found_lines_1: Default::default(),
            find_found_lines_2: Default::default(),
            keybindings: Default::default(),
            myers_diff_algorithm: Default::default(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
// #[serde(bound(serialize = "", deserialize = "T: RawToken"))]
enum AppState {
    Startup(AppStateCtx),
    Idle(AppStateCtx),
    Exit(AppStateCtx),
}

impl AppState {
    fn variant_name(&self) -> &'static str {
        match self {
            Self::Startup(_) => "Startup",
            Self::Idle(_) => "Idle",
            Self::Exit(_) => "Exit",
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        AppState::Startup(AppStateCtx::default())
    }
}

impl AppState {
    fn into_ctx(self) -> AppStateCtx {
        match self {
            AppState::Startup(ctx) | AppState::Idle(ctx) | AppState::Exit(ctx) => ctx,
        }
    }
    fn ctx_mut(&mut self) -> &mut AppStateCtx {
        match self {
            AppState::Startup(ctx) | AppState::Idle(ctx) | AppState::Exit(ctx) => ctx,
        }
    }
}

#[derive(Serialize, Deserialize)]
// #[serde(bound(serialize = "", deserialize = "T: RawToken"))]
pub struct ZApp {
    monitor_size: Vec2,
    scale_factor: f32,
    native_pixel_per_point: f32,
    // Option > Hack to avoid cloning state when matching &mut self.state in update loop
    state: Option<AppState>,
    tree: egui_tiles::Tree<Pane>,

    #[serde(skip)]
    update_diff_rows_thread_handle: Option<std::thread::JoinHandle<()>>,
    #[serde(skip)]
    update_diff_rows_cancel_flag: Option<Arc<AtomicBool>>,

    #[serde(skip)]
    open_shortcuts_window: bool,

    #[serde(skip)]
    open_universal_path_window: bool,
}

const HARDCODED_MONITOR_SIZE: Vec2 = Vec2::new(2560.0, 1440.0);
impl<'a> ZApp {
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

                    if let (Some(p1), Some(p2)) = (args.get(1), args.get(2)) {
                        ctx.file_1.set_path(UniversalPath::from(PathBuf::from(p1)));
                        ctx.file_2.set_path(UniversalPath::from(PathBuf::from(p2)));
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
            open_shortcuts_window: false,
            open_universal_path_window: false,
            update_diff_rows_thread_handle: None,
            update_diff_rows_cancel_flag: None,
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

    fn open_file_picker(tx: mpsc::Sender<UniversalPath>) {
        std::thread::spawn(move || {
            if let Some(path) = pollster::block_on(rfd::AsyncFileDialog::new().pick_file()) {
                if let Err(e) = tx.send(UniversalPath::from(path.path().to_path_buf())) {
                    log::error!("Failed to send path: {e}");
                }
            }
        });
    }

    fn refresh_file_contents(file_1: &mut FileProcessor, file_2: &mut FileProcessor) {
        file_1.invalidate_cache_file();
        file_2.invalidate_cache_file();
    }

    fn show_menu(
        &mut self,
        ui: &mut egui::Ui,
        file_1: &mut FileProcessor,
        file_2: &mut FileProcessor,
        diff_processor: &mut DiffProcessor,
        find_open: &mut bool,
        goto_open: &mut bool,
        scroll_left: &mut f32,
        scroll_right: &mut f32,
        lexer_mode: &mut u8,
        keybindings: &mut Keybindings,
        myers_diff_algorithm: &mut MyersDiffAlgorithm,
    ) {
        ui.horizontal(|ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.menu_button("File", |ui| {
                    if ui
                        .button(format!(
                            "[{}]Open Source",
                            keybindings
                                .open_file_source
                                .as_ref()
                                .map_or_else(|| "None".to_string(), Shortcut::format)
                        ))
                        .clicked()
                    {
                        Self::open_file_picker(file_1.get_tx());
                    }
                    if ui
                        .button(format!(
                            "[{}]Open Target",
                            keybindings
                                .open_file_target
                                .as_ref()
                                .map_or_else(|| "None".to_string(), Shortcut::format)
                        ))
                        .clicked()
                    {
                        Self::open_file_picker(file_2.get_tx());
                    }
                    if ui.button("Swap Source/Target").clicked() {
                        std::mem::swap(file_1, file_2);
                        std::mem::swap(scroll_left, scroll_right);

                        diff_processor.reset_ctx();
                    }
                    if ui
                        .button(format!(
                            "[{}]Find",
                            keybindings
                                .find
                                .as_ref()
                                .map_or_else(|| "None".to_string(), Shortcut::format)
                        ))
                        .clicked()
                    {
                        *find_open = true;
                    }
                    if ui
                        .button(format!(
                            "[{}]Goto",
                            keybindings
                                .goto
                                .as_ref()
                                .map_or_else(|| "None".to_string(), Shortcut::format)
                        ))
                        .clicked()
                    {
                        *goto_open = true;
                    }
                });

                ui.menu_button("Options", |ui| {
                    ui.menu_button("Myers Algo", |ui| {
                        ui.radio_value(myers_diff_algorithm, MyersDiffAlgorithm::Trace, "debug");
                        ui.radio_value(myers_diff_algorithm, MyersDiffAlgorithm::Linear, "Linear");
                        ui.radio_value(
                            myers_diff_algorithm,
                            MyersDiffAlgorithm::LinearMT,
                            "LinearMT",
                        );
                    });
                    ui.menu_button("Lexer", |ui| {
                        ui.radio_value(lexer_mode, LEXER_MODE_GREEDY, "LexerGreedy");
                        ui.radio_value(lexer_mode, LEXER_MODE_TOKENIZE, "LexerTokenize");
                        ui.radio_value(lexer_mode, LEXER_MODE_NEWLINE, "LexerNewLine");
                    });
                    if ui
                        .button(format!(
                            "[{}]P4Config",
                            keybindings
                                .open_universal_path
                                .as_ref()
                                .map_or_else(|| "None".to_string(), Shortcut::format)
                        ))
                        .clicked()
                    {
                        self.open_universal_path_window = true;
                    }
                    if ui
                        .button(format!(
                            "[{}]Keyboard Shortcuts",
                            keybindings
                                .open_options_keybindings
                                .as_ref()
                                .map_or_else(|| "None".to_string(), Shortcut::format)
                        ))
                        .clicked()
                    {
                        self.open_shortcuts_window = true;
                    }
                });

                ui.menu_button("Debug", |ui| {
                    if ui.button("Clear File Paths").clicked() {
                        file_1.set_path(UniversalPath::default());
                        file_2.set_path(UniversalPath::default());
                        diff_processor.reset_ctx();
                    }
                    if ui
                        .button(format!(
                            "[{}]Clear Cached Files",
                            keybindings
                                .refresh_diff
                                .as_ref()
                                .map_or_else(|| "None".to_string(), Shortcut::format)
                        ))
                        .clicked()
                    {
                        diff_processor.reset_ctx();
                        Self::refresh_file_contents(file_1, file_2);
                    }
                    if ui
                        .button(format!(
                            "[{}]Clear Diff Rows",
                            keybindings
                                .refresh_diff_rows_only
                                .as_ref()
                                .map_or_else(|| "None".to_string(), Shortcut::format)
                        ))
                        .clicked()
                    {
                        diff_processor.reset_ctx();
                    }
                    #[cfg(debug_assertions)]
                    {
                        let load_btn = |ui: &mut egui::Ui,
                                        label: &str,
                                        file_1: &mut FileProcessor,
                                        file_2: &mut FileProcessor,
                                        diff_processor: &mut DiffProcessor,
                                        p1: &str,
                                        p2: &str| {
                            if ui.button(label).clicked() {
                                let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

                                file_1.set_root("".into());
                                file_2.set_root("".into());
                                file_1.set_path(UniversalPath::from(base.join(p1)));
                                file_2.set_path(UniversalPath::from(base.join(p2)));

                                diff_processor.reset_ctx();
                            }
                        };

                        load_btn(
                            ui,
                            "Load $A",
                            file_1,
                            file_2,
                            diff_processor,
                            "../../test/rust_files_diff_1/advanced_rust.rs",
                            "../../test/rust_files_diff_1/advanced_rust_2.rs",
                        );

                        load_btn(
                            ui,
                            "Load $B",
                            file_1,
                            file_2,
                            diff_processor,
                            "../../test/rust_files_diff_1/imgui.1.91.1.h",
                            "../../test/rust_files_diff_1/imgui.h",
                        );

                        load_btn(
                            ui,
                            "Load $C",
                            file_1,
                            file_2,
                            diff_processor,
                            "../../test/test_ignore_whitespace_simple/1.txt",
                            "../../test/test_ignore_whitespace_simple/2.txt",
                        );

                        load_btn(
                            ui,
                            "Load $D",
                            file_1,
                            file_2,
                            diff_processor,
                            "../../test/test_ignore_whitespace_extreme_simple/1.txt",
                            "../../test/test_ignore_whitespace_extreme_simple/2.txt",
                        );
                    }
                });
            });
        });

        if self.open_shortcuts_window {
            show_custom_popup(
                ui.ctx(),
                &mut self.open_shortcuts_window,
                "Option - Shortcuts",
                true,
                |ui| {
                    ui_keybindings(ui, keybindings);
                },
            );
        }
        if self.open_universal_path_window {
            show_custom_popup(
                ui.ctx(),
                &mut self.open_universal_path_window,
                "Option - P4Config",
                true,
                |ui| {
                    let mut p4_config = get_p4_config();
                    let before_config = p4_config.clone();
                    ui_p4config(ui, &mut p4_config);
                    if p4_config != before_config {
                        log::info!("P4 config changed: {:?}", p4_config);
                        update_p4_config(p4_config);
                    }
                },
            );
        }
    }

    fn ui(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame, app_ctx: &mut AppStateCtx) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let AppStateCtx {
                scroll_left,
                scroll_right,
                diff_options,
                file_1,
                file_2,
                scroll_to_rows,
                goto_open,
                find_open,
                goto_input,
                find_input,
                diff_lexer_mode: lexer_mode,
                find_found_lines_1,
                find_found_lines_2,
                keybindings,
                myers_diff_algorithm,
                diffpane_file_1_buffer: _,
                diffpane_file_2_buffer: _,
                diff_processor,
            } = app_ctx;
            self.show_menu(
                ui,
                file_1,
                file_2,
                diff_processor,
                find_open,
                goto_open,
                scroll_left,
                scroll_right,
                lexer_mode,
                keybindings,
                myers_diff_algorithm,
            );

            ui.separator();

            let mut goto_window_open = *goto_open;
            show_custom_popup(ctx, &mut goto_window_open, "Goto", true, |ui| {
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
                        *scroll_to_rows = goto_input
                            .parse::<usize>()
                            .ok()
                            .map(|f| (f.saturating_sub(1), None));
                        goto_input.clear();
                    }
                }
            });
            if !goto_window_open {
                *goto_open = goto_window_open;
            }
            let mut find_window_open = *find_open;
            show_custom_popup(ctx, &mut find_window_open, "Find", true, |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(find_input)
                        .desired_width(40.0)
                        .hint_text(""),
                );
                response.request_focus();
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    log::info!("Finding line: {}", find_input);
                    if let Some(diff) = diff_processor.get_diff_ctx() {
                        *find_found_lines_1 = file_1
                            .get_cached_file()
                            .as_ref()
                            .map(|f| {
                                f.content_search(&find_input)
                                    .into_iter()
                                    .map(|f| diff.precomputed_file_rows.0[f])
                                    .collect()
                            })
                            .unwrap_or_default();

                        *find_found_lines_2 = file_2
                            .get_cached_file()
                            .as_ref()
                            .map(|f| {
                                f.content_search(&find_input)
                                    .into_iter()
                                    .map(|f| diff.precomputed_file_rows.0[f])
                                    .collect()
                            })
                            .unwrap_or_default();
                    }

                    log::info!("Found (in #1): {:?}", find_found_lines_1);
                    log::info!("Found (in #2): {:?}", find_found_lines_2);

                    if find_found_lines_1.len() > 0 || find_found_lines_2.len() > 0 {
                        let mut all_found_lines = find_found_lines_1.clone();
                        all_found_lines.extend(find_found_lines_2.clone());
                        all_found_lines.dedup();
                        all_found_lines.sort();
                        diff_processor
                            .find_cursor
                            .set_max(all_found_lines.len().saturating_sub(1));
                        diff_processor.find_cursor.set(0);
                        diff_processor.find_cursor.invalidate_ack();
                    }

                    find_input.clear();
                    *find_open = false;
                }
            });
            if !find_window_open {
                *find_open = find_window_open;
            }

            let DiffProcessor {
                in_progress_input,
                conflict_cursor,
                active_highlights,
                pivot,
                ..
            } = diff_processor;

            let mut conflict_cursor = diff_processor.conflict_cursor.clone();
            let mut active_highlights = diff_processor.active_highlights.clone();
            let mut pivot: (Option<usize>, Option<usize>) = diff_processor.pivot;
            let mut find_cursor = diff_processor.find_cursor.clone();

            if let Some((start, maybe_end)) = &mut scroll_to_rows.as_ref() {
                diff_processor.active_highlights.clear();
                if let Some(end) = maybe_end {
                    diff_processor.active_highlights.extend(*start..=*end);
                } else {
                    diff_processor.active_highlights.push(*start);
                }
            }

            if let Some((start, maybe_end)) = &mut scroll_to_rows.as_ref() {
                active_highlights.clear();
                if let Some(end) = maybe_end {
                    active_highlights.extend(*start..=*end);
                } else {
                    active_highlights.push(*start);
                }
            }

            let mut behavior = TreeBehavior {
                ctx_file_diff: FileDiffPaneCtx {
                    diff_ctx: diff_processor.get_diff_ctx(),
                    scroll_left: scroll_left,
                    scroll_right: scroll_right,
                    diff_options: diff_options,
                    file_source: file_1.get_cached_file().clone(),
                    file_target: file_2.get_cached_file().clone(),
                    scroll_to_row_span: &scroll_to_rows,
                    load_file_1_request: &mut None,
                    load_file_2_request: &mut None,
                    set_file_1_root_request: &mut None,
                    set_file_2_root_request: &mut None,
                    file_source_path: file_1.get_path(),
                    file_target_path: file_2.get_path(),
                    file_source_root: file_1.get_root(),
                    file_target_root: file_2.get_root(),
                    file_source_root_valid: file_1.get_loading_path().is_some()
                        || FileProcessor::is_root_valid(
                            &file_1.get_root().unwrap_or_default(),
                            &&file_1.get_full_path(),
                        ),
                    file_target_root_valid: file_2.get_loading_path().is_some()
                        || FileProcessor::is_root_valid(
                            &file_1.get_root().unwrap_or_default(),
                            &file_1.get_full_path(),
                        ),
                    file_source_path_valid: file_1.get_loading_path().is_some()
                        || file_1.get_cached_file().clone().is_some(),
                    file_target_path_valid: file_2.get_loading_path().is_some()
                        || file_2.get_cached_file().clone().is_some(),
                    file_source_loading: file_1.get_loading_path().is_some(),
                    file_target_loading: file_2.get_loading_path().is_some(),
                    active_highlights: &active_highlights,
                    conflict_cursor: &mut conflict_cursor,
                    pivot: &mut pivot,
                    find_cursor: &mut find_cursor,
                },
            };

            ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                self.tree.ui(&mut behavior, ui);
            });

            if let (Some(pivot_1), Some(pivot_2)) = (
                behavior.ctx_file_diff.pivot.0,
                behavior.ctx_file_diff.pivot.1,
            ) {
                if pivot_1 > 0 && pivot_2 > 0 {
                    behavior.ctx_file_diff.diff_options.pivot_lines = Some((pivot_1, pivot_2));
                }
            }

            // TODO: Remove clones
            let clone_find = behavior.ctx_file_diff.find_cursor.clone();
            let clone_conflict = behavior.ctx_file_diff.conflict_cursor.clone();
            let clone_pivot = behavior.ctx_file_diff.pivot.clone();

            if let Some(file_path) = behavior.ctx_file_diff.set_file_1_root_request.take() {
                log::debug!("Set new root from root_request_1: {:?}", file_path);
                app_ctx.file_1.set_root(file_path);
            }
            if let Some(file_path) = behavior.ctx_file_diff.set_file_2_root_request.take() {
                log::debug!("Set new root from root_request_2: {:?}", file_path);
                app_ctx.file_2.set_root(file_path);
            }

            if let Some(file_path) = behavior.ctx_file_diff.load_file_1_request.take() {
                log::debug!("Set new path from file_1_request: {:?}", file_path);
                app_ctx.file_1.set_path(file_path);
            }
            if let Some(file_path) = behavior.ctx_file_diff.load_file_2_request.take() {
                log::debug!("Set new path from file_1_request: {:?}", file_path);
                app_ctx.file_2.set_path(file_path);
            }

            drop(behavior);
            diff_processor.pivot = clone_pivot;
            diff_processor.find_cursor = clone_find;
            diff_processor.conflict_cursor = clone_conflict;

            *scroll_to_rows = None;

            for (_tile_id, tile) in self.tree.tiles.iter() {
                if let Tile::Pane(Pane::FileDiff(..)) = tile {
                    let source = app_ctx.file_1.get_path_as_string();
                    let target = app_ctx.file_2.get_path_as_string();
                    let total_adds = app_ctx
                        .diff_processor
                        .get_diff_ctx()
                        .as_ref()
                        .and_then(|f| Some(f.num_add_deletes))
                        .unwrap_or_default()
                        .0;
                    let total_deletes = app_ctx
                        .diff_processor
                        .get_diff_ctx()
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
        let app_state_ctx = self
            .state
            .as_mut()
            .expect("State was not valid while processing inputs")
            .ctx_mut();
        let user_quit: bool = false;
        {
            let _input_ctx = ctx.input(|r| {
                // Esc
                if r.key_down(egui::Key::Escape) {
                    // user_quit = true;
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

                if (r.modifiers.ctrl && r.key_pressed(egui::Key::Num1))
                    || (r.modifiers.alt && r.key_pressed(egui::Key::ArrowUp))
                {
                    app_state_ctx.diff_processor.conflict_cursor.dec();
                    log::info!(
                        "ConflictCursor-- @{}",
                        app_state_ctx.diff_processor.conflict_cursor.get()
                    );
                }
                if (r.modifiers.ctrl && r.key_pressed(egui::Key::Num2))
                    || (r.modifiers.alt && r.key_pressed(egui::Key::ArrowDown))
                {
                    app_state_ctx.diff_processor.conflict_cursor.inc();
                    log::info!(
                        "ConflictCursor++ @{}",
                        app_state_ctx.diff_processor.conflict_cursor.get()
                    );
                }

                if r.modifiers.shift && r.key_pressed(egui::Key::Enter) {
                    if app_state_ctx.find_found_lines_1.len() > 0
                        || app_state_ctx.find_found_lines_2.len() > 0
                    {
                        app_state_ctx.diff_processor.find_cursor.dec();
                        log::info!(
                            "FindCursor-- @{}",
                            app_state_ctx.diff_processor.find_cursor.get()
                        );
                    }
                } else if r.key_pressed(egui::Key::Enter) {
                    if app_state_ctx.find_found_lines_1.len() > 0
                        || app_state_ctx.find_found_lines_2.len() > 0
                    {
                        app_state_ctx.diff_processor.find_cursor.inc();
                        log::info!(
                            "FindCursor++ @{}",
                            app_state_ctx.diff_processor.find_cursor.get()
                        );
                    }
                }

                // ### KEYBINDINGS ###
                let handle_kb = |opt: &Option<Shortcut>, func: &mut dyn FnMut(Shortcut)| {
                    if let Some(kb) = opt {
                        if kb.matches(r) {
                            log::info!("Shortcut triggered: {}", kb.format());
                            func(*kb);
                        }
                    }
                };
                handle_kb(&app_state_ctx.keybindings.open_file_source, &mut |_kb| {
                    Self::open_file_picker(app_state_ctx.file_1.get_tx());
                });
                handle_kb(&app_state_ctx.keybindings.open_file_target, &mut |_kb| {
                    Self::open_file_picker(app_state_ctx.file_2.get_tx());
                });
                handle_kb(&app_state_ctx.keybindings.refresh_diff, &mut |_kb| {
                    app_state_ctx.diff_processor.reset_ctx();
                    Self::refresh_file_contents(
                        &mut app_state_ctx.file_1,
                        &mut app_state_ctx.file_2,
                    );
                });
                handle_kb(
                    &app_state_ctx.keybindings.refresh_diff_rows_only,
                    &mut |_kb| {
                        app_state_ctx.diff_processor.reset_ctx();
                    },
                );
                handle_kb(
                    &app_state_ctx.keybindings.open_options_keybindings,
                    &mut |_kb| self.open_shortcuts_window = true,
                );
                handle_kb(&app_state_ctx.keybindings.open_universal_path, &mut |_kb| {
                    self.open_universal_path_window = true
                });
                handle_kb(&app_state_ctx.keybindings.find, &mut |_kb| {
                    app_state_ctx.find_open = true
                });
                handle_kb(&app_state_ctx.keybindings.goto, &mut |_kb| {
                    app_state_ctx.goto_open = true
                });
                handle_kb(
                    &app_state_ctx.keybindings.revision_graph,
                    &mut |_kb| match &app_state_ctx.file_1.get_full_path() {
                        UniversalPath::Depot(path, _rev) => {
                            match P4Command::open_revision_graph(path) {
                                Ok(_) => {
                                    log::info!("Revision graph returned Ok");
                                }
                                Err(e) => log::error!("Failed to open revision graph: {e}"),
                            }
                        }
                        UniversalPath::Local(path_buf) => {
                            log::info!(
                                "Can not open revision graph for local path {}",
                                path_buf.display()
                            );
                            return;
                        }
                    },
                );
                handle_kb(
                    &app_state_ctx.keybindings.timelapse_view,
                    &mut |_kb| match &app_state_ctx.file_1.get_full_path() {
                        UniversalPath::Depot(path, _rev) => {
                            match P4Command::open_timelapse_view(path) {
                                Ok(_) => {
                                    log::info!("Timelapse view returned Ok");
                                }
                                Err(e) => log::error!("Failed to open timelapse view: {e}"),
                            }
                        }
                        UniversalPath::Local(path_buf) => {
                            log::info!(
                                "Can not open timelapse view for local path {}",
                                path_buf.display()
                            );
                            return;
                        }
                    },
                );

                for (i, (kb, path)) in app_state_ctx
                    .keybindings
                    .user_quick_diffs
                    .iter()
                    .enumerate()
                {
                    handle_kb(kb, &mut |kb| {
                        log::info!(
                            "User Quick Diff Shortcut [{}] triggered: {}",
                            i + 1,
                            kb.format()
                        );

                        if let Some((source_root, source_path)) = &path.source {
                            app_state_ctx
                                .file_1
                                .set_root(UniversalPath::from(source_root));

                            app_state_ctx
                                .file_1
                                .set_path(UniversalPath::from(source_path));
                        }

                        // Don't set any target paths if root & path is ""
                        if !(path.target.0.is_empty() && path.target.1.is_empty()) {
                            let target_path = if path.target.1.is_empty() {
                                // Use sources file path split from root
                                &app_state_ctx.file_1.get_path().to_string()
                            } else {
                                &path.target.1
                            };

                            app_state_ctx
                                .file_2
                                .set_root(UniversalPath::from(&path.target.0));
                            app_state_ctx
                                .file_2
                                .set_path(UniversalPath::from(target_path));
                        }

                        if path.source.is_some() {
                            log::info!(
                                "User Quick Diff Shortcut set paths:\nSource: {:?}\nTarget: {:?}",
                                app_state_ctx.file_1.get_full_path(),
                                app_state_ctx.file_2.get_full_path()
                            );
                        } else {
                            log::info!(
                                "User Quick Diff Shortcut set paths:\nTarget: {:?}",
                                app_state_ctx.file_2.get_full_path()
                            );
                        }
                    });
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
            storage.set_string(eframe::APP_KEY, json);
        }
        log::info!("SAVED!");
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Update conflict cursor before input processing
        {
            let app_ctx = self
                .state
                .as_mut()
                .expect("State was not valid while processing inputs")
                .ctx_mut();
            let conflict_max = app_ctx
                .diff_processor
                .get_diff_ctx()
                .as_ref()
                .and_then(|f| Some(f.precomputed_diffs.len()))
                .unwrap_or_default();
            app_ctx.diff_processor.conflict_cursor.set_max(conflict_max);
        }

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
                state.file_1.set_lexer_mode(state.diff_lexer_mode);
                state.file_2.set_lexer_mode(state.diff_lexer_mode);

                let update_input = UpdateDiffRowsInput {
                    file_1: state.file_1.get_cached_file().clone(),
                    file_2: state.file_2.get_cached_file().clone(),
                    options: state.diff_options.clone(),
                    myers_diff_algorithm: state.myers_diff_algorithm.clone(),
                };

                let diff_ctx_invalidated = if state.file_1.get_loading_path().is_some()
                    || state.file_2.get_loading_path().is_some()
                {
                    false
                } else if let Some(in_progress_input) = &state.diff_processor.in_progress_input {
                    *in_progress_input != update_input
                } else if let Some(diff_ctx) = state.diff_processor.get_diff_ctx() {
                    let input_equal = update_input == diff_ctx.update_diff_rows_input;

                    if !input_equal && !state.diff_processor.in_progress_input.is_some() {
                        log::debug!("diff_ctx invalidated!");
                    }
                    !input_equal
                } else {
                    !state.diff_processor.in_progress_input.is_some()
                };

                if diff_ctx_invalidated
                    && (state.file_1.get_cached_file().is_some()
                        || state.file_2.get_cached_file().is_some())
                {
                    state.diff_processor.request_update(update_input);
                }

                state.diff_processor.poll_diff_channel();

                if let Some(conflict_cursor) = state.diff_processor.update_conflict_cursor() {
                    state.scroll_to_rows = Some(conflict_cursor);
                }
                if let Some(find_cursor) = state
                    .diff_processor
                    .update_find(&state.find_found_lines_1, &state.find_found_lines_2)
                {
                    state.scroll_to_rows = Some(find_cursor);
                }

                if let Some(scroll_to) = &state.scroll_to_rows {
                    log::info!("Navigating to line: {:?}", scroll_to);
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
