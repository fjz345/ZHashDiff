use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self},
};

#[cfg(debug_assertions)]
use zdiff::universal_path::UniversalPath;
use zdiff::{
    cached_file::CachedFile,
    diff_builder::{DiffBuilderOptions, DiffRow, LineContent, PivotLines, build_diff_rows},
    diff_ir::{DiffIR, DiffOp},
    lexer::RawToken,
    myers::{
        MyersDiffAlgorithm, MyersNumAddDelete, MyersPath, myers_count_add_deletes, myers_diff_path,
    },
};

use crate::clamped_cursor::ClampedCursor;

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
        self.file_1.as_ref().map(|f| &f.hash) == other.file_1.as_ref().map(|f| &f.hash)
            && self.file_2.as_ref().map(|f| &f.hash) == other.file_2.as_ref().map(|f| &f.hash)
            && self.options == other.options
            && self.myers_diff_algorithm == other.myers_diff_algorithm
            && self.file_1.as_ref().map(|f| &f.lexer_mode)
                == other.file_1.as_ref().map(|f| &f.lexer_mode)
            && self.file_2.as_ref().map(|f| &f.lexer_mode)
                == other.file_2.as_ref().map(|f| &f.lexer_mode)
    }
}

#[derive(Debug, Clone, Default)]
pub struct FindCtx {
    #[allow(unused)] // most likely will use this somehow?
    found_lines_1: Vec<usize>,
    #[allow(unused)] // most likely will use this somehow?
    found_lines_2: Vec<usize>,
    cached_found_lines: Vec<usize>,
}
impl FindCtx {
    pub fn new(find_input: &str, diff_ctx: &MinimalDiffCtx) -> Self {
        Self::build(find_input, diff_ctx)
    }

    fn combine_found_lines(found_lines_1: &Vec<usize>, found_lines_2: &Vec<usize>) -> Vec<usize> {
        let mut all_found_lines = found_lines_1.clone();
        all_found_lines.extend(found_lines_2.clone());
        all_found_lines.sort_unstable();
        all_found_lines.dedup();
        all_found_lines
    }

    fn build(find_input: &str, diff_ctx: &MinimalDiffCtx) -> Self {
        let mut find_found_lines_1: Vec<usize> = Vec::new();
        let mut find_found_lines_2: Vec<usize> = Vec::new();

        if let Some(file) = &diff_ctx.input.file_1 {
            find_found_lines_1 = file
                .content_search(&find_input)
                .into_iter()
                .map(|f| diff_ctx.precomputed_file_rows.0[f])
                .collect()
        }

        if let Some(file) = &diff_ctx.input.file_2 {
            find_found_lines_2 = file
                .content_search(&find_input)
                .into_iter()
                .map(|f| diff_ctx.precomputed_file_rows.1[f])
                .collect()
        }
        log::debug!("Found (in #1): {:?}", find_found_lines_1);
        log::debug!("Found (in #2): {:?}", find_found_lines_2);

        let cached_found_lines =
            Self::combine_found_lines(&find_found_lines_1, &find_found_lines_2);
        let find_ctx = Self {
            found_lines_1: find_found_lines_1,
            found_lines_2: find_found_lines_2,
            cached_found_lines,
        };
        log::debug!("create_find_ctx: {:?}", find_ctx);
        find_ctx
    }
}

#[derive(Debug)]
pub struct DiffSpan {
    start: usize,
    end: usize,
}
pub type PrecomputedDiffs = Vec<DiffSpan>; // list spans with indicies of diff_rows of DiffOp != Equal from diff_rows
pub type PrecomputedFileRows = (Vec<usize>, Vec<usize>); // line mapping from DiffRow index to DiffRow line number
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct ScrollSpan {
    pub start: usize,
    pub maybe_end: Option<usize>,
}
pub type DiffRows = Vec<DiffRow>; // Span with optional end

#[derive(Debug, Clone, PartialEq)]
pub struct MyersCtxInput {
    pub file_1: Option<Arc<CachedFile<RawToken>>>,
    pub file_2: Option<Arc<CachedFile<RawToken>>>,
    pub algo: MyersDiffAlgorithm,
}
impl From<UpdateDiffRowsInput> for MyersCtxInput {
    fn from(input: UpdateDiffRowsInput) -> Self {
        Self {
            file_1: input.file_1,
            file_2: input.file_2,
            algo: input.myers_diff_algorithm,
        }
    }
}

impl From<&UpdateDiffRowsInput> for MyersCtxInput {
    fn from(input: &UpdateDiffRowsInput) -> Self {
        Self {
            file_1: input.file_1.clone(),
            file_2: input.file_2.clone(),
            algo: input.myers_diff_algorithm,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct DiffIRInput {
    pub myers_path: MyersPath,
}
#[derive(Debug, Clone, PartialEq)]
pub struct DiffRowsInput {
    pub file_1: Option<Arc<CachedFile<RawToken>>>,
    pub file_2: Option<Arc<CachedFile<RawToken>>>,
    pub diff_ir: DiffIR,
    pub diff_options: DiffBuilderOptions,
}
#[derive(Debug)]
pub struct MyersCtx {
    input: MyersCtxInput,

    num_add_delete: MyersNumAddDelete,
    path: MyersPath,
}
#[derive(Debug)]
pub struct DiffIRCtx {
    input: DiffIRInput,
    diff_ir: DiffIR,
}
#[derive(Debug)]
pub struct DiffRowsCtx {
    input: DiffRowsInput,
    rows: Arc<DiffRows>,
    precomputed_diffs: Arc<PrecomputedDiffs>,
    precomputed_file_rows: Arc<PrecomputedFileRows>,
}

#[derive(Debug)]
pub struct MinimalDiffCtx {
    #[cfg(debug_assertions)]
    pub debug_file_1_path: UniversalPath,
    #[cfg(debug_assertions)]
    pub debug_file_2_path: UniversalPath,

    pub input: UpdateDiffRowsInput,

    pub num_add_deletes: MyersNumAddDelete,
    pub precomputed_diffs: Arc<PrecomputedDiffs>,
    pub precomputed_file_rows: Arc<PrecomputedFileRows>,
    pub diff_rows: Arc<DiffRows>,
}

macro_rules! check_cancel {
    ($flag:expr, $step:expr) => {
        if $flag.load(Ordering::Relaxed) {
            log::debug!("cancel_flag: {}", $step);
            return None;
        }
    };
}

#[cfg(feature = "debug_alloc")]
macro_rules! track_alloc {
    ($reg:expr, $step:expr) => {
        log::log!("Allocations {}: {:?}", $step, $reg.change_and_reset());
    };
}

#[cfg(not(feature = "debug_alloc"))]
macro_rules! track_alloc {
    ($reg:expr, $step:expr) => {};
}

macro_rules! poll_ctx_channel {
    ($channel:expr, $inflight:expr, $ctx:expr, $transform:expr) => {
        while let Ok(result) = $channel.try_recv() {
            match result {
                Some(res) => {
                    if let Some(pending) = &$inflight {
                        if *pending == res.input {
                            $ctx = $transform(Some(res));
                            $inflight = None;
                        }
                    }
                }
                None => {
                    $inflight = None;
                }
            }
        }
    };
    ($channel:expr, $inflight:expr, $ctx:expr) => {
        poll_ctx_channel!($channel, $inflight, $ctx, |x| x)
    };
}

#[derive(Debug)]
pub struct DiffCtx {
    pub update_diff_rows_input: UpdateDiffRowsInput,

    #[cfg(debug_assertions)]
    pub debug_file_1_path: UniversalPath,
    #[cfg(debug_assertions)]
    pub debug_file_2_path: UniversalPath,

    channel_myers: (
        mpsc::Sender<Option<MyersCtx>>,
        mpsc::Receiver<Option<MyersCtx>>,
    ),
    channel_diff_ir: (
        mpsc::Sender<Option<DiffIRCtx>>,
        mpsc::Receiver<Option<DiffIRCtx>>,
    ),
    channel_diff_rows: (
        mpsc::Sender<Option<DiffRowsCtx>>,
        mpsc::Receiver<Option<DiffRowsCtx>>,
    ),

    myers_inflight_input: Option<MyersCtxInput>,
    myers_ctx: Option<MyersCtx>,

    diff_ir_inflight_input: Option<DiffIRInput>,
    diff_ir_ctx: Option<DiffIRCtx>,

    diff_rows_inflight_input: Option<DiffRowsInput>,
    diff_rows_ctx: Option<Arc<DiffRowsCtx>>,
}
impl Default for DiffCtx {
    fn default() -> Self {
        Self::new(UpdateDiffRowsInput {
            file_1: None,
            file_2: None,
            options: Default::default(),
            myers_diff_algorithm: Default::default(),
        })
    }
}

#[derive(Debug)]
pub enum OneSidedMode {
    TwoSided,
    OnlyLeft,  // No right file
    OnlyRight, // No left file
}

impl DiffCtx {
    #[allow(dead_code)]
    pub fn new(input: UpdateDiffRowsInput) -> Self {
        let (myers_tx, myers_rx) = mpsc::channel();
        let (diff_ir_tx, diff_ir_rx) = mpsc::channel();
        let (diff_rows_tx, diff_rows_rx) = mpsc::channel();

        Self {
            update_diff_rows_input: input.clone(),
            #[cfg(debug_assertions)]
            debug_file_1_path: input
                .file_1
                .as_ref()
                .map(|f| f.path.clone())
                .unwrap_or_default(),
            #[cfg(debug_assertions)]
            debug_file_2_path: input
                .file_2
                .as_ref()
                .map(|f| f.path.clone())
                .unwrap_or_default(),
            channel_myers: (myers_tx, myers_rx),
            channel_diff_ir: (diff_ir_tx, diff_ir_rx),
            channel_diff_rows: (diff_rows_tx, diff_rows_rx),
            myers_inflight_input: None,
            myers_ctx: None,
            diff_ir_inflight_input: None,
            diff_ir_ctx: None,
            diff_rows_inflight_input: None,
            diff_rows_ctx: None,
        }
    }

    pub fn get_one_sided_mode(&self) -> OneSidedMode {
        match (
            self.update_diff_rows_input.file_1.is_some(),
            self.update_diff_rows_input.file_2.is_some(),
        ) {
            (true, true) => OneSidedMode::TwoSided,
            (true, false) => OneSidedMode::OnlyLeft,
            (false, true) => OneSidedMode::OnlyRight,
            (false, false) => panic!("Only call this function with one of two files valid"),
        }
    }

    pub fn set_input(&mut self, input: UpdateDiffRowsInput) {
        log::info!(
            "Diff Ctx recieved new input:\nSource: {:?}\nTarget: {:?}\nOptions: {:?}",
            &input
                .file_1
                .as_ref()
                .map(|f| f.path.to_string())
                .unwrap_or("None".to_string()),
            &input
                .file_2
                .as_ref()
                .map(|f| f.path.to_string())
                .unwrap_or("None".to_string()),
            &input.options
        );
        self.update_diff_rows_input = input;
    }

    pub fn poll(&mut self) {
        while let Ok(myers_res) = self.channel_myers.1.try_recv() {
            match myers_res {
                Some(res) => {
                    if Some(&res.input) == self.myers_inflight_input.as_ref() {
                        self.myers_ctx = Some(res);
                        self.myers_inflight_input = None;
                    }
                }
                None => self.myers_inflight_input = None,
            }
        }

        while let Ok(ir_res) = self.channel_diff_ir.1.try_recv() {
            match ir_res {
                Some(res) => {
                    if Some(&res.input) == self.diff_ir_inflight_input.as_ref() {
                        self.diff_ir_ctx = Some(res);
                        self.diff_ir_inflight_input = None;
                    }
                }
                None => self.diff_ir_inflight_input = None,
            }
        }

        while let Ok(rows_res) = self.channel_diff_rows.1.try_recv() {
            match rows_res {
                Some(res) => {
                    if Some(&res.input) == self.diff_rows_inflight_input.as_ref() {
                        self.diff_rows_ctx = Some(Arc::new(res));
                        self.diff_rows_inflight_input = None;
                    }
                }
                None => self.diff_rows_inflight_input = None,
            }
        }
    }

    pub fn request_myers(&mut self, cancel_flag: Arc<AtomicBool>) -> Option<&MyersCtx> {
        let expected_input: MyersCtxInput = (&self.update_diff_rows_input).into();

        poll_ctx_channel!(
            self.channel_myers.1,
            self.myers_inflight_input,
            self.myers_ctx
        );

        if self
            .myers_ctx
            .as_ref()
            .map_or(false, |ctx| ctx.input == expected_input)
        {
            return self.myers_ctx.as_ref();
        }

        if expected_input.file_1.is_none() && expected_input.file_2.is_none() {
            return None;
        }

        if self.myers_inflight_input.as_ref() != Some(&expected_input) {
            self.myers_inflight_input = Some(expected_input.clone());
            let tx = self.channel_myers.0.clone();
            let input = expected_input;
            let cancel = cancel_flag;

            log::info!("new request_myers");
            std::thread::Builder::new()
                .name("MyersCtxTHREAD".into())
                .spawn(move || {
                    let (c1, c2, _) = resolve_files(&input.file_1, &input.file_2);
                    let cmp = |a: &RawToken, b: &RawToken| compare_tokens(a, b, c1, c2);

                    let cancel_ref = cancel.clone();
                    if let Some(path) =
                        myers_diff_path(input.algo, &c1.tokens, &c2.tokens, cmp, cancel)
                    {
                        let num_add_delete = myers_count_add_deletes(&path);
                        let _ = tx.send(Some(MyersCtx {
                            input,
                            num_add_delete,
                            path,
                        }));
                    } else if !cancel_ref.load(Ordering::Relaxed) {
                        let _ = tx.send(None);
                    }
                })
                .ok();
        }
        None
    }

    pub fn request_diff_ir(&mut self, cancel_flag: Arc<AtomicBool>) -> Option<&DiffIRCtx> {
        let myers_ctx = self.request_myers(cancel_flag.clone())?;
        let expected_input = DiffIRInput {
            myers_path: myers_ctx.path.clone(),
        };

        poll_ctx_channel!(
            self.channel_diff_ir.1,
            self.diff_ir_inflight_input,
            self.diff_ir_ctx
        );

        if self
            .diff_ir_ctx
            .as_ref()
            .map_or(false, |ctx| ctx.input == expected_input)
        {
            return self.diff_ir_ctx.as_ref();
        }

        if self.diff_ir_inflight_input.as_ref() != Some(&expected_input) {
            self.diff_ir_inflight_input = Some(expected_input.clone());
            let tx = self.channel_diff_ir.0.clone();
            let input = expected_input;
            let cancel = cancel_flag;
            let is_equal_left = !matches!(self.get_one_sided_mode(), OneSidedMode::OnlyRight);

            log::info!("new request_diff_ir");
            std::thread::Builder::new()
                .name("DiffIrTHREAD".into())
                .spawn(move || {
                    let cancel_ref = cancel.clone();
                    if let Some(diff_ir) = DiffIR::new(&input.myers_path, is_equal_left, cancel) {
                        let _ = tx.send(Some(DiffIRCtx { input, diff_ir }));
                    } else if !cancel_ref.load(Ordering::Relaxed) {
                        let _ = tx.send(None);
                    }
                })
                .ok();
        }
        None
    }

    pub fn request_diff_rows(&mut self, cancel_flag: Arc<AtomicBool>) -> Option<&DiffRowsCtx> {
        let file_1 = self.update_diff_rows_input.file_1.clone();
        let file_2 = self.update_diff_rows_input.file_2.clone();
        let ir_ctx = self.request_diff_ir(cancel_flag.clone())?;
        let expected_input = DiffRowsInput {
            file_1: file_1,
            file_2: file_2,
            diff_ir: ir_ctx.diff_ir.clone(),
            diff_options: self.update_diff_rows_input.options.clone(),
        };

        poll_ctx_channel!(
            self.channel_diff_rows.1,
            self.diff_rows_inflight_input,
            self.diff_rows_ctx,
            |res: Option<DiffRowsCtx>| res.map(Arc::new)
        );

        if self
            .diff_rows_ctx
            .as_ref()
            .map_or(false, |ctx| ctx.input == expected_input)
        {
            return self.diff_rows_ctx.as_deref();
        }

        if self.diff_rows_inflight_input.as_ref() != Some(&expected_input) {
            self.diff_rows_inflight_input = Some(expected_input.clone());
            let tx = self.channel_diff_rows.0.clone();
            let input = expected_input;
            let cancel = cancel_flag;

            log::info!("new request_diff_rows");
            std::thread::Builder::new()
                .name("DiffRowsTHREAD".into())
                .spawn(move || {
                    let (c1, c2, _) = resolve_files(&input.file_1, &input.file_2);
                    let diff_rows = build_diff_rows(
                        input.diff_ir.clone(),
                        Some(&c1.tokens),
                        Some(&c2.tokens),
                        &input.diff_options,
                        c1.metadata.num_lines().max(c2.metadata.num_lines()),
                    );

                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }

                    if let Some((rows, precomputed_diffs)) = finalize_diff_rows(
                        diff_rows,
                        &input.diff_options,
                        c1.metadata.line_starts.len(),
                        c2.metadata.line_starts.len(),
                        &cancel,
                    ) {
                        let precomputed_file_rows = precompute_file_rows(
                            &rows,
                            c1.metadata.line_starts.len(),
                            c2.metadata.line_starts.len(),
                        );
                        let _ = tx.send(Some(DiffRowsCtx {
                            input,
                            rows: Arc::new(rows),
                            precomputed_diffs: Arc::new(precomputed_diffs),
                            precomputed_file_rows: Arc::new(precomputed_file_rows),
                        }));
                    } else if !cancel.load(Ordering::Relaxed) {
                        let _ = tx.send(None);
                    }
                })
                .ok();
        }
        None
    }

    pub fn request_minimal_diff_ctx(
        &mut self,
        cancel_flag: Arc<AtomicBool>,
    ) -> Option<MinimalDiffCtx> {
        let num_add_deletes = self.request_myers(cancel_flag.clone())?.num_add_delete;
        let diff_row_ctx = self.request_diff_rows(cancel_flag)?;

        let diff_rows = diff_row_ctx.rows.clone();
        let precomputed_diffs = diff_row_ctx.precomputed_diffs.clone();
        let precomputed_file_rows = diff_row_ctx.precomputed_file_rows.clone();

        Some(MinimalDiffCtx {
            #[cfg(debug_assertions)]
            debug_file_1_path: self.debug_file_1_path.clone(),
            #[cfg(debug_assertions)]
            debug_file_2_path: self.debug_file_2_path.clone(),
            input: self.update_diff_rows_input.clone(),
            num_add_deletes,
            precomputed_diffs,
            precomputed_file_rows,
            diff_rows,
        })
    }
}

#[derive(Debug)]
pub struct DiffProcessor {
    ctx: DiffCtx,
    pub in_progress_input: Option<UpdateDiffRowsInput>,
    cancel_flag: Arc<AtomicBool>,

    // Active user state
    pub conflict_cursor: ClampedCursor,
    pub active_highlights: Vec<usize>,
    pub pivot: (Option<usize>, Option<usize>),
    pub find_cursor: ClampedCursor,
    pub find_ctx: FindCtx,
    goto_line_number: Option<usize>,

    last_conflict_scroll_to_row: Option<ScrollSpan>,
    last_goto_scroll_to_row: Option<ScrollSpan>,
    last_find_scroll_to_row: Option<ScrollSpan>,
}

impl Default for DiffProcessor {
    fn default() -> Self {
        Self {
            ctx: Default::default(),
            conflict_cursor: ClampedCursor::default(),
            active_highlights: Vec::new(),
            pivot: (None, None),
            in_progress_input: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            find_cursor: ClampedCursor::default(),
            find_ctx: FindCtx::default(),
            goto_line_number: None,
            last_conflict_scroll_to_row: None,
            last_goto_scroll_to_row: None,
            last_find_scroll_to_row: None,
        }
    }
}

impl DiffProcessor {
    pub fn reset_ctx(&mut self) {
        self.ctx = DiffCtx::default();
        self.reset_ui();
    }
    pub fn reset_ui(&mut self) {
        self.update_find(FindCtx::default());
        self.conflict_cursor.set(0);
        self.goto_line_number = None;
        self.pivot = (None, None);
        self.active_highlights.clear();
        self.last_conflict_scroll_to_row = None;
        self.last_goto_scroll_to_row = None;
        self.last_find_scroll_to_row = None;
    }

    pub fn is_in_progress(&self) -> bool {
        self.in_progress_input.is_some()
    }

    pub fn cancel_in_progress(&mut self) {
        if self.is_in_progress() {
            self.cancel_flag.store(true, Ordering::Release);
            log::info!("Diff Processor sent cancel_flag: true");
        }
    }

    pub fn request_update(&mut self, input: UpdateDiffRowsInput) {
        if input.file_1.is_none() && input.file_2.is_none() {
            log::warn!("request update was called with no file_1 or file_2");
            return;
        }

        self.cancel_in_progress();
        self.cancel_flag = Arc::new(AtomicBool::new(false));
        self.in_progress_input = Some(input.clone());
        log::trace!(
            "Diff Processor new in_progress_input: {:?}",
            &self.in_progress_input
        );
        self.ctx.set_input(input);
    }

    pub fn update(&mut self) {
        let mut reset_ui = false;

        self.ctx.poll();

        if self.is_in_progress()
            && self
                .ctx
                .request_minimal_diff_ctx(self.cancel_flag.clone())
                .is_some()
        {
            self.in_progress_input = None;
            reset_ui = true;
        }

        if reset_ui {
            self.reset_ui();
        }
    }

    pub fn update_goto(&mut self, line_number: Option<usize>) {
        log::info!("Goto to line: {:?}", line_number);
        self.goto_line_number = line_number;
    }

    pub fn update_find(&mut self, find_ctx: FindCtx) {
        self.find_ctx = find_ctx;
        self.find_cursor
            .set_max(self.find_ctx.cached_found_lines.len().saturating_sub(1));
        self.find_cursor.set(0);
    }

    pub fn get_scroll_to_row(&mut self) -> Option<ScrollSpan> {
        let check_update = |new: Option<ScrollSpan>, last: &mut Option<ScrollSpan>| {
            if new != *last {
                *last = new;
                new
            } else {
                None
            }
        };

        let conflict = check_update(
            self.conflict_scroll_to_row(),
            &mut self.last_conflict_scroll_to_row,
        );

        let goto = check_update(
            self.goto_line_number.map(|f| ScrollSpan {
                start: f.saturating_sub(1),
                maybe_end: None,
            }),
            &mut self.last_goto_scroll_to_row,
        );

        let find = check_update(self.find_scroll_to_row(), &mut self.last_find_scroll_to_row);

        let scroll_to_row = find.or(goto).or(conflict);

        if let Some(ScrollSpan { start, maybe_end }) = &scroll_to_row {
            self.active_highlights.clear();
            if let Some(end) = maybe_end {
                self.active_highlights.extend(*start..=*end);
            } else {
                self.active_highlights.push(*start);
            }
        }

        scroll_to_row
    }

    pub fn conflict_scroll_to_row(&mut self) -> Option<ScrollSpan> {
        let cursor_val = self.conflict_cursor.get();
        let mut ret = None;
        if let Some(diff_ctx) = self.get_minimal_diff_ctx() {
            if cursor_val > 0 {
                let conflict_idx_span = &diff_ctx.precomputed_diffs[cursor_val.saturating_sub(1)];
                ret = Some(ScrollSpan {
                    start: conflict_idx_span.start,
                    maybe_end: Some(conflict_idx_span.end),
                });
            } else {
                ret = None;
            }
        }
        ret
    }

    pub fn find_scroll_to_row(&self) -> Option<ScrollSpan> {
        assert_eq!(
            self.find_cursor.get_max(),
            self.find_ctx.cached_found_lines.len().saturating_sub(1)
        );

        let find_idx_1 = self
            .find_ctx
            .cached_found_lines
            .get(self.find_cursor.get())
            .cloned();

        // TODO: Improve so that user can decide which 1/2 file search operates on
        Some(ScrollSpan {
            start: find_idx_1.unwrap_or_default(),
            maybe_end: None,
        })
    }

    pub fn get_minimal_diff_ctx(&mut self) -> Option<MinimalDiffCtx> {
        if self.is_in_progress() {
            return None;
        }
        self.ctx.request_minimal_diff_ctx(self.cancel_flag.clone())
    }
}

fn precompute_diff_spans(diff_rows: &[DiffRow]) -> PrecomputedDiffs {
    let has_change = |content: &LineContent| match content {
        LineContent::Code { tokens, .. } => tokens
            .iter()
            .any(|(res, _, _)| !res.hide_in_diff && !matches!(res.operation, DiffOp::Equal(_))),
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
        .map(|chunk| DiffSpan {
            start: *chunk.first().unwrap(),
            end: *chunk.last().unwrap(),
        })
        .collect()
}

fn precompute_file_rows(
    diff_rows: &[DiffRow],
    file_1_line_count: usize,
    file_2_line_count: usize,
) -> PrecomputedFileRows {
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

fn shift_left_content_up(rows: &mut [DiffRow], offset: usize) {
    for i in 0..rows.len() {
        rows[i].left = if i + offset < rows.len() {
            rows[i + offset].left.clone()
        } else {
            LineContent::Void
        };
    }
}

fn shift_right_content_up(rows: &mut [DiffRow], offset: usize) {
    for i in 0..rows.len() {
        rows[i].right = if i + offset < rows.len() {
            rows[i + offset].right.clone()
        } else {
            LineContent::Void
        };
    }
}

fn align_rows_to_pivot(
    diff_rows: &mut Vec<DiffRow>,
    pivot_lines: PivotLines,
    precomputed_file_rows: &PrecomputedFileRows,
) {
    log::debug!("pivot: {:?}", pivot_lines);
    let found_diff_row_pivot_index_1 = precomputed_file_rows
        .0
        .get(pivot_lines.left.saturating_sub(1));
    let found_diff_row_pivot_index_2 = precomputed_file_rows
        .1
        .get(pivot_lines.right.saturating_sub(1));
    log::debug!(
        "found_diff_row_pivot_index_1: {:?}",
        found_diff_row_pivot_index_1
    );
    log::debug!(
        "found_diff_row_pivot_index_2: {:?}",
        found_diff_row_pivot_index_2
    );

    let (Some(left_pivot_row), Some(right_pivot_row)) =
        (found_diff_row_pivot_index_1, found_diff_row_pivot_index_2)
    else {
        return;
    };
    if left_pivot_row == right_pivot_row {
        return;
    }

    // +: pad right side
    // -: pad left side
    let row_offset = *left_pivot_row as isize - *right_pivot_row as isize;
    let shift = row_offset.unsigned_abs();
    log::debug!("pivot diff: {}", row_offset);

    let dummy_diff_row = DiffRow {
        left: LineContent::Void,
        right: LineContent::Void,
    };
    diff_rows.splice(0..0, std::iter::repeat_n(dummy_diff_row, shift));

    match row_offset.cmp(&0) {
        std::cmp::Ordering::Greater => shift_left_content_up(diff_rows, shift),
        std::cmp::Ordering::Less => shift_right_content_up(diff_rows, shift),
        std::cmp::Ordering::Equal => {
            panic!("Should early exit out before here")
        }
    }
}

// Initial MinimalDiffCtx code, does not handle partially invalidating the diffctx
#[allow(dead_code)]
fn update_diff_rows_minimal_diff_ctx(
    input: UpdateDiffRowsInput,
    cancel_flag: Arc<AtomicBool>,
) -> Option<MinimalDiffCtx> {
    #[cfg(feature = "debug_alloc")]
    let mut reg = stats_alloc::Region::new(&crate::STATS_ALLOC);
    track_alloc!(reg, "update_diff_rows");

    let (c1, c2, one_sided_diff_is_left) = resolve_files(&input.file_1, &input.file_2);
    let cmp = |a: &RawToken, b: &RawToken| compare_tokens(a, b, c1, c2);

    track_alloc!(reg, "before myers_diff");
    let myers_path = myers_diff_path(
        input.myers_diff_algorithm,
        &c1.tokens,
        &c2.tokens,
        cmp,
        cancel_flag.clone(),
    )?;
    track_alloc!(reg, "myers_diff");
    check_cancel!(cancel_flag, "myers_diff_path");

    let is_equal_left = one_sided_diff_is_left.unwrap_or(true);
    let diff_ir = DiffIR::new(&myers_path, is_equal_left, cancel_flag.clone())?;
    track_alloc!(reg, "DiffIR::new()");
    check_cancel!(cancel_flag, "DiffIR::new");

    track_alloc!(reg, "hash_file");
    let diff_rows = build_diff_rows(
        diff_ir,
        Some(&c1.tokens),
        Some(&c2.tokens),
        &input.options,
        c1.metadata.num_lines().max(c2.metadata.num_lines()),
    );
    track_alloc!(reg, "build_diff_rows");
    check_cancel!(cancel_flag, "build_diff_rows");

    let (final_rows, precomputed_diffs) = finalize_diff_rows(
        diff_rows,
        &input.options,
        c1.metadata.line_starts.len(),
        c2.metadata.line_starts.len(),
        &cancel_flag,
    )?;

    let precomputed_file_rows = precompute_file_rows(
        &final_rows,
        c1.metadata.line_starts.len(),
        c2.metadata.line_starts.len(),
    );
    Some(MinimalDiffCtx {
        #[cfg(debug_assertions)]
        debug_file_1_path: c1.path.clone(),
        #[cfg(debug_assertions)]
        debug_file_2_path: c2.path.clone(),
        input: input.clone(),
        num_add_deletes: myers_count_add_deletes(&myers_path),
        precomputed_diffs: Arc::new(precomputed_diffs),
        precomputed_file_rows: Arc::new(precomputed_file_rows),
        diff_rows: Arc::new(final_rows),
    })
}

fn resolve_files<'a>(
    f1: &'a Option<Arc<CachedFile<RawToken>>>,
    f2: &'a Option<Arc<CachedFile<RawToken>>>,
) -> (
    &'a CachedFile<RawToken>,
    &'a CachedFile<RawToken>,
    Option<bool>,
) {
    match (f1, f2) {
        (Some(c1), Some(c2)) => (c1, c2, None),
        (Some(c1), None) => (c1, c1, Some(true)),
        (None, Some(c2)) => (c2, c2, Some(false)),
        (None, None) => panic!("Only call this function with one of two files valid"),
    }
}

fn compare_tokens(
    a: &RawToken,
    b: &RawToken,
    c1: &CachedFile<RawToken>,
    c2: &CachedFile<RawToken>,
) -> bool {
    if a.as_ref().kind != b.as_ref().kind {
        return false;
    }
    let a_len = a.span.end - a.span.start;
    let b_len = b.span.end - b.span.start;

    if a_len != b_len {
        return false;
    }
    let a_bytes = &c1.contents.as_bytes()[a.span.start..a.span.end];
    let b_bytes = &c2.contents.as_bytes()[b.span.start..b.span.end];

    a_bytes == b_bytes
}

fn finalize_diff_rows(
    mut diff_rows: DiffRows,
    options: &DiffBuilderOptions,
    c1_lines: usize,
    c2_lines: usize,
    cancel_flag: &Arc<AtomicBool>,
) -> Option<(DiffRows, PrecomputedDiffs)> {
    if let Some(pivot_lines) = &options.pivot_lines {
        if pivot_lines.left > 0 && pivot_lines.right > 0 {
            let precomputed = precompute_file_rows(&diff_rows, c1_lines, c2_lines);
            align_rows_to_pivot(&mut diff_rows, *pivot_lines, &precomputed);
        }
    }

    check_cancel!(cancel_flag, "pivot_lines");

    let mut precomputed_diffs = precompute_diff_spans(&diff_rows);

    check_cancel!(cancel_flag, "precomputed_diffs");

    if let Some(diff_only_rows) = options.diff_only_with_extra_rows {
        let mut keep_indices = vec![false; diff_rows.len()];

        for &DiffSpan { start, end } in &precomputed_diffs {
            let bound_start = start.saturating_sub(diff_only_rows);
            let bound_end = (end + diff_only_rows).min(diff_rows.len().saturating_sub(1));
            for idx in bound_start..=bound_end {
                if idx < keep_indices.len() {
                    keep_indices[idx] = true;
                }
            }
        }

        let mut filtered_rows = Vec::with_capacity(diff_rows.len());
        let mut in_gap = false;

        for (idx, row) in diff_rows.into_iter().enumerate() {
            if keep_indices[idx] {
                filtered_rows.push(row);
                in_gap = false;
            } else if !in_gap {
                if diff_only_rows > 0 {
                    let mut void_row = row.clone();
                    void_row.left = LineContent::Collapsed;
                    void_row.right = LineContent::Collapsed;
                    filtered_rows.push(void_row);
                }
                in_gap = true;
            }
        }
        diff_rows = filtered_rows;
        precomputed_diffs = precompute_diff_spans(&diff_rows);
    }

    Some((diff_rows, precomputed_diffs))
}
