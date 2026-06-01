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
    cached_file::CachedFile,
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
    file::FileProcessor,
    keybindings::{Keybindings, Shortcut, ui_keybindings},
    p4::open_revision_graph,
    quick_diff::{UniversalPathConfig, ui_universal_path},
    ui_egui::{
        diff_pane::{FileDiffPane, FileDiffPaneCtx},
        panes::{Pane, TreeBehavior},
    },
};

#[derive(Debug, Default)]
pub struct DiffCtx {
    pub file_1_hash: String,
    pub file_2_hash: String,
    #[cfg(debug_assertions)]
    pub debug_file_1_path: UniversalPath,
    #[cfg(debug_assertions)]
    pub debug_file_2_path: UniversalPath,

    pub one_sided_diff_is_left: Option<bool>,
    pub diff_option: DiffBuilderOptions,
    pub precomputed_diffs: Vec<(usize, usize)>, // list indicies of diff_rows of DiffOp != Equal from diff_rows
    pub precomputed_file_rows: (Vec<usize>, Vec<usize>), // line mapping from DiffRow index to DiffRow line number
    // Myers
    pub diff_rows: Vec<DiffRow>,
    pub num_add_deletes: (u32, u32),

    pub update_diff_rows_input: UpdateDiffRowsInput,
}

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AppStateCtx {
    pub file_1: FileProcessor,
    pub file_2: FileProcessor,
    pub diffpane_file_1_buffer: String,
    pub diffpane_file_2_buffer: String,

    pub diff_lexer_mode: u8,
    pub diff_options: DiffBuilderOptions,

    #[cfg_attr(feature = "serde", serde(skip))]
    pub myers_diff_algorithm: MyersDiffAlgorithm,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub diff_ctx: Option<DiffCtx>,
    #[cfg_attr(feature = "serde", serde(skip))]
    diff_ctx_rx: Option<Receiver<DiffCtx>>,

    #[cfg_attr(feature = "serde", serde(skip))]
    pub diff_ctx_in_progress_input: Option<UpdateDiffRowsInput>,

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
    pub find_cursor: ClampedCursor,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub find_found_lines_1: Vec<usize>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub find_found_lines_2: Vec<usize>,

    #[cfg_attr(feature = "serde", serde(skip))]
    pub diff_ctx_conflict_cursor: ClampedCursor,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub diff_ctx_active_highlights: Vec<usize>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub diff_ctx_pivot: (Option<usize>, Option<usize>),

    // ### Keybindings
    pub keybindings: Keybindings,
    // ### Universal Path
    pub universal_path_config: UniversalPathConfig,
}

impl Default for AppStateCtx {
    fn default() -> Self {
        Self {
            file_1: Default::default(),
            file_2: Default::default(),
            diffpane_file_1_buffer: Default::default(),
            diffpane_file_2_buffer: Default::default(),
            diff_options: Default::default(),
            diff_ctx: Default::default(),
            diff_ctx_rx: Default::default(),
            scroll_left: Default::default(),
            scroll_right: Default::default(),
            scroll_to_rows: Default::default(),
            goto_open: Default::default(),
            find_open: Default::default(),
            goto_input: Default::default(),
            find_input: Default::default(),
            diff_ctx_conflict_cursor: Default::default(),
            diff_ctx_active_highlights: Default::default(),
            diff_lexer_mode: LEXER_MODE_DEFAULT,
            find_cursor: Default::default(),
            find_found_lines_1: Default::default(),
            find_found_lines_2: Default::default(),
            diff_ctx_pivot: Default::default(),
            keybindings: Default::default(),
            universal_path_config: Default::default(),
            myers_diff_algorithm: Default::default(),
            diff_ctx_in_progress_input: Default::default(),
        }
    }
}

#[derive(Debug, Default, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UpdateDiffRowsInput {
    #[cfg_attr(feature = "serde", serde(skip))]
    pub file_1: Option<Arc<CachedFile<RawToken>>>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub file_2: Option<Arc<CachedFile<RawToken>>>,
    pub options: DiffBuilderOptions,
    pub myers_diff_algorithm: MyersDiffAlgorithm,
}
impl PartialEq for UpdateDiffRowsInput {
    fn eq(&self, other: &Self) -> bool {
        self.file_1.as_ref().map_or(None, |f| Some(f.hash.clone()))
            == other.file_1.as_ref().map_or(None, |f| Some(f.hash.clone()))
            && self.file_2.as_ref().map_or(None, |f| Some(f.hash.clone()))
                == other.file_2.as_ref().map_or(None, |f| Some(f.hash.clone()))
            && self.options == other.options
            && self.myers_diff_algorithm == other.myers_diff_algorithm
            && self.file_1.as_ref().map_or(None, |f| Some(f.lexer_mode))
                == other.file_1.as_ref().map_or(None, |f| Some(f.lexer_mode))
            && self.file_2.as_ref().map_or(None, |f| Some(f.lexer_mode))
                == other.file_2.as_ref().map_or(None, |f| Some(f.lexer_mode))
    }
}

impl AppStateCtx {
    fn precompute_diff_spans(diff_rows: &[DiffRow]) -> Vec<(usize, usize)> {
        let has_change = |content: &LineContent| match content {
            LineContent::Code { tokens, .. } => tokens
                .iter()
                .any(|(res, _)| !res.hide_in_diff && !matches!(res.operation, DiffOp::Equal(_))),
            _ => false,
        };

        let diff_indices: Vec<usize> = diff_rows
            .iter()
            .enumerate()
            .filter_map(|(idx, row)| {
                if has_change(&row.left) || has_change(&row.right) {
                    Some(idx)
                } else {
                    None
                }
            })
            .collect();

        diff_indices
            .chunk_by(|&a, &b| b == a + 1)
            .map(|chunk| (*chunk.first().unwrap(), *chunk.last().unwrap()))
            .collect()
    }

    fn precompute_file_rows(
        diff_rows: &[DiffRow],
        file_1_line_count: usize,
        file_2_line_count: usize,
    ) -> (Vec<usize>, Vec<usize>) {
        let mut file_1_to_diff = vec![usize::MAX; file_1_line_count];
        let mut file_2_to_diff = vec![usize::MAX; file_2_line_count];

        for (row_idx, row) in diff_rows.iter().enumerate() {
            if let LineContent::Code { line_num, .. } = row.left {
                if line_num > 0 {
                    let idx = line_num as usize - 1;
                    if idx < file_1_line_count && file_1_to_diff[idx] == usize::MAX {
                        file_1_to_diff[idx] = row_idx;
                    }
                }
            }

            if let LineContent::Code { line_num, .. } = row.right {
                if line_num > 0 {
                    let idx = line_num as usize - 1;
                    if idx < file_2_line_count && file_2_to_diff[idx] == usize::MAX {
                        file_2_to_diff[idx] = row_idx;
                    }
                }
            }
        }

        for val in file_1_to_diff.iter_mut() {
            if *val == usize::MAX {
                *val = 0;
            }
        }
        for val in file_2_to_diff.iter_mut() {
            if *val == usize::MAX {
                *val = 0;
            }
        }

        (file_1_to_diff, file_2_to_diff)
    }

    fn apply_pivot(
        diff_rows: &mut Vec<DiffRow>,
        pivot_lines: (usize, usize),
        precomputed_file_rows: &(Vec<usize>, Vec<usize>),
    ) {
        log::debug!("pivot: {:?}", pivot_lines);
        let found_diff_row_pivot_index_1 =
            precomputed_file_rows.0.get(pivot_lines.0.saturating_sub(1));
        let found_diff_row_pivot_index_2 =
            precomputed_file_rows.1.get(pivot_lines.1.saturating_sub(1));
        log::debug!(
            "found_diff_row_pivot_index_1: {:?}",
            found_diff_row_pivot_index_1
        );
        log::debug!(
            "found_diff_row_pivot_index_2: {:?}",
            found_diff_row_pivot_index_2
        );
        if let (Some(pivot_1), Some(pivot_2)) =
            (found_diff_row_pivot_index_1, found_diff_row_pivot_index_2)
        {
            // +: pad right side
            // -: pad left side
            let diff = if pivot_1 > pivot_2 {
                (pivot_1 - pivot_2) as isize
            } else {
                -((pivot_2 - pivot_1) as isize)
            };
            let offset = diff.abs() as usize;
            log::debug!("pivot diff: {}", diff);

            let dummy_diff_row = DiffRow {
                left: LineContent::Void,
                right: LineContent::Void,
            };
            diff_rows.splice(0..0, std::iter::repeat(dummy_diff_row).take(offset));

            if diff > 0 {
                // Move LEFT side "up" (Left pivot was further down)
                for i in 0..diff_rows.len() {
                    if i + offset < diff_rows.len() {
                        // Take the 'left' from a later row and bring it here
                        diff_rows[i].left = diff_rows[i + offset].left.clone();
                    } else {
                        // No more data to pull from, fill with Void
                        diff_rows[i].left = LineContent::Void;
                    }
                }
            } else if diff < 0 {
                // Move RIGHT side "up" (Right pivot was further down)
                for i in 0..diff_rows.len() {
                    if i + offset < diff_rows.len() {
                        diff_rows[i].right = diff_rows[i + offset].right.clone();
                    } else {
                        diff_rows[i].right = LineContent::Void;
                    }
                }
            }
        }
    }

    fn update_diff_rows(
        input: UpdateDiffRowsInput,
        cancel_flag: Arc<AtomicBool>,
    ) -> Option<DiffCtx> {
        let UpdateDiffRowsInput {
            file_1,
            file_2,
            options,
            myers_diff_algorithm,
        } = input;

        #[cfg(feature = "debug_alloc")]
        let mut reg = stats_alloc::Region::new(&crate::STATS_ALLOC);
        #[cfg(feature = "debug_alloc")]
        log::log!("Allocations update_diff_rows: {:?}", reg.change_and_reset());

        let file_1_clone = file_1.clone();
        let file_2_clone = file_2.clone();

        let (c1, c2, one_sided_diff_is_left) = match (&file_1_clone, &file_2_clone) {
            (Some(c1), Some(c2)) => (c1, c2, None),
            (Some(c1), None) => (c1, c1, Some(true)),
            (None, Some(c2)) => (c2, c2, Some(false)),
            (None, None) => panic!("Only call this function with one of two files valid"),
        };

        let t1 = &c1.tokens;
        let t2 = &c2.tokens;
        let cmp = |a: &RawToken, b: &RawToken| {
            if a.as_ref().kind != b.as_ref().kind {
                return false;
            }

            let a_span = &a.span;
            let b_span = &b.span;

            let a_len = a_span.end - a_span.start;
            let b_len = b_span.end - b_span.start;

            if a_len != b_len {
                return false;
            }

            let a_bytes = &c1.contents.as_bytes()[a_span.start..a_span.end];
            let b_bytes = &c2.contents.as_bytes()[b_span.start..b_span.end];

            a_bytes == b_bytes
        };

        #[cfg(feature = "debug_alloc")]
        log::log!(
            "Allocations before myers_diff: {:?}",
            reg.change_and_reset()
        );
        let myers_path = myers_diff_path(myers_diff_algorithm, t1, t2, cmp, cancel_flag.clone())?;
        #[cfg(feature = "debug_alloc")]
        log::log!("Allocations myers_diff: {:?}", reg.change_and_reset());

        if cancel_flag.load(Ordering::Relaxed) {
            log::debug!("cancel_flag: myers_diff_path");
            return None;
        }

        let is_equal_left = one_sided_diff_is_left.unwrap_or(true);
        let diff_ir = DiffIR::new(&myers_path, is_equal_left, cancel_flag.clone())?;

        #[cfg(feature = "debug_alloc")]
        log::log!("Allocations DiffIR::new(): {:?}", reg.change_and_reset());

        if cancel_flag.load(Ordering::Relaxed) {
            log::debug!("cancel_flag: DiffIR::new");
            return None;
        }

        let hash1 = hash_contents(&c1.contents.as_bytes());
        let hash2 = hash_contents(&c2.contents.as_bytes());

        #[cfg(feature = "debug_alloc")]
        log::log!("Allocations hash_file: {:?}", reg.change_and_reset());
        let mut diff_rows: Vec<DiffRow> = build_diff_rows(
            diff_ir,
            Some(&t1),
            Some(&t2),
            &options,
            c1.metadata.num_lines().max(c2.metadata.num_lines()),
        );
        #[cfg(feature = "debug_alloc")]
        log::log!("Allocations build_diff_rows: {:?}", reg.change_and_reset());

        if cancel_flag.load(Ordering::Relaxed) {
            log::debug!("cancel_flag: build_diff_rows");
            return None;
        }

        let line_count_1 = c1.metadata.line_starts.len();
        let line_count_2 = c2.metadata.line_starts.len();

        if let Some(pivot_lines) = options.pivot_lines {
            if pivot_lines.0 > 0 && pivot_lines.1 > 0 {
                let precomputed_file_rows =
                    Self::precompute_file_rows(&diff_rows, line_count_1, line_count_2);
                Self::apply_pivot(&mut diff_rows, pivot_lines, &precomputed_file_rows);
            }
        }

        if cancel_flag.load(Ordering::Relaxed) {
            log::debug!("cancel_flag: precompute_diff_spans");
            return None;
        }

        let precomputed_diffs = Self::precompute_diff_spans(&diff_rows);
        let precomputed_file_rows =
            Self::precompute_file_rows(&diff_rows, line_count_1, line_count_2);

        if cancel_flag.load(Ordering::Relaxed) {
            log::debug!("cancel_flag: precompute_file_rows");
            return None;
        }

        let input = UpdateDiffRowsInput {
            file_1,
            file_2,
            options: options.clone(),
            myers_diff_algorithm,
        };
        Some(DiffCtx {
            file_1_hash: hash1,
            file_2_hash: hash2,
            diff_option: options.clone(),
            diff_rows,
            num_add_deletes: myers_count_add_deletes(&myers_path),
            one_sided_diff_is_left,
            precomputed_diffs,
            precomputed_file_rows,
            #[cfg(debug_assertions)]
            debug_file_1_path: c1.path.clone(),
            #[cfg(debug_assertions)]
            debug_file_2_path: c2.path.clone(),
            update_diff_rows_input: input,
        })
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

    fn refresh_file_contents(
        file_1: &mut FileProcessor,
        file_2: &mut FileProcessor,
        diff_ctx: &mut Option<DiffCtx>,
    ) {
        file_1.invalidate_cache_file();
        file_2.invalidate_cache_file();
        *diff_ctx = None;
    }

    fn refresh_diff_rows(diff_ctx: &mut Option<DiffCtx>) {
        *diff_ctx = None;
    }

    fn show_menu(
        &mut self,
        ui: &mut egui::Ui,
        file_1: &mut FileProcessor,
        file_2: &mut FileProcessor,
        diff_ctx: &mut Option<DiffCtx>,
        find_open: &mut bool,
        goto_open: &mut bool,
        scroll_left: &mut f32,
        scroll_right: &mut f32,
        lexer_mode: &mut u8,
        keybindings: &mut Keybindings,
        universal_path_config: &mut UniversalPathConfig,
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

                        *diff_ctx = None;
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
                            "[{}]Universal Path",
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
                        *diff_ctx = None;
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
                        Self::refresh_file_contents(file_1, file_2, diff_ctx);
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
                        Self::refresh_diff_rows(diff_ctx);
                    }
                    #[cfg(debug_assertions)]
                    {
                        let load_btn = |ui: &mut egui::Ui,
                                        label: &str,
                                        file_1: &mut FileProcessor,
                                        file_2: &mut FileProcessor,
                                        diff_ctx: &mut Option<_>,
                                        p1: &str,
                                        p2: &str| {
                            if ui.button(label).clicked() {
                                let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));

                                file_1.set_root("".into());
                                file_2.set_root("".into());
                                file_1.set_path(UniversalPath::from(base.join(p1)));
                                file_2.set_path(UniversalPath::from(base.join(p2)));

                                *diff_ctx = None;
                            }
                        };

                        load_btn(
                            ui,
                            "Load $A",
                            file_1,
                            file_2,
                            diff_ctx,
                            "../../test/rust_files_diff_1/advanced_rust.rs",
                            "../../test/rust_files_diff_1/advanced_rust_2.rs",
                        );

                        load_btn(
                            ui,
                            "Load $B",
                            file_1,
                            file_2,
                            diff_ctx,
                            "../../test/rust_files_diff_1/imgui.1.91.1.h",
                            "../../test/rust_files_diff_1/imgui.h",
                        );

                        load_btn(
                            ui,
                            "Load $C",
                            file_1,
                            file_2,
                            diff_ctx,
                            "../../test/test_ignore_whitespace_simple/1.txt",
                            "../../test/test_ignore_whitespace_simple/2.txt",
                        );

                        load_btn(
                            ui,
                            "Load $D",
                            file_1,
                            file_2,
                            diff_ctx,
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
                "Option - Universal Path",
                true,
                |ui| {
                    ui_universal_path(ui, universal_path_config);
                },
            );
        }
    }

    fn ui(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame, app_ctx: &mut AppStateCtx) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let AppStateCtx {
                scroll_left,
                scroll_right,
                diff_ctx,
                diff_options,
                file_1,
                file_2,
                scroll_to_rows,
                goto_open,
                find_open,
                goto_input,
                find_input,
                diff_ctx_conflict_cursor,
                diff_ctx_active_highlights,
                diff_ctx_rx: _,
                diff_lexer_mode: lexer_mode,
                find_cursor,
                find_found_lines_1,
                find_found_lines_2,
                diff_ctx_pivot,
                keybindings,
                universal_path_config,
                myers_diff_algorithm,
                diff_ctx_in_progress_input: _,
                diffpane_file_1_buffer: _,
                diffpane_file_2_buffer: _,
            } = app_ctx;
            self.show_menu(
                ui,
                file_1,
                file_2,
                diff_ctx,
                find_open,
                goto_open,
                scroll_left,
                scroll_right,
                lexer_mode,
                keybindings,
                universal_path_config,
                myers_diff_algorithm,
            );

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
                    if let Some(diff) = diff_ctx {
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
                        find_cursor.set_max(all_found_lines.len().saturating_sub(1));
                        find_cursor.set(0);
                        find_cursor.invalidate_ack();
                    }

                    find_input.clear();
                    *find_open = false;
                }
            });
            if !find_window_open {
                *find_open = find_window_open;
            }

            let diff_ctx_ref = diff_ctx.as_ref();

            ui.separator();

            if let Some((start, maybe_end)) = &mut scroll_to_rows.as_ref() {
                diff_ctx_active_highlights.clear();
                if let Some(end) = maybe_end {
                    diff_ctx_active_highlights.extend(*start..=*end);
                } else {
                    diff_ctx_active_highlights.push(*start);
                }
            }

            let mut behavior = TreeBehavior {
                ctx_file_diff: FileDiffPaneCtx {
                    diff_ctx: diff_ctx_ref,
                    scroll_left: scroll_left,
                    scroll_right: scroll_right,
                    diff_options: diff_options,
                    file_source: file_1.get_cached_file().clone(),
                    file_target: file_2.get_cached_file().clone(),
                    scroll_to_row_span: &scroll_to_rows,
                    active_highlights: &diff_ctx_active_highlights,
                    conflict_cursor: diff_ctx_conflict_cursor,
                    load_file_1_request: &mut None,
                    load_file_2_request: &mut None,
                    set_file_1_root_request: &mut None,
                    set_file_2_root_request: &mut None,
                    find_cursor,
                    pivot: diff_ctx_pivot,
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

            *scroll_to_rows = None;

            for (_tile_id, tile) in self.tree.tiles.iter() {
                if let Tile::Pane(Pane::FileDiff(..)) = tile {
                    let source = app_ctx.file_1.get_path_as_string();
                    let target = app_ctx.file_2.get_path_as_string();
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
                    app_state_ctx.diff_ctx_conflict_cursor.dec();
                    log::info!(
                        "ConflictCursor-- @{}",
                        app_state_ctx.diff_ctx_conflict_cursor.get()
                    );
                }
                if (r.modifiers.ctrl && r.key_pressed(egui::Key::Num2))
                    || (r.modifiers.alt && r.key_pressed(egui::Key::ArrowDown))
                {
                    app_state_ctx.diff_ctx_conflict_cursor.inc();
                    log::info!(
                        "ConflictCursor++ @{}",
                        app_state_ctx.diff_ctx_conflict_cursor.get()
                    );
                }

                if r.modifiers.shift && r.key_pressed(egui::Key::Enter) {
                    if app_state_ctx.find_found_lines_1.len() > 0
                        || app_state_ctx.find_found_lines_2.len() > 0
                    {
                        app_state_ctx.find_cursor.dec();
                        log::info!("FindCursor-- @{}", app_state_ctx.find_cursor.get());
                    }
                } else if r.key_pressed(egui::Key::Enter) {
                    if app_state_ctx.find_found_lines_1.len() > 0
                        || app_state_ctx.find_found_lines_2.len() > 0
                    {
                        app_state_ctx.find_cursor.inc();
                        log::info!("FindCursor++ @{}", app_state_ctx.find_cursor.get());
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
                    Self::refresh_file_contents(
                        &mut app_state_ctx.file_1,
                        &mut app_state_ctx.file_2,
                        &mut app_state_ctx.diff_ctx,
                    );
                });
                handle_kb(
                    &app_state_ctx.keybindings.refresh_diff_rows_only,
                    &mut |_kb| {
                        Self::refresh_diff_rows(&mut app_state_ctx.diff_ctx);
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
                handle_kb(&app_state_ctx.keybindings.revision_graph, &mut |_kb| {
                    let path = &app_state_ctx.file_1.get_path();
                    match path {
                        UniversalPath::Depot(_, _) => {
                            match open_revision_graph(&path.to_p4_string()) {
                                Ok(_) => {
                                    log::info!("Revision graph returned Ok");
                                }
                                Err(e) => log::error!("Failed to open revision graph: {e}"),
                            }
                        }
                        UniversalPath::Local(_path_buf) => {
                            log::info!("Can not open revision graph for local path {}", path);
                            return;
                        }
                    }
                });
                handle_kb(&app_state_ctx.keybindings.timelapse_view, &mut |_kb| {});

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

                        if let Some(source) = &path.source {
                            app_state_ctx.file_1.set_path(UniversalPath::from(source));
                        }

                        app_state_ctx
                            .file_2
                            .set_root(UniversalPath::from(&path.target_root));

                        let target_path = if !path.target.is_empty() {
                            &path.target
                        } else {
                            // Use sources file path split from root
                            &app_state_ctx.file_1.get_path().to_string()
                        };
                        app_state_ctx
                            .file_2
                            .set_path(UniversalPath::from(target_path));

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
                .diff_ctx
                .as_ref()
                .and_then(|f| Some(f.precomputed_diffs.len()))
                .unwrap_or_default();
            app_ctx.diff_ctx_conflict_cursor.set_max(conflict_max);
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

                let diff_ctx_invalidated =
                    if let Some(in_progress_input) = &state.diff_ctx_in_progress_input {
                        *in_progress_input != update_input
                    } else if let Some(diff_ctx) = &state.diff_ctx {
                        let input_equal = update_input == diff_ctx.update_diff_rows_input;

                        if !input_equal && !state.diff_ctx_in_progress_input.is_some() {
                            log::debug!("diff_ctx invalidated!");
                        }
                        !input_equal
                    } else {
                        !state.diff_ctx_in_progress_input.is_some()
                    };

                if diff_ctx_invalidated && state.diff_ctx_in_progress_input.is_some() {
                    log::info!("diff_ctx invalidated && diff_ctx_in_progress");
                    self.update_diff_rows_cancel_flag
                        .as_deref()
                        .unwrap()
                        .store(true, Ordering::Release);
                    log::info!("thread cancel_flag set true");
                    state.diff_ctx_in_progress_input = None;
                } else if diff_ctx_invalidated && state.diff_ctx_in_progress_input.is_none() {
                    let f1 = state.file_1.get_cached_file().clone();
                    let f2 = state.file_2.get_cached_file().clone();

                    state.diff_ctx = None;

                    if f1.is_some() || f2.is_some() {
                        let (tx, rx) = channel();
                        state.diff_ctx_rx = Some(rx);

                        let cancel_flag = Arc::new(AtomicBool::new(false));
                        self.update_diff_rows_cancel_flag = Some(cancel_flag.clone());
                        let input = UpdateDiffRowsInput {
                            file_1: f1.clone(),
                            file_2: f2.clone(),
                            options: state.diff_options.clone(),
                            myers_diff_algorithm: state.myers_diff_algorithm.clone(),
                        };
                        state.diff_ctx_in_progress_input = Some(input.clone());

                        let builder = std::thread::Builder::new().name("DiffCtxTHREAD".into());
                        let handle = builder.spawn(move || {
                            log::info!(
                                "Spawned thread for DiffCtx\nSource: {}, Target: {}",
                                f1.as_ref()
                                    .and_then(|f| Some(format!("{}", f.path)))
                                    .unwrap_or_default(),
                                f2.as_ref()
                                    .and_then(|f| Some(format!("{}", f.path)))
                                    .unwrap_or_default(),
                            );
                            let result = AppStateCtx::update_diff_rows(input, cancel_flag);
                            log::info!(
                                "Spawned thread for DiffCtx complete {:?}",
                                result.is_some()
                            );
                            if let Some(result) = result {
                                let _ = tx.send(result);
                            }
                        });
                        match handle {
                            Ok(h) => {
                                self.update_diff_rows_thread_handle = Some(h);
                            }
                            Err(e) => {
                                log::error!("Failed to spawn thread: {e}");
                                state.diff_ctx_in_progress_input = None;
                                state.diff_ctx_rx = None;
                            }
                        }
                    }
                }

                if state.diff_ctx_in_progress_input.is_some() {
                    if let Some(rx) = &state.diff_ctx_rx {
                        match rx.try_recv() {
                            Ok(r) => {
                                log::info!("Recieved new diff_ctx from thread");
                                self.update_diff_rows_thread_handle = None;
                                state.diff_ctx_in_progress_input = None;
                                state.diff_ctx = Some(r);
                                ctx.request_repaint();
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                log::error!("Channel error: Disconnected");
                                self.update_diff_rows_thread_handle = None;
                                state.diff_ctx_in_progress_input = None;
                            }
                        }
                    }
                }

                if state.diff_ctx_conflict_cursor.has_changed() {
                    state.diff_ctx_conflict_cursor.ack_change();
                    if let Some(diff_ctx) = state.diff_ctx.as_ref() {
                        if state.diff_ctx_conflict_cursor.get() > 0 {
                            let conflict_idx_span = diff_ctx.precomputed_diffs
                                [state.diff_ctx_conflict_cursor.get().saturating_sub(1)];
                            state.scroll_to_rows =
                                Some((conflict_idx_span.0, Some(conflict_idx_span.1)));
                        } else {
                            state.scroll_to_rows = None;
                            state.diff_ctx_active_highlights.clear();
                        }
                    }
                }

                // FIND
                if state.find_cursor.has_changed() {
                    state.find_cursor.ack_change();

                    let mut all_found_lines = state.find_found_lines_1.clone();
                    all_found_lines.extend(state.find_found_lines_2.clone());
                    all_found_lines.dedup();
                    all_found_lines.sort();
                    assert_eq!(
                        state.find_cursor.get_max(),
                        all_found_lines.len().saturating_sub(1)
                    );

                    let find_idx_1 = all_found_lines.get(state.find_cursor.get()).cloned();

                    // TODO: Improve so that user can decide which 1/2 file search operates on
                    state.scroll_to_rows = Some((find_idx_1.unwrap_or_default(), None));
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
