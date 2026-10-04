use eframe::egui::{self, Layout, PointerButton, containers::menu::MenuConfig};
use serde::{Deserialize, Serialize};
use std::{
    env,
    path::{Path, PathBuf},
    sync::mpsc,
};
use zcommon::ui_egui::common::show_custom_popup;
use zdiff::{
    diff_builder::{DiffBuilderOptions, PivotLines},
    lexer::{LEXER_MODE_DEFAULT, LEXER_MODE_GREEDY, LEXER_MODE_NEWLINE, LEXER_MODE_TOKENIZE},
    myers::MyersDiffAlgorithm,
    universal_path::UniversalPath,
};

use eframe::{
    CreationContext,
    epaint::{Pos2, Vec2},
};
use egui_tiles::Tile;

use crate::{
    diff_ctx::{DiffProcessor, FindCtx, UpdateDiffRowsInput},
    file::{FileProcessor, LoadedFile},
    keybindings::{Keybindings, QuickDiffPaths, Shortcut, ui_keybindings},
    p4::{P4Command, get_p4_config, ui_p4config, update_p4_config},
    revert::{
        self, HistoryStep, PendingP4Edit, RevertHistory, RevertRecord, RevertRefusal, RevertTarget,
        RevertWrite, WriteRefusal,
    },
    ui_egui::{
        diff_pane::{FileDiffPane, FileDiffPaneCtx},
        panes::{Pane, TreeBehavior},
    },
    viewer::{
        ExtensionMap, LoadedSide, ViewerKind, ViewerOverride, conflict_count,
        hex::{HexDiffProcessor, parse_hex_offset},
        image::ImageDiffProcessor,
        image_decode_fallback, resolve_viewer, ui_extension_map,
    },
};

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppStateCtx {
    pub file_1: FileProcessor,
    pub file_2: FileProcessor,

    #[cfg_attr(feature = "serde", serde(skip), serde(default))]
    pub diff_processor: DiffProcessor,
    #[cfg_attr(feature = "serde", serde(skip), serde(default))]
    pub hex_processor: HexDiffProcessor,
    #[cfg_attr(feature = "serde", serde(skip), serde(default))]
    pub image_processor: ImageDiffProcessor,
    /// Resolved each frame from the loaded files; `None` while neither side is loaded.
    #[cfg_attr(feature = "serde", serde(skip), serde(default))]
    pub viewer_kind: Option<ViewerKind>,
    /// Not persisted: paths aren't either, so a restart always opens a new pair.
    #[cfg_attr(feature = "serde", serde(skip), serde(default))]
    pub viewer_override: ViewerOverride,
    /// The last logged reason the pair isn't in the viewer it asked for.
    #[cfg_attr(feature = "serde", serde(skip), serde(default))]
    pub viewer_fallback: Option<String>,
    #[cfg_attr(feature = "serde", serde(default))]
    pub extension_map: ExtensionMap,
    /// Undo and redo of reverts; in memory only.
    #[cfg_attr(feature = "serde", serde(skip), serde(default))]
    pub revert_history: RevertHistory,

    pub diff_lexer_mode: u8,
    pub diff_options: DiffBuilderOptions,

    pub myers_diff_algorithm: MyersDiffAlgorithm,
    pub code_language: String,
    pub code_language_custom: String,

    // ### Keybindings
    pub keybindings: Keybindings,

    // ### UI TEMP
    pub scroll_left: f32,
    pub scroll_right: f32,
    // Saves from before the setting load linked, like a fresh state.
    #[cfg_attr(feature = "serde", serde(default = "default_h_scroll_linked"))]
    pub h_scroll_linked: bool,
    #[cfg_attr(feature = "serde", serde(default))]
    pub word_wrap: bool,

    #[cfg_attr(feature = "serde", serde(skip))]
    pub goto_open: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub goto_input: String,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub find_open: bool,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub find_input: String,
}

fn default_h_scroll_linked() -> bool {
    true
}

impl Default for AppStateCtx {
    fn default() -> Self {
        Self {
            file_1: Default::default(),
            file_2: Default::default(),
            diff_options: Default::default(),
            diff_processor: Default::default(),
            hex_processor: Default::default(),
            image_processor: Default::default(),
            viewer_kind: None,
            viewer_override: Default::default(),
            viewer_fallback: None,
            extension_map: Default::default(),
            revert_history: Default::default(),
            scroll_left: Default::default(),
            scroll_right: Default::default(),
            h_scroll_linked: default_h_scroll_linked(),
            word_wrap: false,
            goto_open: Default::default(),
            find_open: Default::default(),
            goto_input: Default::default(),
            find_input: Default::default(),
            diff_lexer_mode: LEXER_MODE_DEFAULT,
            keybindings: Default::default(),
            myers_diff_algorithm: Default::default(),
            code_language: "rs".to_string(),
            code_language_custom: "".into(),
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
    open_shortcuts_window: bool,
    #[serde(skip)]
    open_universal_path_window: bool,
    #[serde(skip)]
    open_viewers_window: bool,
    /// A revert waiting on the p4 edit prompt.
    #[serde(skip)]
    pending_p4_edit: Option<(RevertTarget, PendingP4Edit)>,
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
            open_viewers_window: false,
            pending_p4_edit: None,
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

    /// Records a written revert for undo and reloads both sides; reloads the target when it was
    /// stale.
    fn finish_revert_write(
        result: Result<RevertRecord, WriteRefusal>,
        path: &Path,
        target: RevertTarget,
        revert_history: &mut RevertHistory,
        diff_processor: &mut DiffProcessor,
        file_1: &mut FileProcessor,
        file_2: &mut FileProcessor,
    ) {
        match result {
            Ok(record) => {
                revert_history.record(record);
                diff_processor.reset_ctx();
                Self::refresh_file_contents(file_1, file_2);
            }
            Err(WriteRefusal::Stale) => {
                log::error!(
                    "Revert refused: {} changed on disk since it was loaded. Reloading it.",
                    path.display()
                );
                diff_processor.reset_ctx();
                match target {
                    RevertTarget::Left => file_1.invalidate_cache_file(),
                    RevertTarget::Right => file_2.invalidate_cache_file(),
                }
            }
            Err(e) => log::error!("Revert refused for {}: {}", path.display(), e),
        }
    }

    /// Undoes or redoes the latest revert in this diff. Both sides reload after a write, and
    /// after a refusal for a file that changed on disk.
    fn step_revert_history(app_ctx: &mut AppStateCtx, step: HistoryStep) {
        // A path picked or set by a Quick Diff since the last frame makes this another diff.
        let (path_1, path_2) = (
            app_ctx.file_1.get_full_path(),
            app_ctx.file_2.get_full_path(),
        );
        app_ctx.revert_history.observe_pair(&path_1, &path_2);
        let (name, since) = match step {
            HistoryStep::Undo => ("Undo", "the revert"),
            HistoryStep::Redo => ("Redo", "the undo"),
        };
        let stepped = app_ctx
            .revert_history
            .step(step, revert::write_history_step(&std::env::temp_dir()));
        let reload = match stepped {
            None => {
                log::info!("{name}: no revert to {}", name.to_lowercase());
                false
            }
            Some((path, Ok(()))) => {
                log::info!("{name} of a revert written to {}", path.display());
                true
            }
            Some((path, Err(WriteRefusal::Stale))) => {
                log::error!(
                    "{name} refused: {} changed on disk since {since}. Reloading it.",
                    path.display()
                );
                true
            }
            Some((path, Err(e))) => {
                log::error!("{name} refused for {}: {}", path.display(), e);
                false
            }
        };
        if reload {
            app_ctx.diff_processor.reset_ctx();
            Self::refresh_file_contents(&mut app_ctx.file_1, &mut app_ctx.file_2);
        }
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
        extension_map: &mut ExtensionMap,
        myers_diff_algorithm: &mut MyersDiffAlgorithm,
        code_language: &mut String,
        code_language_custom: &mut String,
    ) {
        ui.horizontal(|ui| {
            egui::MenuBar::new()
                .config(
                    MenuConfig::new().close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside),
                )
                .ui(ui, |ui| {
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
                            ui.radio_value(
                                myers_diff_algorithm,
                                MyersDiffAlgorithm::Trace,
                                "Trace",
                            );
                            ui.radio_value(
                                myers_diff_algorithm,
                                MyersDiffAlgorithm::Linear,
                                "Linear",
                            );
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
                        ui.menu_button("Code Language", |ui| {
                            ui.radio_value(code_language, "rs".to_string(), "Rust (.rs)");
                            ui.radio_value(code_language, "py".to_string(), "Python (.py)");
                            ui.radio_value(code_language, "cpp".to_string(), "C++ (.cpp)");
                            ui.radio_value(code_language, "js".to_string(), "JavaScript (.js)");
                            ui.radio_value(code_language, "json".to_string(), "JSON (.json)");
                            ui.radio_value(code_language, "md".to_string(), "Markdown (.md)");
                            ui.separator();
                            ui.horizontal(|ui| {
                                if ui
                                    .radio(
                                        code_language == code_language_custom
                                            && !code_language_custom.is_empty(),
                                        "Use Custom",
                                    )
                                    .clicked()
                                {
                                    *code_language = code_language_custom.trim().to_string();
                                }
                                let text_edit = egui::TextEdit::singleline(code_language_custom)
                                    .hint_text("Custom (e.g. go, html)");
                                let res = ui.add(text_edit);
                                if res.changed() && !code_language_custom.is_empty() {
                                    *code_language = code_language_custom.trim().to_string();
                                }
                            });
                        });
                        *code_language = code_language.trim_matches('.').to_string();
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
                        if ui.button("File Viewers").clicked() {
                            self.open_viewers_window = true;
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
        if self.open_viewers_window {
            show_custom_popup(
                ui.ctx(),
                &mut self.open_viewers_window,
                "Option - File Viewers",
                true,
                |ui| {
                    ui_extension_map(ui, extension_map);
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
                h_scroll_linked,
                word_wrap,
                diff_options,
                file_1,
                file_2,
                goto_open,
                find_open,
                goto_input,
                find_input,
                diff_lexer_mode: lexer_mode,
                keybindings,
                myers_diff_algorithm,
                diff_processor,
                hex_processor,
                image_processor,
                viewer_kind,
                viewer_override,
                viewer_fallback,
                extension_map,
                revert_history,
                code_language,
                code_language_custom,
            } = app_ctx;
            let is_hex = *viewer_kind == Some(ViewerKind::Hex);
            let is_image = *viewer_kind == Some(ViewerKind::Image);
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
                extension_map,
                myers_diff_algorithm,
                code_language,
                code_language_custom,
            );

            ui.separator();

            let mut goto_window_open = *goto_open;
            show_custom_popup(ctx, &mut goto_window_open, "Goto", true, |ui| {
                // Hex gotos a byte offset, in decimal or 0x hex.
                if is_hex {
                    goto_input.retain(|c| c.is_ascii_hexdigit() || c == 'x' || c == 'X');
                } else {
                    goto_input.retain(|c| c.is_ascii_digit());
                }
                let response = ui.add(
                    egui::TextEdit::singleline(goto_input)
                        .desired_width(40.0)
                        .hint_text(if is_hex { "offset" } else { "#" }),
                );
                response.request_focus();
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if is_hex {
                        match parse_hex_offset(goto_input, hex_processor.max_len()) {
                            Ok(offset) => {
                                hex_processor.goto(offset);
                                *goto_open = false;
                            }
                            Err(e) => log::error!("Goto offset {:?}: {}", goto_input, e),
                        }
                    } else if let Ok(line_number) = goto_input.parse::<usize>() {
                        diff_processor.update_goto(Some(line_number));
                        *goto_open = false;
                    }
                }
            });
            if !goto_window_open {
                goto_input.clear();
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
                    if let Some(ctx) = &diff_processor.get_minimal_diff_ctx() {
                        let find_ctx = FindCtx::new(find_input, ctx);
                        diff_processor.update_find(find_ctx);
                    }

                    find_input.clear();
                    *find_open = false;
                }
            });
            if !find_window_open {
                find_input.clear();
                *find_open = find_window_open;
            }

            // The text diff's state is stale while another viewer shows the pair.
            let scroll_to_rows = &if is_hex {
                hex_processor.scroll_to_row(diff_processor.conflict_cursor.get())
            } else if is_image {
                None
            } else {
                diff_processor.get_scroll_to_row()
            };
            if let Some(scroll_to) = scroll_to_rows {
                log::info!("Navigating to line: {:?}", scroll_to);
            }

            let active_highlights = diff_processor.active_highlights.clone();
            let mut conflict_cursor = diff_processor.conflict_cursor.clone();
            let mut pivot: (Option<usize>, Option<usize>) = diff_processor.pivot;
            let mut find_cursor = diff_processor.find_cursor.clone();
            let mut active_side = diff_processor.active_side;

            let diff_ctx = if is_hex || is_image {
                None
            } else {
                diff_processor.get_minimal_diff_ctx()
            };
            let mut block_toggle_request = None;
            let mut behavior = TreeBehavior {
                ctx_file_diff: FileDiffPaneCtx {
                    diff_ctx: diff_ctx.as_ref(),
                    scroll_left: scroll_left,
                    scroll_right: scroll_right,
                    h_scroll_linked,
                    word_wrap,
                    diff_options: diff_options,
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
                        || file_1.get_loaded_file().is_some(),
                    file_target_path_valid: file_2.get_loading_path().is_some()
                        || file_2.get_loaded_file().is_some(),
                    file_source_loading: file_1.get_loading_path().is_some(),
                    file_target_loading: file_2.get_loading_path().is_some(),
                    active_highlights: &active_highlights,
                    conflict_cursor: &mut conflict_cursor,
                    pivot: &mut pivot,
                    find_cursor: &mut find_cursor,
                    active_side: &mut active_side,
                    diff_loading: diff_processor.is_in_progress(),
                    code_language,
                    revert_request: &mut None,
                    block_toggle_request: &mut block_toggle_request,
                    hex_view: is_hex.then(|| hex_processor.view_ctx()),
                    image_view: is_image.then_some(image_processor),
                    viewer_override: &mut viewer_override.kind,
                    viewer_fallback: viewer_fallback.as_deref(),
                },
            };

            ui.with_layout(Layout::left_to_right(egui::Align::Min), |ui| {
                self.tree.ui(&mut behavior, ui);
            });

            if let (Some(pivot_left), Some(pivot_right)) = (
                behavior.ctx_file_diff.pivot.0,
                behavior.ctx_file_diff.pivot.1,
            ) {
                if pivot_left > 0 && pivot_right > 0 {
                    behavior.ctx_file_diff.diff_options.pivot_lines = Some(PivotLines {
                        left: pivot_left,
                        right: pivot_right,
                    });
                }
            }

            // TODO: Remove clones
            let clone_find = behavior.ctx_file_diff.find_cursor.clone();
            let clone_active_side = *behavior.ctx_file_diff.active_side;
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
            if let Some(revert_request) = *behavior.ctx_file_diff.revert_request {
                log::debug!("new revert request: {:?}", revert_request);
                // Plan against the files the shown rows were built from, not the latest loads.
                let planned = behavior
                    .ctx_file_diff
                    .diff_ctx
                    .ok_or(RevertRefusal::NoSuchHunk)
                    .and_then(|diff_ctx| {
                        revert::plan_hunk_revert(diff_ctx, revert_request, &std::env::temp_dir())
                    });
                match planned {
                    Ok(planned) => {
                        let path = planned.path.clone();
                        let written = revert::write_revert(
                            planned,
                            &std::env::temp_dir(),
                            &P4Command::new(false).for_file(UniversalPath::Local(path.clone())),
                        );
                        let result = match written {
                            Ok(RevertWrite::NeedsP4Edit(pending)) => {
                                log::info!(
                                    "{} is read-only and Perforce-managed, asking to p4 edit it",
                                    path.display()
                                );
                                self.pending_p4_edit = Some((revert_request.target, pending));
                                None
                            }
                            Ok(RevertWrite::Written(record)) => Some(Ok(record)),
                            Err(e) => Some(Err(e)),
                        };
                        if let Some(result) = result {
                            Self::finish_revert_write(
                                result,
                                &path,
                                revert_request.target,
                                revert_history,
                                diff_processor,
                                &mut app_ctx.file_1,
                                &mut app_ctx.file_2,
                            );
                        }
                    }
                    Err(refusal) => {
                        log::error!("Revert refused: {} {:?}", refusal, revert_request);
                    }
                }
            }

            if let Some((target, pending)) = self.pending_p4_edit.take() {
                let mut confirmed = None;
                let modal = egui::Modal::new(egui::Id::new("p4_edit_prompt")).show(ctx, |ui| {
                    ui.heading("Check out for edit?");
                    ui.label(format!(
                        "{} is read-only and managed by Perforce.",
                        pending.path().display()
                    ));
                    ui.label("Run p4 edit on it and apply the revert?");
                    ui.horizontal(|ui| {
                        if ui.button("p4 edit and revert").clicked() {
                            confirmed = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            confirmed = Some(false);
                        }
                    });
                });
                // Escape or a click outside the prompt declines.
                if modal.should_close() {
                    confirmed.get_or_insert(false);
                }
                match confirmed {
                    Some(true) => {
                        let path = pending.path().to_path_buf();
                        let p4 = P4Command::new(false).for_file(UniversalPath::Local(path.clone()));
                        let result = pending.confirm(&std::env::temp_dir(), &p4);
                        Self::finish_revert_write(
                            result,
                            &path,
                            target,
                            revert_history,
                            diff_processor,
                            &mut app_ctx.file_1,
                            &mut app_ctx.file_2,
                        );
                    }
                    Some(false) => {
                        log::info!(
                            "Revert cancelled: p4 edit declined for {}",
                            pending.path().display()
                        );
                    }
                    None => self.pending_p4_edit = Some((target, pending)),
                }
            }

            drop(behavior);
            diff_processor.pivot = clone_pivot;
            diff_processor.find_cursor = clone_find;
            diff_processor.active_side = clone_active_side;
            diff_processor.conflict_cursor = clone_conflict;
            if let Some((key, toggle)) = block_toggle_request {
                diff_processor.toggle_block(key, toggle);
                ctx.request_repaint();
            }

            for (_tile_id, tile) in self.tree.tiles.iter() {
                if let Tile::Pane(Pane::FileDiff(..)) = tile {
                    let source = app_ctx.file_1.get_path_as_string();
                    let target = app_ctx.file_2.get_path_as_string();
                    let total_adds = app_ctx
                        .diff_processor
                        .get_minimal_diff_ctx()
                        .as_ref()
                        .and_then(|f| Some(f.num_add_deletes))
                        .unwrap_or_default()
                        .0;
                    let total_deletes = app_ctx
                        .diff_processor
                        .get_minimal_diff_ctx()
                        .and_then(|f| Some(f.num_add_deletes))
                        .unwrap_or_default()
                        .1;
                    let counts = if is_hex {
                        "hex".to_string()
                    } else if is_image {
                        "image".to_string()
                    } else {
                        format!("+{}/-{}", total_adds, total_deletes)
                    };
                    ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!(
                        "zdiff [{}] - {}, {}",
                        counts, source, target
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
        // Read outside ctx.input: Context methods inside its closure can deadlock.
        let text_focused = ctx.wants_keyboard_input();
        let mut history_step = None;
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
                    app_state_ctx.diff_processor.find_cursor.dec();
                    log::info!(
                        "FindCursor-- @{}",
                        app_state_ctx.diff_processor.find_cursor.get()
                    );
                } else if r.key_pressed(egui::Key::Enter) {
                    app_state_ctx.diff_processor.find_cursor.inc();
                    log::info!(
                        "FindCursor++ @{}",
                        app_state_ctx.diff_processor.find_cursor.get()
                    );
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
                        file @ UniversalPath::Depot(..) => {
                            match P4Command::open_revision_graph(file) {
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
                        file @ UniversalPath::Depot(..) => {
                            match P4Command::open_timelapse_view(file) {
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
                // A focused text field (path editors, Find, Goto, a binding being captured) has
                // its own Ctrl+Z/Ctrl+Y; they must not also rewrite a file on disk.
                if !text_focused {
                    handle_kb(&app_state_ctx.keybindings.undo_revert, &mut |_kb| {
                        history_step = Some(HistoryStep::Undo)
                    });
                    handle_kb(&app_state_ctx.keybindings.redo_revert, &mut |_kb| {
                        history_step = Some(HistoryStep::Redo)
                    });
                    handle_kb(&app_state_ctx.keybindings.redo_revert_alt, &mut |_kb| {
                        history_step = Some(HistoryStep::Redo)
                    });
                }

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

                        if let Err(refusal) = apply_quick_diff(
                            path,
                            &mut app_state_ctx.file_1,
                            &mut app_state_ctx.file_2,
                            &std::env::temp_dir(),
                        ) {
                            log::warn!(
                                "User Quick Diff Shortcut [{}] disabled: {}:\n{:?}\n{:?}",
                                i + 1,
                                refusal,
                                app_state_ctx.file_1.get_full_path(),
                                app_state_ctx.file_2.get_full_path()
                            );
                        } else if path.source.is_some() {
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
        // After the input closure: this writes to disk.
        if let Some(step) = history_step {
            Self::step_revert_history(app_state_ctx, step);
        }

        if user_quit {
            self.request_shutdown();
        }
    }
}

impl eframe::App for ZApp {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        log::debug!("SAVING...");
        #[cfg(feature = "serde")]
        if let Ok(json) = serde_json::to_string(self) {
            storage.set_string(eframe::APP_KEY, json);
        }
        log::debug!("SAVED!");
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        // Update conflict cursor before input processing
        {
            let app_ctx = self
                .state
                .as_mut()
                .expect("State was not valid while processing inputs")
                .ctx_mut();
            let text_conflicts = if matches!(
                app_ctx.viewer_kind,
                Some(ViewerKind::Hex | ViewerKind::Image)
            ) {
                0
            } else {
                app_ctx
                    .diff_processor
                    .get_minimal_diff_ctx()
                    .as_ref()
                    .and_then(|f| Some(f.precomputed_diffs.len()))
                    .unwrap_or_default()
            };
            let conflict_max = conflict_count(
                app_ctx.viewer_kind,
                text_conflicts,
                app_ctx.hex_processor.nav_count(),
            );
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

                let loaded_1 = state.file_1.get_loaded_file();
                let loaded_2 = state.file_2.get_loaded_file();
                let path_1 = state.file_1.get_full_path();
                let path_2 = state.file_2.get_full_path();
                state.viewer_override.observe_pair(&path_1, &path_2);
                state.revert_history.observe_pair(&path_1, &path_2);
                fn side(f: &LoadedFile) -> LoadedSide<'_> {
                    LoadedSide {
                        path: f.path(),
                        sniffed: f.viewer_kind(),
                    }
                }
                let resolution = resolve_viewer(
                    state.viewer_override.kind,
                    &state.extension_map,
                    loaded_1.as_ref().map(side),
                    loaded_2.as_ref().map(side),
                );
                // Decoding follows the first resolution, so a failed decode stays cached (and
                // the pair stays in Hex) instead of being retried every frame.
                if resolution.as_ref().map(|r| r.kind) == Some(ViewerKind::Image) {
                    state
                        .image_processor
                        .request(loaded_1.clone(), loaded_2.clone());
                } else {
                    // Drops the old pair's pixels and textures.
                    state.image_processor.request(None, None);
                }
                state
                    .image_processor
                    .poll(ctx.input(|i| i.max_texture_side));
                let resolution =
                    resolution.map(|r| image_decode_fallback(r, &state.image_processor.failures()));
                // Resolved every frame, so only a changed reason is logged.
                let fallback = resolution.as_ref().and_then(|r| r.fallback.clone());
                if fallback != state.viewer_fallback {
                    if let Some(reason) = &fallback {
                        log::warn!("{}", reason);
                    }
                    state.viewer_fallback = fallback;
                }
                state.viewer_kind = resolution.map(|r| r.kind);
                let is_hex = state.viewer_kind == Some(ViewerKind::Hex);
                let is_text = matches!(state.viewer_kind, Some(ViewerKind::Text) | None);
                if is_hex {
                    // The cursor may hold the text diff's or the previous pair's stop.
                    if state.hex_processor.request(loaded_1, loaded_2) {
                        state.diff_processor.conflict_cursor.set(0);
                    }
                } else {
                    // Cancels a running compare and drops the old pair's bytes.
                    state.hex_processor.request(None, None);
                }
                state.hex_processor.poll();

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
                } else if let Some(diff_ctx) = state.diff_processor.get_minimal_diff_ctx() {
                    let input_equal = update_input == diff_ctx.input;

                    if !input_equal && !state.diff_processor.in_progress_input.is_some() {
                        log::debug!("diff_ctx invalidated!");
                    }
                    !input_equal
                } else {
                    !state.diff_processor.in_progress_input.is_some()
                };

                if diff_ctx_invalidated
                    && is_text
                    && (state.file_1.get_cached_file().is_some()
                        || state.file_2.get_cached_file().is_some())
                {
                    state.diff_processor.request_update(update_input);
                }

                state.diff_processor.update();

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

/// Why a Quick Diff left both sides as they were.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuickDiffRefusal {
    BothTemp,
}

impl std::fmt::Display for QuickDiffRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            QuickDiffRefusal::BothTemp => {
                "both sides are temporary files, so neither has a workspace path to diff against"
            }
        })
    }
}

/// Points the sides at a Quick Diff slot's paths. A slot without a target path diffs the same
/// root-relative path under its target root. That path comes from the left side, or from the
/// right side when the left one is a temp copy, as when p4 launches an external diff. With both
/// sides temp, nothing changes.
fn apply_quick_diff(
    slot: &QuickDiffPaths,
    file_1: &mut FileProcessor,
    file_2: &mut FileProcessor,
    temp_root: &Path,
) -> Result<(), QuickDiffRefusal> {
    let is_temp = |file: &mut FileProcessor| file.get_full_path().is_temp(temp_root);
    if is_temp(file_1) && is_temp(file_2) {
        return Err(QuickDiffRefusal::BothTemp);
    }
    if let Some((source_root, source_path)) = &slot.source {
        file_1.set_root(UniversalPath::from(source_root));
        file_1.set_path(UniversalPath::from(source_path));
    }

    // Don't set any target paths if root & path is ""
    if !(slot.target.0.is_empty() && slot.target.1.is_empty()) {
        let target_path = if slot.target.1.is_empty() {
            // The sides as they are now, so a slot source counts as the left side. Read before
            // file_2 gets the target root.
            let identity = if is_temp(file_1) {
                &mut *file_2
            } else {
                &mut *file_1
            };
            &identity.get_path().to_string()
        } else {
            &slot.target.1
        };

        file_2.set_root(UniversalPath::from(&slot.target.0));
        file_2.set_path(UniversalPath::from(target_path));
    }
    Ok(())
}

#[cfg(all(test, feature = "serde"))]
mod tests {
    use super::*;

    #[test]
    fn extension_map_edits_survive_a_restart() {
        let mut ctx = AppStateCtx::default();
        ctx.extension_map.insert(".ZBIN", ViewerKind::Hex);
        ctx.viewer_override.kind = Some(ViewerKind::Text);

        let json = serde_json::to_string(&ctx).unwrap();
        let restored: AppStateCtx = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.extension_map, ctx.extension_map);
        assert_eq!(restored.extension_map.get("zbin"), Some(ViewerKind::Hex));
        // Paths aren't saved, so a restart opens a new pair and the override is gone.
        assert_eq!(restored.viewer_override.kind, None);
    }

    #[test]
    fn a_save_without_an_extension_map_loads_the_defaults() {
        let mut json = serde_json::to_value(AppStateCtx::default()).unwrap();
        json.as_object_mut()
            .unwrap()
            .remove("extension_map")
            .unwrap();
        let restored: AppStateCtx = serde_json::from_value(json).unwrap();
        assert_eq!(restored.extension_map, ExtensionMap::defaults());
    }

    #[test]
    fn horizontal_scroll_link_defaults_on_and_survives_a_restart() {
        assert!(AppStateCtx::default().h_scroll_linked);

        let mut ctx = AppStateCtx::default();
        ctx.h_scroll_linked = false;
        let json = serde_json::to_string(&ctx).unwrap();
        let restored: AppStateCtx = serde_json::from_str(&json).unwrap();
        assert!(!restored.h_scroll_linked);
    }

    #[test]
    fn a_save_without_the_horizontal_scroll_link_loads_linked() {
        let mut json = serde_json::to_value(AppStateCtx::default()).unwrap();
        json.as_object_mut()
            .unwrap()
            .remove("h_scroll_linked")
            .unwrap();
        let restored: AppStateCtx = serde_json::from_value(json).unwrap();
        assert!(restored.h_scroll_linked);
    }

    #[test]
    fn word_wrap_defaults_off_and_survives_a_restart() {
        assert!(!AppStateCtx::default().word_wrap);

        let mut ctx = AppStateCtx::default();
        ctx.word_wrap = true;
        let json = serde_json::to_string(&ctx).unwrap();
        let restored: AppStateCtx = serde_json::from_str(&json).unwrap();
        assert!(restored.word_wrap);
    }

    #[test]
    fn a_save_without_word_wrap_loads_unwrapped() {
        let mut json = serde_json::to_value(AppStateCtx::default()).unwrap();
        json.as_object_mut().unwrap().remove("word_wrap").unwrap();
        let restored: AppStateCtx = serde_json::from_value(json).unwrap();
        assert!(!restored.word_wrap);
    }

    #[test]
    fn a_save_without_the_undo_keys_loads_the_defaults() {
        let mut json = serde_json::to_value(AppStateCtx::default()).unwrap();
        let keybindings = json["keybindings"].as_object_mut().unwrap();
        for key in ["undo_revert", "redo_revert", "redo_revert_alt"] {
            keybindings.remove(key).unwrap();
        }
        let restored: AppStateCtx = serde_json::from_value(json).unwrap();
        let defaults = Keybindings::default();
        assert!(defaults.undo_revert.is_some());
        assert!(defaults.redo_revert.is_some());
        assert!(defaults.redo_revert_alt.is_some());
        assert_eq!(restored.keybindings.undo_revert, defaults.undo_revert);
        assert_eq!(restored.keybindings.redo_revert, defaults.redo_revert);
        assert_eq!(
            restored.keybindings.redo_revert_alt,
            defaults.redo_revert_alt
        );
    }

    #[test]
    fn undo_keys_survive_a_restart() {
        let mut ctx = AppStateCtx::default();
        ctx.keybindings.undo_revert = None;
        ctx.keybindings.redo_revert = ctx.keybindings.find;
        let json = serde_json::to_string(&ctx).unwrap();
        let restored: AppStateCtx = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.keybindings.undo_revert, None);
        assert_eq!(restored.keybindings.redo_revert, ctx.keybindings.find);
    }

    mod quick_diff {
        use super::*;

        const TEMP: &str = r"C:\Users\me\AppData\Local\Temp";
        const TEMP_COPY: &str = r"C:\Users\me\AppData\Local\Temp\p4v\main.rs#3";
        const WORKSPACE: &str = r"C:\ws";
        const WORKSPACE_FILE: &str = r"C:\ws\src\main.rs";

        /// A slot that diffs the open file's root-relative path under a depot root.
        fn depot_slot() -> QuickDiffPaths {
            QuickDiffPaths {
                target: ("//depot/main".into(), String::new()),
                source: None,
            }
        }

        fn side(root: Option<&str>, path: &str) -> FileProcessor {
            let mut file = FileProcessor::new();
            if let Some(root) = root {
                file.set_root(UniversalPath::from(root));
            }
            file.set_path(UniversalPath::from(path));
            file
        }

        fn apply(
            slot: &QuickDiffPaths,
            file_1: &mut FileProcessor,
            file_2: &mut FileProcessor,
        ) -> Result<(), QuickDiffRefusal> {
            apply_quick_diff(slot, file_1, file_2, Path::new(TEMP))
        }

        #[test]
        fn neither_temp_takes_the_left_sides_path() {
            let mut left = side(Some(WORKSPACE), WORKSPACE_FILE);
            let mut right = side(None, r"C:\other\lib.rs");
            assert_eq!(apply(&depot_slot(), &mut left, &mut right), Ok(()));
            assert_eq!(
                right.get_full_path(),
                UniversalPath::new("//depot/main/src/main.rs")
            );
            assert_eq!(left.get_full_path(), UniversalPath::new(WORKSPACE_FILE));
        }

        #[test]
        fn a_temp_left_side_takes_the_right_sides_path() {
            let mut left = side(None, TEMP_COPY);
            let mut right = side(Some(WORKSPACE), WORKSPACE_FILE);
            assert_eq!(apply(&depot_slot(), &mut left, &mut right), Ok(()));
            assert_eq!(
                right.get_full_path(),
                UniversalPath::new("//depot/main/src/main.rs")
            );
            assert_eq!(left.get_full_path(), UniversalPath::new(TEMP_COPY));
        }

        #[test]
        fn a_temp_right_side_takes_the_left_sides_path() {
            let mut left = side(Some(WORKSPACE), WORKSPACE_FILE);
            let mut right = side(None, TEMP_COPY);
            assert_eq!(apply(&depot_slot(), &mut left, &mut right), Ok(()));
            assert_eq!(
                right.get_full_path(),
                UniversalPath::new("//depot/main/src/main.rs")
            );
            assert_eq!(left.get_full_path(), UniversalPath::new(WORKSPACE_FILE));
        }

        #[test]
        fn both_temp_disables_every_slot_and_changes_nothing() {
            let other_copy = r"C:\Users\me\AppData\Local\Temp\p4v\main.rs#4";
            let slots = [
                depot_slot(),
                QuickDiffPaths {
                    target: ("//depot/main".into(), "src/main.rs".into()),
                    source: Some((WORKSPACE.into(), WORKSPACE_FILE.into())),
                },
            ];
            for slot in slots {
                let mut left = side(None, TEMP_COPY);
                let mut right = side(None, other_copy);
                assert_eq!(
                    apply(&slot, &mut left, &mut right),
                    Err(QuickDiffRefusal::BothTemp),
                    "{slot:?}"
                );
                assert_eq!(left.get_full_path(), UniversalPath::new(TEMP_COPY));
                assert_eq!(left.get_root(), None);
                assert_eq!(right.get_full_path(), UniversalPath::new(other_copy));
                assert_eq!(right.get_root(), None);
            }
        }

        #[test]
        fn a_slot_source_is_applied_before_the_path_is_taken() {
            // The source replaces the temp left side, so it supplies the path.
            let slot = QuickDiffPaths {
                source: Some((WORKSPACE.into(), r"C:\ws\src\lib.rs".into())),
                ..depot_slot()
            };
            let mut left = side(None, TEMP_COPY);
            let mut right = side(None, r"C:\other\main.rs");
            assert_eq!(apply(&slot, &mut left, &mut right), Ok(()));
            assert_eq!(
                left.get_full_path(),
                UniversalPath::new(r"C:\ws\src\lib.rs")
            );
            assert_eq!(
                right.get_full_path(),
                UniversalPath::new("//depot/main/src/lib.rs")
            );
        }
    }
}
