use std::{
    collections::BTreeMap,
    ops::{Range, RangeInclusive},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self},
    },
    time::{Duration, Instant},
};

#[cfg(debug_assertions)]
use zdiff::universal_path::UniversalPath;
use zdiff::{
    cached_file::CachedFile,
    diff_builder::{DiffBuilderOptions, DiffRow, LineContent, PivotLines, build_diff_rows},
    diff_ir::{DiffIR, DiffOp},
    ignore::IgnoreOptions,
    lexer::RawToken,
    myers::{
        MyersDiffAlgorithm, MyersNumAddDelete, MyersPath, line_diff, myers_count_add_deletes,
        token_diff,
    },
    row_text::build_row_text,
};

use crate::{
    clamped_cursor::ClampedCursor,
    scope,
    ui_egui::{active_side::ActiveSide, occurrence::find_occurrences},
};

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

/// A row shows its line's text, so `ordinal` picks the same match the pane paints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindHit {
    pub side: ActiveSide,
    /// 0-based. `row` is rebuilt from it after an expansion.
    pub line: usize,
    pub ordinal: usize,
    pub row: usize,
}

#[derive(Debug, Clone, Default)]
pub struct FindCtx {
    needle: String,
    /// Sorted by row, then side (left first), then position.
    hits: Vec<FindHit>,
}
impl FindCtx {
    pub fn new(find_input: &str, diff_ctx: &MinimalDiffCtx) -> Self {
        let mut hits = Vec::new();
        let files = [
            (ActiveSide::Left, &diff_ctx.input.file_1),
            (ActiveSide::Right, &diff_ctx.input.file_2),
        ];
        for (side, file) in files {
            if let Some(file) = file
                && !find_input.is_empty()
            {
                hits.extend(Self::search(file, find_input, side));
            }
        }
        let mut find_ctx = Self {
            needle: find_input.to_owned(),
            hits,
        };
        find_ctx.map_to_rows(&diff_ctx.precomputed_file_rows);
        log::debug!("create_find_ctx: {:?}", find_ctx);
        find_ctx
    }

    pub fn needle(&self) -> &str {
        &self.needle
    }

    pub fn hits(&self) -> &[FindHit] {
        &self.hits
    }

    fn search(file: &CachedFile<RawToken>, needle: &str, side: ActiveSide) -> Vec<FindHit> {
        let mut hits: Vec<FindHit> = Vec::new();
        for range in find_occurrences(&file.contents, needle) {
            let line = file.metadata.get_line_index(range.start);
            let ordinal = match hits.last() {
                Some(last) if last.line == line => last.ordinal + 1,
                _ => 0,
            };
            hits.push(FindHit {
                side,
                line,
                ordinal,
                row: 0,
            });
        }
        hits
    }

    fn map_to_rows(&mut self, file_rows: &PrecomputedFileRows) {
        for hit in &mut self.hits {
            let line_to_row = match hit.side {
                ActiveSide::Left => &file_rows.0,
                ActiveSide::Right => &file_rows.1,
            };
            hit.row = line_to_row[hit.line];
        }
        self.hits.sort_by_key(|hit| {
            (
                hit.row,
                hit.side == ActiveSide::Right,
                hit.line,
                hit.ordinal,
            )
        });
    }
}

#[derive(Debug)]
pub struct DiffSpan {
    start: usize,
    end: usize,
}
impl DiffSpan {
    pub fn rows(&self) -> RangeInclusive<usize> {
        self.start..=self.end
    }
}
pub type PrecomputedDiffs = Vec<DiffSpan>; // list spans with indicies of diff_rows of DiffOp != Equal from diff_rows
pub type PrecomputedFileRows = (Vec<usize>, Vec<usize>); // line mapping from DiffRow index to DiffRow line number
#[derive(Debug, PartialEq, Clone, Copy)]
pub struct ScrollSpan {
    pub start: usize,
    pub maybe_end: Option<usize>,
}
pub type DiffRows = Vec<DiffRow>; // Span with optional end

/// The rows a `Collapsed` row stands for, kept so the block can be expanded without
/// recomputing the diff.
#[derive(Debug)]
pub struct CollapsedBlock {
    /// Index of the block's `Collapsed` row in the unexpanded rows. Identifies the block.
    pub row: usize,
    hidden: Vec<DiffRow>,
}
pub type CollapsedBlocks = Vec<CollapsedBlock>;

/// Where a collapsed block sits in the shown rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowBlock {
    /// `CollapsedBlock::row` of the block.
    pub key: usize,
    /// The block's `Collapsed` row while any of its rows is hidden, then its revealed rows. Rows
    /// are revealed from the block's bottom up.
    pub rows: Range<usize>,
    /// Every hidden row is revealed, so the block has no `Collapsed` row.
    pub expanded: bool,
}

/// What a collapsed block's buttons ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockToggle {
    Expand,
    ExpandToScope,
    Collapse,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MyersCtxInput {
    pub file_1: Option<Arc<CachedFile<RawToken>>>,
    pub file_2: Option<Arc<CachedFile<RawToken>>>,
    pub algo: MyersDiffAlgorithm,
    /// Ignore options decide line equality, so changing them recomputes this stage.
    pub ignore: IgnoreOptions,
}
impl From<UpdateDiffRowsInput> for MyersCtxInput {
    fn from(input: UpdateDiffRowsInput) -> Self {
        Self {
            file_1: input.file_1,
            file_2: input.file_2,
            algo: input.myers_diff_algorithm,
            ignore: input.options.ignore,
        }
    }
}

impl From<&UpdateDiffRowsInput> for MyersCtxInput {
    fn from(input: &UpdateDiffRowsInput) -> Self {
        Self {
            file_1: input.file_1.clone(),
            file_2: input.file_2.clone(),
            algo: input.myers_diff_algorithm,
            ignore: input.options.ignore.clone(),
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
    pub diff_ir: Arc<DiffIR>,
    pub diff_options: DiffBuilderOptions,
}
#[derive(Debug)]
pub struct MyersCtx {
    input: MyersCtxInput,

    num_add_delete: MyersNumAddDelete,
    path: MyersPath,
    line_elapsed: Duration,
    token_elapsed: Duration,
}
#[derive(Debug)]
pub struct DiffIRCtx {
    input: DiffIRInput,
    diff_ir: Arc<DiffIR>,
    elapsed: Duration,
}
#[derive(Debug)]
pub struct DiffRowsCtx {
    input: DiffRowsInput,
    rows: Arc<DiffRows>,
    precomputed_diffs: Arc<PrecomputedDiffs>,
    precomputed_file_rows: Arc<PrecomputedFileRows>,
    collapsed_blocks: Arc<CollapsedBlocks>,
    /// Every block unexpanded.
    row_blocks: Arc<Vec<RowBlock>>,
    elapsed: Duration,
}

/// Compute time of each stage behind a diff. A stage served from its cache keeps the time it
/// took when it ran, so the total is what the shown diff cost, not what the last request did.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct DiffStageTimes {
    /// The Myers stage's two phases.
    pub line_diff: Duration,
    pub token_diff: Duration,
    pub diff_ir: Duration,
    pub diff_rows: Duration,
}
impl DiffStageTimes {
    pub fn total(&self) -> Duration {
        self.line_diff + self.token_diff + self.diff_ir + self.diff_rows
    }
}

#[derive(Debug)]
pub struct MinimalDiffCtx {
    #[cfg(debug_assertions)]
    pub debug_file_1_path: UniversalPath,
    #[cfg(debug_assertions)]
    pub debug_file_2_path: UniversalPath,

    pub input: UpdateDiffRowsInput,

    pub num_add_deletes: MyersNumAddDelete,
    pub stage_times: DiffStageTimes,
    pub precomputed_diffs: Arc<PrecomputedDiffs>,
    pub precomputed_file_rows: Arc<PrecomputedFileRows>,
    pub diff_rows: Arc<DiffRows>,
    /// The IR the rows were built from. Revert reads the token alignment from it.
    pub diff_ir: Arc<DiffIR>,
    /// Hidden rows of each collapsed block, ordered by row.
    pub collapsed_blocks: Arc<CollapsedBlocks>,
    /// Each collapsed block's place in `diff_rows`, ordered by row.
    pub row_blocks: Arc<Vec<RowBlock>>,
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

    /// A stage thread whose cancel flag is set exits without sending, so its in-flight input would
    /// never clear and would block respawning the same input later. Results still arriving for a
    /// forgotten input are dropped by `poll`.
    pub fn forget_inflight(&mut self) {
        self.myers_inflight_input = None;
        self.diff_ir_inflight_input = None;
        self.diff_rows_inflight_input = None;
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
                    let start = Instant::now();
                    let (c1, c2, _) = resolve_files(&input.file_1, &input.file_2);
                    let cmp = |a: &RawToken, b: &RawToken| compare_tokens(a, b, c1, c2);

                    // myers_diff_path's two phases, timed separately.
                    let (t1, t2) = (&c1.tokens, &c2.tokens);
                    let ignore = input.ignore.mask(t1, &c1.contents, t2, &c2.contents);
                    let result = line_diff(input.algo, t1, t2, &cmp, &ignore, cancel.clone())
                        .and_then(|hunks| {
                            let line_elapsed = start.elapsed();
                            let start = Instant::now();
                            let path = token_diff(
                                input.algo,
                                t1,
                                t2,
                                &hunks,
                                &cmp,
                                &ignore,
                                cancel.clone(),
                            )?;
                            Some((path, line_elapsed, start.elapsed()))
                        });
                    if let Some((path, line_elapsed, token_elapsed)) = result {
                        let num_add_delete = myers_count_add_deletes(&path);
                        let _ = tx.send(Some(MyersCtx {
                            input,
                            num_add_delete,
                            path,
                            line_elapsed,
                            token_elapsed,
                        }));
                    } else if !cancel.load(Ordering::Relaxed) {
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
                    let start = Instant::now();
                    let cancel_ref = cancel.clone();
                    if let Some(diff_ir) = DiffIR::new(&input.myers_path, is_equal_left, cancel) {
                        let _ = tx.send(Some(DiffIRCtx {
                            input,
                            diff_ir: Arc::new(diff_ir),
                            elapsed: start.elapsed(),
                        }));
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
                    let start = Instant::now();
                    let (c1, c2, _) = resolve_files(&input.file_1, &input.file_2);
                    let diff_rows = build_diff_rows(
                        (*input.diff_ir).clone(),
                        Some(&c1.tokens),
                        Some(&c2.tokens),
                        &c1.contents,
                        &c2.contents,
                        &input.diff_options,
                        c1.metadata.num_lines().max(c2.metadata.num_lines()),
                    );

                    if cancel.load(Ordering::Relaxed) {
                        return;
                    }

                    if let Some((rows, precomputed_diffs, collapsed_blocks)) = finalize_diff_rows(
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
                        let row_blocks = unexpanded_row_blocks(&collapsed_blocks);
                        let _ = tx.send(Some(DiffRowsCtx {
                            input,
                            rows: Arc::new(rows),
                            precomputed_diffs: Arc::new(precomputed_diffs),
                            precomputed_file_rows: Arc::new(precomputed_file_rows),
                            collapsed_blocks: Arc::new(collapsed_blocks),
                            row_blocks: Arc::new(row_blocks),
                            elapsed: start.elapsed(),
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
        let myers_ctx = self.request_myers(cancel_flag.clone())?;
        let num_add_deletes = myers_ctx.num_add_delete;
        let (line_elapsed, token_elapsed) = (myers_ctx.line_elapsed, myers_ctx.token_elapsed);
        let diff_row_ctx = self.request_diff_rows(cancel_flag)?;

        let diff_rows = diff_row_ctx.rows.clone();
        let precomputed_diffs = diff_row_ctx.precomputed_diffs.clone();
        let precomputed_file_rows = diff_row_ctx.precomputed_file_rows.clone();
        let diff_ir = diff_row_ctx.input.diff_ir.clone();
        let collapsed_blocks = diff_row_ctx.collapsed_blocks.clone();
        let row_blocks = diff_row_ctx.row_blocks.clone();
        let diff_rows_elapsed = diff_row_ctx.elapsed;
        // Rows are only returned when they were built from the current IR ctx. Read it here
        // rather than requesting it again, which would clone and compare the Myers path.
        let diff_ir_elapsed = self
            .diff_ir_ctx
            .as_ref()
            .expect("diff rows without a diff IR ctx")
            .elapsed;
        let stage_times = DiffStageTimes {
            line_diff: line_elapsed,
            token_diff: token_elapsed,
            diff_ir: diff_ir_elapsed,
            diff_rows: diff_rows_elapsed,
        };

        Some(MinimalDiffCtx {
            #[cfg(debug_assertions)]
            debug_file_1_path: self.debug_file_1_path.clone(),
            #[cfg(debug_assertions)]
            debug_file_2_path: self.debug_file_2_path.clone(),
            input: self.update_diff_rows_input.clone(),
            num_add_deletes,
            stage_times,
            precomputed_diffs,
            precomputed_file_rows,
            diff_rows,
            diff_ir,
            collapsed_blocks,
            row_blocks,
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
    /// `None`: both sides.
    pub highlight_side: Option<ActiveSide>,
    pub pivot: (Option<usize>, Option<usize>),
    pub find_cursor: ClampedCursor,
    pub find_ctx: FindCtx,
    goto_line_number: Option<usize>,
    pub active_side: ActiveSide,

    last_conflict_scroll_to_row: Option<ScrollSpan>,
    last_find_scroll_to_row: Option<ScrollSpan>,
    /// `None` while every block is collapsed.
    expansion: Option<RowExpansion>,
}

impl Default for DiffProcessor {
    fn default() -> Self {
        Self {
            ctx: Default::default(),
            conflict_cursor: ClampedCursor::default(),
            active_highlights: Vec::new(),
            highlight_side: None,
            pivot: (None, None),
            in_progress_input: None,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            find_cursor: ClampedCursor::default(),
            find_ctx: FindCtx::default(),
            goto_line_number: None,
            active_side: ActiveSide::default(),
            last_conflict_scroll_to_row: None,
            last_find_scroll_to_row: None,
            expansion: None,
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
        self.highlight_side = None;
        self.last_conflict_scroll_to_row = None;
        self.last_find_scroll_to_row = None;
        self.expansion = None;
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
        self.ctx.forget_inflight();
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
            .set_max(self.find_ctx.hits.len().saturating_sub(1));
        self.find_cursor.set(0);
    }

    pub fn clear_results(&mut self) {
        self.update_find(FindCtx::default());
        self.goto_line_number = None;
        self.active_highlights.clear();
        self.highlight_side = None;
    }

    pub fn current_find_hit(&self) -> Option<FindHit> {
        self.find_ctx.hits.get(self.find_cursor.get()).copied()
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

        let goto_side = self.active_side;
        let goto = self.goto_scroll_to_row();

        let find = check_update(self.find_scroll_to_row(), &mut self.last_find_scroll_to_row);

        let scroll_to_row = find.or(goto).or(conflict);

        if let Some(ScrollSpan { start, maybe_end }) = &scroll_to_row {
            self.highlight_side = (find.is_none() && goto.is_some()).then_some(goto_side);
            self.active_highlights.clear();
            if let Some(end) = maybe_end {
                self.active_highlights.extend(*start..=*end);
            } else {
                self.active_highlights.push(*start);
            }
        }

        scroll_to_row
    }

    /// One-shot: the request is kept until the diff is ready, then consumed, so going to the
    /// same line again scrolls again.
    fn goto_scroll_to_row(&mut self) -> Option<ScrollSpan> {
        let line = self.goto_line_number?;
        let side = self.active_side;
        let diff_ctx = self.get_minimal_diff_ctx()?;
        self.goto_line_number = None;
        let line_to_row = match side {
            ActiveSide::Left => &diff_ctx.precomputed_file_rows.0,
            ActiveSide::Right => &diff_ctx.precomputed_file_rows.1,
        };
        row_for_line(line, line_to_row).map(|start| ScrollSpan {
            start,
            maybe_end: None,
        })
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
            self.find_ctx.hits.len().saturating_sub(1)
        );

        self.current_find_hit().map(|hit| ScrollSpan {
            start: hit.row,
            maybe_end: None,
        })
    }

    pub fn get_minimal_diff_ctx(&mut self) -> Option<MinimalDiffCtx> {
        if self.is_in_progress() {
            return None;
        }
        let mut ctx = self
            .ctx
            .request_minimal_diff_ctx(self.cancel_flag.clone())?;
        if let Some(expansion) = &self.expansion {
            if Arc::ptr_eq(&expansion.base, &ctx.diff_rows) {
                expansion.apply(&mut ctx);
            } else {
                // reset_ui drops the expansion whenever a rebuild completes.
                log::error!("Rows were rebuilt under an expansion; dropping it");
                self.expansion = None;
            }
        }
        Some(ctx)
    }

    pub fn toggle_block(&mut self, key: usize, toggle: BlockToggle) {
        match toggle {
            BlockToggle::Expand => self.set_block_expanded(key, true),
            BlockToggle::ExpandToScope => self.expand_block_to_scope(key),
            BlockToggle::Collapse => self.set_block_expanded(key, false),
        }
    }

    /// Expands or re-collapses the whole block whose `RowBlock::key` is `key`.
    pub fn set_block_expanded(&mut self, key: usize, expanded: bool) {
        self.set_block_revealed_from(key, expanded.then_some(0));
    }

    /// Reveals the block's rows from the line opening the scope that encloses the change below
    /// the block. The whole block when that line isn't hidden in it.
    pub fn expand_block_to_scope(&mut self, key: usize) {
        let from = self
            .get_minimal_diff_ctx()
            .and_then(|ctx| scope_reveal_from(&ctx, key))
            .unwrap_or(0);
        self.set_block_revealed_from(key, Some(from));
    }

    /// Reveals the block's hidden rows from index `from` down, or none of them. Row indices held
    /// across frames (find hits, highlights, the last scroll targets) follow their rows, so the
    /// view doesn't jump.
    fn set_block_revealed_from(&mut self, key: usize, from: Option<usize>) {
        let Some(old) = self.get_minimal_diff_ctx() else {
            log::warn!("No diff shown to expand a block in");
            return;
        };
        if !old.row_blocks.iter().any(|block| block.key == key) {
            log::error!("No collapsed block at row {key}");
            return;
        }

        let mut keys = self
            .expansion
            .take()
            .map(|expansion| expansion.expanded)
            .unwrap_or_default();
        match from {
            Some(from) => keys.insert(key, from),
            None => keys.remove(&key),
        };
        if !keys.is_empty() {
            let base = self.get_minimal_diff_ctx().expect("diff shown above");
            self.expansion = Some(RowExpansion::new(&base, keys));
        }
        let new = self.get_minimal_diff_ctx().expect("diff shown above");

        let remap = |row| remap_row(&old.row_blocks, &new.row_blocks, row);
        let current = self.current_find_hit();
        self.find_ctx.map_to_rows(&new.precomputed_file_rows);
        if let Some(current) = current {
            let same = |hit: &FindHit| {
                (hit.side, hit.line, hit.ordinal) == (current.side, current.line, current.ordinal)
            };
            let index = self.find_ctx.hits.iter().position(same);
            self.find_cursor
                .set(index.expect("a remap keeps every hit"));
        }
        let mut highlights: Vec<usize> = self.active_highlights.iter().map(|&r| remap(r)).collect();
        highlights.dedup();
        self.active_highlights = highlights;
        self.last_conflict_scroll_to_row = self.conflict_scroll_to_row();
        self.last_find_scroll_to_row = self.find_scroll_to_row();
    }
}

/// Index of the first hidden row of block `key` that a scope expansion reveals. `None` when no
/// scope opens inside the block, or no change follows it.
fn scope_reveal_from(ctx: &MinimalDiffCtx, key: usize) -> Option<usize> {
    let block = ctx.row_blocks.iter().find(|block| block.key == key)?;
    let hidden = &ctx
        .collapsed_blocks
        .iter()
        .find(|block| block.row == key)?
        .hidden;
    // Blocks lie between changes, so the rows down to the next change are shown context.
    let change = ctx
        .precomputed_diffs
        .iter()
        .find(|span| span.start >= block.rows.end)?
        .start;
    // Context rows show the same line on both sides (up to ignore options), so either will do.
    let text = |row: &DiffRow| match (&row.left, &row.right) {
        (LineContent::Code { tokens, .. }, _) | (_, LineContent::Code { tokens, .. }) => {
            build_row_text(
                tokens,
                ctx.input.file_1.as_deref(),
                ctx.input.file_2.as_deref(),
            )
            .text
        }
        _ => String::new(),
    };
    let hidden: Vec<String> = hidden.iter().map(text).collect();
    let below: Vec<String> = ctx.diff_rows[block.rows.end..change]
        .iter()
        .map(text)
        .collect();
    scope::scope_opening(&hidden, &below)
}

/// The rows shown with some collapsed blocks expanded, built once per change.
#[derive(Debug)]
struct RowExpansion {
    /// The unexpanded rows the keys refer to. Rebuilt rows invalidate the keys.
    base: Arc<DiffRows>,
    /// Block key to the index of its first revealed hidden row.
    expanded: BTreeMap<usize, usize>,
    diff_rows: Arc<DiffRows>,
    precomputed_diffs: Arc<PrecomputedDiffs>,
    precomputed_file_rows: Arc<PrecomputedFileRows>,
    row_blocks: Arc<Vec<RowBlock>>,
}
impl RowExpansion {
    fn new(base: &MinimalDiffCtx, expanded: BTreeMap<usize, usize>) -> Self {
        let (rows, row_blocks) = expand_rows(&base.diff_rows, &base.collapsed_blocks, &expanded);
        let (c1, c2, _) = resolve_files(&base.input.file_1, &base.input.file_2);
        let precomputed_file_rows = precompute_file_rows(
            &rows,
            c1.metadata.line_starts.len(),
            c2.metadata.line_starts.len(),
        );
        Self {
            base: base.diff_rows.clone(),
            expanded,
            precomputed_diffs: Arc::new(precompute_diff_spans(&rows)),
            precomputed_file_rows: Arc::new(precomputed_file_rows),
            diff_rows: Arc::new(rows),
            row_blocks: Arc::new(row_blocks),
        }
    }

    fn apply(&self, ctx: &mut MinimalDiffCtx) {
        ctx.diff_rows = self.diff_rows.clone();
        ctx.precomputed_diffs = self.precomputed_diffs.clone();
        ctx.precomputed_file_rows = self.precomputed_file_rows.clone();
        ctx.row_blocks = self.row_blocks.clone();
    }
}

/// Diff row of the 1-based file `line`, given each file line's row. Line 0 counts as the first
/// line and a line past the end as the last. `None` for an empty file.
fn row_for_line(line: usize, line_to_row: &[usize]) -> Option<usize> {
    let last = line_to_row.len().checked_sub(1)?;
    line_to_row.get(line.saturating_sub(1).min(last)).copied()
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
    let (algo, t1, t2) = (input.myers_diff_algorithm, &c1.tokens, &c2.tokens);
    let start = Instant::now();
    let ignore = input
        .options
        .ignore
        .mask(t1, &c1.contents, t2, &c2.contents);
    let hunks = line_diff(algo, t1, t2, &cmp, &ignore, cancel_flag.clone())?;
    let line_elapsed = start.elapsed();
    let start = Instant::now();
    let myers_path = token_diff(algo, t1, t2, &hunks, &cmp, &ignore, cancel_flag.clone())?;
    let token_elapsed = start.elapsed();
    track_alloc!(reg, "myers_diff");
    check_cancel!(cancel_flag, "myers_diff_path");

    let is_equal_left = one_sided_diff_is_left.unwrap_or(true);
    let start = Instant::now();
    let diff_ir = DiffIR::new(&myers_path, is_equal_left, cancel_flag.clone())?;
    let diff_ir_elapsed = start.elapsed();
    track_alloc!(reg, "DiffIR::new()");
    check_cancel!(cancel_flag, "DiffIR::new");

    track_alloc!(reg, "hash_file");
    let start = Instant::now();
    let diff_rows = build_diff_rows(
        diff_ir.clone(),
        Some(&c1.tokens),
        Some(&c2.tokens),
        &c1.contents,
        &c2.contents,
        &input.options,
        c1.metadata.num_lines().max(c2.metadata.num_lines()),
    );
    track_alloc!(reg, "build_diff_rows");
    check_cancel!(cancel_flag, "build_diff_rows");

    let (final_rows, precomputed_diffs, collapsed_blocks) = finalize_diff_rows(
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
    let stage_times = DiffStageTimes {
        line_diff: line_elapsed,
        token_diff: token_elapsed,
        diff_ir: diff_ir_elapsed,
        diff_rows: start.elapsed(),
    };
    Some(MinimalDiffCtx {
        #[cfg(debug_assertions)]
        debug_file_1_path: c1.path.clone(),
        #[cfg(debug_assertions)]
        debug_file_2_path: c2.path.clone(),
        input: input.clone(),
        num_add_deletes: myers_count_add_deletes(&myers_path),
        stage_times,
        precomputed_diffs: Arc::new(precomputed_diffs),
        precomputed_file_rows: Arc::new(precomputed_file_rows),
        diff_rows: Arc::new(final_rows),
        diff_ir: Arc::new(diff_ir),
        row_blocks: Arc::new(unexpanded_row_blocks(&collapsed_blocks)),
        collapsed_blocks: Arc::new(collapsed_blocks),
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

/// Pivot alignment and diff-only collapsing. A block collapsed with zero context rows leaves no
/// `Collapsed` row, so it has no `CollapsedBlock` and can't be expanded.
pub(crate) fn finalize_diff_rows(
    mut diff_rows: DiffRows,
    options: &DiffBuilderOptions,
    c1_lines: usize,
    c2_lines: usize,
    cancel_flag: &Arc<AtomicBool>,
) -> Option<(DiffRows, PrecomputedDiffs, CollapsedBlocks)> {
    if let Some(pivot_lines) = &options.pivot_lines {
        if pivot_lines.left > 0 && pivot_lines.right > 0 {
            let precomputed = precompute_file_rows(&diff_rows, c1_lines, c2_lines);
            align_rows_to_pivot(&mut diff_rows, *pivot_lines, &precomputed);
        }
    }

    check_cancel!(cancel_flag, "pivot_lines");

    let mut precomputed_diffs = precompute_diff_spans(&diff_rows);
    let mut collapsed_blocks = CollapsedBlocks::new();

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
            } else if diff_only_rows == 0 {
                // No Collapsed row to expand from, so the row is dropped.
            } else {
                if !in_gap {
                    collapsed_blocks.push(CollapsedBlock {
                        row: filtered_rows.len(),
                        hidden: Vec::new(),
                    });
                    filtered_rows.push(DiffRow {
                        left: LineContent::Collapsed,
                        right: LineContent::Collapsed,
                    });
                    in_gap = true;
                }
                collapsed_blocks
                    .last_mut()
                    .expect("a gap starts with a block")
                    .hidden
                    .push(row);
            }
        }
        diff_rows = filtered_rows;
        precomputed_diffs = precompute_diff_spans(&diff_rows);
    }

    Some((diff_rows, precomputed_diffs, collapsed_blocks))
}

/// `rows` (the unexpanded rows) with the hidden rows of every block in `expanded` put back,
/// and where each block ends up. `expanded` maps a block's key to the index of its first
/// revealed hidden row. The rows above it stay behind the `Collapsed` row; from 0 the whole
/// block replaces it.
pub(crate) fn expand_rows(
    rows: &[DiffRow],
    blocks: &[CollapsedBlock],
    expanded: &BTreeMap<usize, usize>,
) -> (DiffRows, Vec<RowBlock>) {
    let extra: usize = blocks
        .iter()
        .filter_map(|block| {
            let from = *expanded.get(&block.row)?;
            Some(block.hidden.len() - from - usize::from(from == 0))
        })
        .sum();
    let mut shown = Vec::with_capacity(rows.len() + extra);
    let mut row_blocks = Vec::with_capacity(blocks.len());
    let mut next = 0;
    for block in blocks {
        assert!(
            matches!(rows[block.row].left, LineContent::Collapsed),
            "block key {} is not a collapsed row",
            block.row
        );
        shown.extend_from_slice(&rows[next..block.row]);
        let start = shown.len();
        let from = expanded.get(&block.row).copied();
        match from {
            None => shown.push(rows[block.row].clone()),
            Some(from) => {
                assert!(
                    from < block.hidden.len(),
                    "block key {} reveals from {from} of {} hidden rows",
                    block.row,
                    block.hidden.len()
                );
                if from > 0 {
                    shown.push(rows[block.row].clone());
                }
                shown.extend_from_slice(&block.hidden[from..]);
            }
        }
        row_blocks.push(RowBlock {
            key: block.row,
            rows: start..shown.len(),
            expanded: from == Some(0),
        });
        next = block.row + 1;
    }
    shown.extend_from_slice(&rows[next..]);
    (shown, row_blocks)
}

/// Every block of `blocks` unexpanded.
fn unexpanded_row_blocks(blocks: &[CollapsedBlock]) -> Vec<RowBlock> {
    blocks
        .iter()
        .map(|block| RowBlock {
            key: block.row,
            rows: block.row..block.row + 1,
            expanded: false,
        })
        .collect()
}

/// The shown row `row` of a view laid out as `from`, in a view of the same rows laid out as
/// `to`. A row inside a block that `to` hides maps to its `Collapsed` row.
fn remap_row(from: &[RowBlock], to: &[RowBlock], row: usize) -> usize {
    // Shown rows minus unexpanded rows, after a block.
    let shift = |block: &RowBlock| block.rows.end - (block.key + 1);

    // A block's hidden rows are revealed from its bottom up, so a revealed row is identified by
    // how far above the block's last row it is. `None` for rows outside blocks and Collapsed rows.
    let mut unexpanded = (row, None);
    for block in from {
        if row < block.rows.start {
            break;
        }
        if block.rows.contains(&row) {
            let is_collapsed_row = !block.expanded && row == block.rows.start;
            unexpanded = (
                block.key,
                (!is_collapsed_row).then(|| block.rows.end - 1 - row),
            );
            break;
        }
        unexpanded = (row - shift(block), None);
    }

    let (row, above_bottom) = unexpanded;
    let mut shown = row;
    for block in to {
        if row < block.key {
            break;
        }
        if row == block.key {
            let revealed = block.rows.len() - usize::from(!block.expanded);
            return match above_bottom {
                Some(above) if above < revealed => block.rows.end - 1 - above,
                _ => block.rows.start,
            };
        }
        shown = row + shift(block);
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    // Row of each line, 0-based. A ghost/void row sits at row 1, another at row 4.
    const LINE_TO_ROW: [usize; 4] = [0, 2, 3, 5];

    #[test]
    fn goto_line_one_is_the_first_line() {
        assert_eq!(row_for_line(1, &LINE_TO_ROW), Some(0));
    }

    #[test]
    fn goto_line_after_ghost_rows_lands_on_that_lines_row() {
        assert_eq!(row_for_line(2, &LINE_TO_ROW), Some(2));
        assert_eq!(row_for_line(4, &LINE_TO_ROW), Some(5));
    }

    #[test]
    fn goto_past_the_last_line_clamps_to_it() {
        assert_eq!(row_for_line(5, &LINE_TO_ROW), Some(5));
        assert_eq!(row_for_line(usize::MAX, &LINE_TO_ROW), Some(5));
    }

    #[test]
    fn goto_line_zero_is_the_first_line() {
        assert_eq!(row_for_line(0, &LINE_TO_ROW), Some(0));
    }

    #[test]
    fn goto_in_an_empty_file_has_no_row() {
        assert_eq!(row_for_line(1, &[]), None);
    }

    #[test]
    fn remap_moves_rows_by_the_rows_an_expansion_adds_or_removes() {
        let block = |key, rows, expanded| RowBlock {
            key,
            rows,
            expanded,
        };
        // Blocks at unexpanded rows 2 and 5; the first hides 4 rows.
        let collapsed = [block(2, 2..3, false), block(5, 5..6, false)];
        let expanded = [block(2, 2..6, true), block(5, 8..9, false)];

        let forth: Vec<_> = (0..7)
            .map(|row| remap_row(&collapsed, &expanded, row))
            .collect();
        assert_eq!(forth, [0, 1, 2, 6, 7, 8, 9]);
        let back: Vec<_> = (0..10)
            .map(|row| remap_row(&expanded, &collapsed, row))
            .collect();
        assert_eq!(back, [0, 1, 2, 2, 2, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn remap_keeps_rows_revealed_from_the_bottom_of_a_partly_expanded_block() {
        let block = |key, rows, expanded| RowBlock {
            key,
            rows,
            expanded,
        };
        // The first block hides 6 rows. Partly expanded, its last 3 show below its Collapsed row.
        let collapsed = [block(2, 2..3, false), block(5, 5..6, false)];
        let partial = [block(2, 2..6, false), block(5, 8..9, false)];
        let full = [block(2, 2..8, true), block(5, 10..11, false)];
        let remap = |from: &[RowBlock], to: &[RowBlock], rows: usize| -> Vec<usize> {
            (0..rows).map(|row| remap_row(from, to, row)).collect()
        };

        assert_eq!(remap(&collapsed, &partial, 7), [0, 1, 2, 6, 7, 8, 9]);
        assert_eq!(
            remap(&partial, &collapsed, 10),
            [0, 1, 2, 2, 2, 2, 3, 4, 5, 6]
        );
        assert_eq!(remap(&partial, &full, 10), [0, 1, 2, 5, 6, 7, 8, 9, 10, 11]);
        assert_eq!(
            remap(&full, &partial, 12),
            [0, 1, 2, 2, 2, 3, 4, 5, 6, 7, 8, 9]
        );
    }

    mod pipeline {
        use std::{
            path::Path,
            time::{Duration, Instant},
        };

        use zcommon::logger::LogCollector;
        use zdiff::{ignore::IgnorePatterns, universal_path::UniversalPath};

        use super::*;
        use crate::file::FileProcessor;

        const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

        /// `lines` lines where every 7th line depends on `seed`, so two seeds give a scattered diff.
        fn write_source(path: &Path, lines: usize, seed: usize) -> UniversalPath {
            let contents: String = (0..lines)
                .map(|i| {
                    if i % 7 == 0 {
                        format!("let edited_{i} = {};\n", seed * 31 + i)
                    } else {
                        format!("fn line_{i}() -> usize {{ {} }}\n", i * 3)
                    }
                })
                .collect();
            std::fs::write(path, contents).unwrap();
            UniversalPath::from(path.to_path_buf())
        }

        /// Loads through a `FileProcessor` like the app does. `None` when the load failed.
        fn load(path: &UniversalPath) -> Option<Arc<CachedFile<RawToken>>> {
            let mut file = FileProcessor::new();
            file.set_path(path.clone());
            let start = Instant::now();
            loop {
                let cached = file.get_cached_file();
                if file.get_loading_path().is_none() {
                    return cached;
                }
                assert!(
                    start.elapsed() < SETTLE_TIMEOUT,
                    "load of {path:?} never finished"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        fn input(
            file_1: &Option<Arc<CachedFile<RawToken>>>,
            file_2: &Option<Arc<CachedFile<RawToken>>>,
        ) -> UpdateDiffRowsInput {
            UpdateDiffRowsInput {
                file_1: file_1.clone(),
                file_2: file_2.clone(),
                ..Default::default()
            }
        }

        /// One frame of the app opening a pair: request it, then poll once.
        fn open(processor: &mut DiffProcessor, input: &UpdateDiffRowsInput) {
            processor.request_update(input.clone());
            processor.update();
        }

        fn settle(processor: &mut DiffProcessor) -> Option<MinimalDiffCtx> {
            let start = Instant::now();
            while processor.is_in_progress() {
                if start.elapsed() > SETTLE_TIMEOUT {
                    return None;
                }
                std::thread::sleep(Duration::from_millis(1));
                processor.update();
            }
            processor.get_minimal_diff_ctx()
        }

        #[test]
        fn reopening_a_pair_after_a_cached_pair_interrupted_it_completes() {
            let dir = tempfile::tempdir().unwrap();
            let a = (
                load(&write_source(&dir.path().join("a1.rs"), 1000, 1)),
                load(&write_source(&dir.path().join("a2.rs"), 1000, 2)),
            );
            let b = (
                load(&write_source(&dir.path().join("b1.rs"), 40, 3)),
                load(&write_source(&dir.path().join("b2.rs"), 40, 4)),
            );
            let (input_a, input_b) = (input(&a.0, &a.1), input(&b.0, &b.1));
            let mut processor = DiffProcessor::default();

            open(&mut processor, &input_b);
            assert!(
                settle(&mut processor).is_some(),
                "first diff of B never completed"
            );

            open(&mut processor, &input_a);
            assert!(processor.is_in_progress(), "A should still be diffing");
            open(&mut processor, &input_b);
            assert!(
                !processor.is_in_progress(),
                "B should be served from the stage caches"
            );
            open(&mut processor, &input_a);

            let ctx = settle(&mut processor).expect("re-opened pair A never completed");
            assert!(ctx.input == input_a, "diff shows {:?}", ctx.input);
        }

        #[test]
        fn failed_load_mid_sequence_is_logged_and_next_request_diffs() {
            let logs = LogCollector::init().expect("no other logger in the test binary");
            let dir = tempfile::tempdir().unwrap();
            let a = (
                load(&write_source(&dir.path().join("a1.rs"), 300, 1)),
                load(&write_source(&dir.path().join("a2.rs"), 300, 2)),
            );
            let b = (
                load(&write_source(&dir.path().join("b1.rs"), 300, 3)),
                load(&write_source(&dir.path().join("b2.rs"), 300, 4)),
            );
            let mut processor = DiffProcessor::default();

            open(&mut processor, &input(&b.0, &b.1));
            assert!(
                settle(&mut processor).is_some(),
                "diff of B never completed"
            );
            open(&mut processor, &input(&a.0, &a.1));

            let missing = dir.path().join("missing.rs");
            let missing_file = load(&UniversalPath::from(missing.clone()));
            assert!(missing_file.is_none());
            let missing_display = missing.display().to_string();
            assert!(
                logs.lock()
                    .unwrap()
                    .iter()
                    .any(|l| l.starts_with("[ERROR]") && l.contains(&missing_display)),
                "no error logged for {missing_display}"
            );

            // The app diffs whatever side did load, so the pair becomes one-sided.
            open(&mut processor, &input(&missing_file, &b.1));
            let one_sided = settle(&mut processor)
                .expect("one-sided diff after the failed load never completed");
            assert!(one_sided.input.file_1.is_none());
            open(&mut processor, &input(&a.0, &a.1));

            let ctx = settle(&mut processor).expect("pair after the failed load never completed");
            assert!(ctx.input == input(&a.0, &a.1), "diff shows {:?}", ctx.input);
            assert!(!ctx.diff_rows.is_empty());
        }

        #[test]
        fn completed_diff_carries_each_stage_time_and_their_sum() {
            let dir = tempfile::tempdir().unwrap();
            let a = (
                load(&write_source(&dir.path().join("a1.rs"), 1000, 1)),
                load(&write_source(&dir.path().join("a2.rs"), 1000, 2)),
            );
            let mut processor = DiffProcessor::default();

            open(&mut processor, &input(&a.0, &a.1));
            let times = settle(&mut processor)
                .expect("diff never completed")
                .stage_times;

            assert!(times.line_diff > Duration::ZERO, "{times:?}");
            assert!(times.token_diff > Duration::ZERO, "{times:?}");
            assert!(times.diff_ir > Duration::ZERO, "{times:?}");
            assert!(times.diff_rows > Duration::ZERO, "{times:?}");
            assert_eq!(
                times.total(),
                times.line_diff + times.token_diff + times.diff_ir + times.diff_rows
            );
        }

        #[test]
        fn no_stage_times_until_a_diff_completes_and_none_while_another_is_in_flight() {
            let dir = tempfile::tempdir().unwrap();
            let a = (
                load(&write_source(&dir.path().join("a1.rs"), 1000, 1)),
                load(&write_source(&dir.path().join("a2.rs"), 1000, 2)),
            );
            let b = (
                load(&write_source(&dir.path().join("b1.rs"), 40, 3)),
                load(&write_source(&dir.path().join("b2.rs"), 40, 4)),
            );
            let mut processor = DiffProcessor::default();
            assert!(processor.get_minimal_diff_ctx().is_none());

            open(&mut processor, &input(&b.0, &b.1));
            assert!(processor.get_minimal_diff_ctx().is_none());
            let times_b = settle(&mut processor)
                .expect("diff of B never completed")
                .stage_times;

            open(&mut processor, &input(&a.0, &a.1));
            assert!(processor.is_in_progress(), "A should still be diffing");
            assert!(processor.get_minimal_diff_ctx().is_none());

            // B again is served from the stage caches, so it shows what B cost when it ran.
            open(&mut processor, &input(&b.0, &b.1));
            assert!(!processor.is_in_progress());
            let ctx = processor.get_minimal_diff_ctx().expect("cached B");
            assert_eq!(ctx.stage_times, times_b);
        }

        #[test]
        fn toggling_ignore_whitespace_recomputes_the_diff_stage_and_highlight_rows_does_not() {
            let dir = tempfile::tempdir().unwrap();
            let a = (
                load(&write_source(&dir.path().join("a1.rs"), 300, 1)),
                load(&write_source(&dir.path().join("a2.rs"), 300, 2)),
            );
            let mut processor = DiffProcessor::default();
            let mut current = input(&a.0, &a.1);
            open(&mut processor, &current);
            let times = settle(&mut processor)
                .expect("first diff never completed")
                .stage_times;

            // A stage served from its cache keeps its time, and spawns no thread.
            current.options.highlight_rows = !current.options.highlight_rows;
            open(&mut processor, &current);
            assert!(processor.ctx.myers_inflight_input.is_none());
            assert!(processor.ctx.diff_rows_inflight_input.is_some());
            let after = settle(&mut processor).expect("highlight toggle never completed");
            assert_eq!(after.stage_times.line_diff, times.line_diff);
            assert_eq!(after.stage_times.token_diff, times.token_diff);

            current.options.ignore.whitespace = !current.options.ignore.whitespace;
            open(&mut processor, &current);
            assert!(
                processor.ctx.myers_inflight_input.is_some(),
                "ignore-whitespace must recompute the diff stage"
            );
            let ctx = settle(&mut processor).expect("ignore toggle never completed");
            assert!(ctx.input == current, "diff shows {:?}", ctx.input);
        }

        #[test]
        fn toggling_ignore_comments_recomputes_the_diff_stage() {
            let dir = tempfile::tempdir().unwrap();
            let a = (
                load(&write_source(&dir.path().join("a1.rs"), 300, 1)),
                load(&write_source(&dir.path().join("a2.rs"), 300, 2)),
            );
            let mut processor = DiffProcessor::default();
            let mut current = input(&a.0, &a.1);
            open(&mut processor, &current);
            settle(&mut processor).expect("first diff never completed");

            current.options.ignore.comments = !current.options.ignore.comments;
            open(&mut processor, &current);
            assert!(
                processor.ctx.myers_inflight_input.is_some(),
                "ignore-comments must recompute the diff stage"
            );
            let ctx = settle(&mut processor).expect("ignore toggle never completed");
            assert!(ctx.input == current, "diff shows {:?}", ctx.input);
        }

        #[test]
        fn editing_ignore_patterns_recomputes_the_diff_stage() {
            let dir = tempfile::tempdir().unwrap();
            let a = (
                load(&write_source(&dir.path().join("a1.rs"), 300, 1)),
                load(&write_source(&dir.path().join("a2.rs"), 300, 2)),
            );
            let mut processor = DiffProcessor::default();
            let mut current = input(&a.0, &a.1);
            open(&mut processor, &current);
            settle(&mut processor).expect("first diff never completed");

            current.options.ignore.patterns = IgnorePatterns::new("\\d+");
            open(&mut processor, &current);
            assert!(
                processor.ctx.myers_inflight_input.is_some(),
                "a pattern edit must recompute the diff stage"
            );
            let ctx = settle(&mut processor).expect("pattern edit never completed");
            assert!(ctx.input == current, "diff shows {:?}", ctx.input);
        }

        mod expansion {
            use super::*;
            use crate::revert::{RevertRequest, RevertTarget, plan_hunk_revert};

            type Pair = (
                Option<Arc<CachedFile<RawToken>>>,
                Option<Arc<CachedFile<RawToken>>>,
            );

            /// 20 lines where lines 3 and 18 differ, so 2 context rows collapse lines 6..=15.
            fn gap_pair(dir: &Path) -> Pair {
                let write = |name: &str, edit: bool| {
                    let contents: String = (1..=20)
                        .map(|n| match edit && (n == 3 || n == 18) {
                            true => format!("edit_{n}\n"),
                            false => format!("keep_{n}\n"),
                        })
                        .collect();
                    let path = dir.join(name);
                    std::fs::write(&path, contents).unwrap();
                    load(&UniversalPath::from(path))
                };
                (write("left.rs", false), write("right.rs", true))
            }

            fn diff_only(pair: &Pair, context_rows: usize) -> UpdateDiffRowsInput {
                let mut input = input(&pair.0, &pair.1);
                input.options.diff_only_with_extra_rows = Some(context_rows);
                input
            }

            fn opened(input: &UpdateDiffRowsInput) -> (DiffProcessor, MinimalDiffCtx) {
                let mut processor = DiffProcessor::default();
                open(&mut processor, input);
                let ctx = settle(&mut processor).expect("diff never completed");
                (processor, ctx)
            }

            /// (left, right) line number, 0 where a side has no line.
            fn line_nums(row: &DiffRow) -> (i32, i32) {
                let num = |content: &LineContent| match content {
                    LineContent::Code { line_num, .. } => *line_num,
                    _ => 0,
                };
                (num(&row.left), num(&row.right))
            }

            fn collapsed_rows(rows: &[DiffRow]) -> Vec<usize> {
                (0..rows.len())
                    .filter(|&i| matches!(rows[i].left, LineContent::Collapsed))
                    .collect()
            }

            fn row_of_left_line(rows: &[DiffRow], line: i32) -> usize {
                rows.iter()
                    .position(|row| line_nums(row).0 == line)
                    .unwrap_or_else(|| panic!("no row shows left line {line}"))
            }

            fn only_block(ctx: &MinimalDiffCtx) -> usize {
                let [key] = collapsed_rows(&ctx.diff_rows)[..] else {
                    panic!("expected one collapsed row: {:#?}", ctx.diff_rows);
                };
                key
            }

            #[test]
            fn expanding_restores_the_hidden_rows_at_the_block_with_their_line_numbers() {
                let dir = tempfile::tempdir().unwrap();
                let pair = gap_pair(dir.path());
                let (mut processor, collapsed) = opened(&diff_only(&pair, 2));
                let key = only_block(&collapsed);
                assert_eq!(
                    *collapsed.row_blocks,
                    vec![RowBlock {
                        key,
                        rows: key..key + 1,
                        expanded: false
                    }]
                );

                processor.set_block_expanded(key, true);
                // Polling and other state changes leave the expansion alone.
                processor.update();
                processor.conflict_cursor.set_max(2);
                processor.conflict_cursor.set(1);
                processor.get_scroll_to_row();
                let ctx = processor.get_minimal_diff_ctx().expect("diff still shown");

                assert!(collapsed_rows(&ctx.diff_rows).is_empty());
                let shown: Vec<_> = ctx.diff_rows[key..key + 10].iter().map(line_nums).collect();
                let expected: Vec<_> = (6..=15).map(|n| (n, n)).collect();
                assert_eq!(shown, expected);
                assert_eq!(
                    *ctx.row_blocks,
                    vec![RowBlock {
                        key,
                        rows: key..key + 10,
                        expanded: true
                    }]
                );
                assert_eq!(
                    format!("{:?}", &ctx.diff_rows[..key]),
                    format!("{:?}", &collapsed.diff_rows[..key])
                );
                assert_eq!(
                    format!("{:?}", &ctx.diff_rows[key + 10..]),
                    format!("{:?}", &collapsed.diff_rows[key + 1..])
                );
                let (_, full) = opened(&input(&pair.0, &pair.1));
                assert_eq!(
                    format!("{:?}", ctx.diff_rows),
                    format!("{:?}", full.diff_rows)
                );
            }

            #[test]
            fn re_collapsing_restores_the_collapsed_rows() {
                let dir = tempfile::tempdir().unwrap();
                let pair = gap_pair(dir.path());
                let (mut processor, collapsed) = opened(&diff_only(&pair, 2));
                let key = only_block(&collapsed);

                processor.set_block_expanded(key, true);
                processor.set_block_expanded(key, false);
                let ctx = processor.get_minimal_diff_ctx().expect("diff still shown");

                assert_eq!(
                    format!("{:?}", ctx.diff_rows),
                    format!("{:?}", collapsed.diff_rows)
                );
                assert_eq!(ctx.row_blocks, collapsed.row_blocks);
                assert_eq!(
                    format!("{:?}", ctx.precomputed_diffs),
                    format!("{:?}", collapsed.precomputed_diffs)
                );
                assert_eq!(ctx.precomputed_file_rows, collapsed.precomputed_file_rows);
            }

            #[test]
            fn a_row_rebuild_clears_the_expansion() {
                let dir = tempfile::tempdir().unwrap();
                let pair = gap_pair(dir.path());
                let (mut processor, collapsed) = opened(&diff_only(&pair, 2));
                processor.set_block_expanded(only_block(&collapsed), true);

                open(&mut processor, &diff_only(&pair, 3));
                let rebuilt = settle(&mut processor).expect("rebuild never completed");
                only_block(&rebuilt);
                assert!(rebuilt.row_blocks.iter().all(|block| !block.expanded));

                open(&mut processor, &diff_only(&pair, 2));
                let back = settle(&mut processor).expect("rebuild never completed");
                assert_eq!(
                    format!("{:?}", back.diff_rows),
                    format!("{:?}", collapsed.diff_rows)
                );
                assert_eq!(back.row_blocks, collapsed.row_blocks);
            }

            #[test]
            fn conflicts_find_goto_and_revert_land_on_the_right_rows_after_an_expansion() {
                let dir = tempfile::tempdir().unwrap();
                let pair = gap_pair(dir.path());
                let (mut processor, collapsed) = opened(&diff_only(&pair, 2));
                let key = only_block(&collapsed);
                let revert = RevertRequest {
                    hunk: 1,
                    target: RevertTarget::Left,
                };
                let planned = plan_hunk_revert(&collapsed, revert, std::path::Path::new(""))
                    .expect("revert planned");

                // The second conflict and a find hit below the block were navigated to.
                assert_eq!(collapsed.precomputed_diffs.len(), 2);
                processor.conflict_cursor.set_max(2);
                processor.conflict_cursor.set(2);
                processor.update_find(FindCtx::new("keep_19", &collapsed));
                assert!(processor.get_scroll_to_row().is_some());

                processor.set_block_expanded(key, true);
                assert_eq!(
                    processor.get_scroll_to_row(),
                    None,
                    "expanding must not scroll"
                );

                let ctx = processor.get_minimal_diff_ctx().expect("diff still shown");
                let rows = &ctx.diff_rows;
                let found = row_of_left_line(rows, 19);
                assert_eq!(processor.active_highlights, vec![found]);
                assert_eq!(processor.find_scroll_to_row().map(|s| s.start), Some(found));

                let conflict = processor.conflict_scroll_to_row().expect("conflict 2");
                let conflict_rows = conflict.start..=conflict.maybe_end.expect("a span");
                assert!(
                    rows[conflict_rows.clone()]
                        .iter()
                        .any(|row| line_nums(row).1 == 18),
                    "conflict 2 is rows {conflict_rows:?}"
                );
                assert_eq!(ctx.precomputed_diffs[1].rows(), conflict_rows);

                // Line 10 was hidden before the expansion.
                processor.active_side = ActiveSide::Left;
                processor.update_goto(Some(10));
                let goto = processor.get_scroll_to_row().expect("goto scrolls");
                assert_eq!(goto.start, row_of_left_line(rows, 10));
                assert_eq!(ctx.precomputed_file_rows.0[9], goto.start);

                assert_eq!(
                    plan_hunk_revert(&ctx, revert, std::path::Path::new("")),
                    Ok(planned)
                );
            }

            /// `gap_pair` where a function opens on line 8, inside the block of lines 6..=15, and
            /// holds the line 18 change. Its inner `if` closes before the change.
            fn scope_pair(dir: &Path) -> Pair {
                let write = |name: &str, edit: bool| {
                    let contents: String = (1..=20)
                        .map(|n| match n {
                            8 => "fn scoped() {\n".to_string(),
                            9 => "    if a {\n".to_string(),
                            11 => "    }\n".to_string(),
                            19 => "}\n".to_string(),
                            3 | 18 if edit => format!("    edit_{n}\n"),
                            _ => format!("    keep_{n}\n"),
                        })
                        .collect();
                    let path = dir.join(name);
                    std::fs::write(&path, contents).unwrap();
                    load(&UniversalPath::from(path))
                };
                (write("left.rs", false), write("right.rs", true))
            }

            #[test]
            fn expanding_to_scope_reveals_the_lines_from_the_scope_opening_to_the_block_bottom() {
                let dir = tempfile::tempdir().unwrap();
                let pair = scope_pair(dir.path());
                let (mut processor, collapsed) = opened(&diff_only(&pair, 2));
                let key = only_block(&collapsed);

                processor.expand_block_to_scope(key);
                let ctx = processor.get_minimal_diff_ctx().expect("diff still shown");

                // Lines 6 and 7 stay behind the Collapsed row.
                assert_eq!(collapsed_rows(&ctx.diff_rows), vec![key]);
                let shown: Vec<_> = ctx.diff_rows[key + 1..key + 9]
                    .iter()
                    .map(line_nums)
                    .collect();
                let expected: Vec<_> = (8..=15).map(|n| (n, n)).collect();
                assert_eq!(shown, expected);
                assert_eq!(
                    *ctx.row_blocks,
                    vec![RowBlock {
                        key,
                        rows: key..key + 9,
                        expanded: false
                    }]
                );
                assert_eq!(
                    format!("{:?}", &ctx.diff_rows[..=key]),
                    format!("{:?}", &collapsed.diff_rows[..=key])
                );
                assert_eq!(
                    format!("{:?}", &ctx.diff_rows[key + 9..]),
                    format!("{:?}", &collapsed.diff_rows[key + 1..])
                );
                assert_eq!(ctx.precomputed_file_rows.0[7], key + 1);
                assert_eq!(ctx.precomputed_file_rows.1[14], key + 8);
            }

            #[test]
            fn a_scope_expansion_expands_fully_and_re_collapses() {
                let dir = tempfile::tempdir().unwrap();
                let pair = scope_pair(dir.path());
                let (mut processor, collapsed) = opened(&diff_only(&pair, 2));
                let key = only_block(&collapsed);
                let (_, full) = opened(&input(&pair.0, &pair.1));

                processor.expand_block_to_scope(key);
                processor.set_block_expanded(key, true);
                let ctx = processor.get_minimal_diff_ctx().expect("diff still shown");
                assert_eq!(
                    format!("{:?}", ctx.diff_rows),
                    format!("{:?}", full.diff_rows)
                );

                processor.expand_block_to_scope(key);
                processor.set_block_expanded(key, false);
                let ctx = processor.get_minimal_diff_ctx().expect("diff still shown");
                assert_eq!(
                    format!("{:?}", ctx.diff_rows),
                    format!("{:?}", collapsed.diff_rows)
                );
                assert_eq!(ctx.row_blocks, collapsed.row_blocks);
            }

            #[test]
            fn expanding_to_scope_with_no_scope_in_the_block_expands_all_of_it() {
                let dir = tempfile::tempdir().unwrap();
                let pair = gap_pair(dir.path());
                let (mut processor, collapsed) = opened(&diff_only(&pair, 2));
                let key = only_block(&collapsed);
                let (_, full) = opened(&input(&pair.0, &pair.1));

                processor.expand_block_to_scope(key);
                let ctx = processor.get_minimal_diff_ctx().expect("diff still shown");
                assert_eq!(
                    format!("{:?}", ctx.diff_rows),
                    format!("{:?}", full.diff_rows)
                );
                assert!(ctx.row_blocks[0].expanded);
            }

            #[test]
            fn a_find_hit_inside_the_block_lands_on_its_line_once_expanded() {
                let dir = tempfile::tempdir().unwrap();
                let pair = gap_pair(dir.path());
                let (mut processor, collapsed) = opened(&diff_only(&pair, 2));
                // Hidden lines have no row of their own, so the hit sits on row 0.
                assert_eq!(collapsed.precomputed_file_rows.0[9], 0);
                processor.update_find(FindCtx::new("keep_10", &collapsed));
                processor.get_scroll_to_row();

                processor.set_block_expanded(only_block(&collapsed), true);
                assert_eq!(
                    processor.get_scroll_to_row(),
                    None,
                    "expanding must not scroll"
                );

                let ctx = processor.get_minimal_diff_ctx().expect("diff still shown");
                assert_eq!(
                    processor.find_scroll_to_row().map(|s| s.start),
                    Some(row_of_left_line(&ctx.diff_rows, 10))
                );
            }

            #[test]
            fn the_current_find_match_stays_current_when_an_expansion_reorders_the_hits() {
                let dir = tempfile::tempdir().unwrap();
                let pair = gap_pair(dir.path());
                let (mut processor, collapsed) = opened(&diff_only(&pair, 2));
                processor.update_find(FindCtx::new("keep_1", &collapsed));
                let is_left_12 = |hit: &FindHit| hit.side == ActiveSide::Left && hit.line == 11;
                let before = processor.find_ctx.hits().iter().position(is_left_12);
                processor
                    .find_cursor
                    .set(before.expect("left line 12 is a hit"));

                processor.set_block_expanded(only_block(&collapsed), true);

                let ctx = processor.get_minimal_diff_ctx().expect("diff still shown");
                let current = processor.current_find_hit().expect("a current hit");
                assert!(is_left_12(&current), "{current:?}");
                assert_eq!(current.row, row_of_left_line(&ctx.diff_rows, 12));
                assert_ne!(
                    processor.find_ctx.hits().iter().position(is_left_12),
                    before
                );
            }
        }

        mod find_and_goto {
            use super::*;
            use ActiveSide::{Left, Right};

            fn opened_pair(dir: &Path, left: &str, right: &str) -> (DiffProcessor, MinimalDiffCtx) {
                let write = |name: &str, contents: &str| {
                    let path = dir.join(name);
                    std::fs::write(&path, contents).unwrap();
                    load(&UniversalPath::from(path))
                };
                let mut processor = DiffProcessor::default();
                let pair = input(&write("left.rs", left), &write("right.rs", right));
                open(&mut processor, &pair);
                let ctx = settle(&mut processor).expect("diff never completed");
                (processor, ctx)
            }

            fn foo_pair(dir: &Path) -> (DiffProcessor, MinimalDiffCtx) {
                let opened = opened_pair(dir, "foo foo\nbar\nfoo\n", "foo foo\nbar foo\nfoo\n");
                let rows = &opened.1.precomputed_file_rows;
                assert_eq!(
                    (&rows.0[..3], &rows.1[..3]),
                    (&[0, 1, 2][..], &[0, 1, 2][..])
                );
                opened
            }

            fn hit(hit: Option<FindHit>) -> Option<(ActiveSide, usize, usize)> {
                hit.map(|h| (h.side, h.row, h.ordinal))
            }

            #[test]
            fn find_lists_every_match_on_both_sides_by_row_then_side() {
                let dir = tempfile::tempdir().unwrap();
                let (_, ctx) = foo_pair(dir.path());

                let find = FindCtx::new("foo", &ctx);

                let hits: Vec<_> = find.hits().iter().map(|&h| hit(Some(h)).unwrap()).collect();
                assert_eq!(
                    hits,
                    [
                        (Left, 0, 0),
                        (Left, 0, 1),
                        (Right, 0, 0),
                        (Right, 0, 1),
                        (Right, 1, 0),
                        (Left, 2, 0),
                        (Right, 2, 0),
                    ]
                );
                assert_eq!(find.needle(), "foo");
            }

            #[test]
            fn find_counts_overlapping_matches_like_the_row_highlight() {
                let dir = tempfile::tempdir().unwrap();
                let (_, ctx) = opened_pair(dir.path(), "aaa\n", "x\n");

                let find = FindCtx::new("aa", &ctx);

                let ordinals: Vec<_> = find.hits().iter().map(|h| h.ordinal).collect();
                assert_eq!(ordinals, [0, 1]);
            }

            #[test]
            fn a_find_without_matches_has_no_current_hit_and_does_not_scroll() {
                let dir = tempfile::tempdir().unwrap();
                let (mut processor, ctx) = foo_pair(dir.path());

                for needle in ["zzz", ""] {
                    processor.update_find(FindCtx::new(needle, &ctx));
                    assert!(processor.find_ctx.hits().is_empty(), "{needle:?}");
                    assert_eq!(processor.current_find_hit(), None, "{needle:?}");
                    assert_eq!(processor.find_scroll_to_row(), None, "{needle:?}");
                    assert_eq!(processor.get_scroll_to_row(), None, "{needle:?}");
                    assert!(processor.active_highlights.is_empty(), "{needle:?}");
                }
            }

            #[test]
            fn stepping_find_visits_each_match_and_lights_up_both_sides() {
                let dir = tempfile::tempdir().unwrap();
                let (mut processor, ctx) = foo_pair(dir.path());
                processor.update_find(FindCtx::new("foo", &ctx));

                assert_eq!(hit(processor.current_find_hit()), Some((Left, 0, 0)));
                assert_eq!(processor.get_scroll_to_row().map(|s| s.start), Some(0));
                assert_eq!(processor.highlight_side, None);

                processor.find_cursor.inc();
                processor.find_cursor.inc();
                assert_eq!(hit(processor.current_find_hit()), Some((Right, 0, 0)));
                processor.find_cursor.inc();
                processor.find_cursor.inc();
                assert_eq!(hit(processor.current_find_hit()), Some((Right, 1, 0)));
                assert_eq!(processor.get_scroll_to_row().map(|s| s.start), Some(1));
                assert_eq!(processor.active_highlights, [1]);
            }

            #[test]
            fn clear_results_drops_find_and_goto() {
                let dir = tempfile::tempdir().unwrap();
                let (mut processor, ctx) = foo_pair(dir.path());
                processor.update_find(FindCtx::new("bar", &ctx));
                assert_eq!(processor.get_scroll_to_row().map(|s| s.start), Some(1));

                processor.clear_results();
                assert!(processor.find_ctx.hits().is_empty());
                assert_eq!(processor.find_ctx.needle(), "");
                assert!(processor.active_highlights.is_empty());
                assert_eq!(processor.get_scroll_to_row(), None);

                processor.update_find(FindCtx::new("bar", &ctx));
                assert_eq!(processor.get_scroll_to_row().map(|s| s.start), Some(1));

                processor.active_side = Right;
                processor.update_goto(Some(2));
                processor.clear_results();
                assert_eq!(processor.get_scroll_to_row(), None);
                assert!(processor.active_highlights.is_empty());
                assert_eq!(processor.highlight_side, None);
            }

            #[test]
            fn goto_lights_up_only_its_side_until_the_next_navigation() {
                let dir = tempfile::tempdir().unwrap();
                let (mut processor, ctx) = foo_pair(dir.path());
                processor.active_side = Right;
                processor.update_goto(Some(2));

                assert_eq!(processor.get_scroll_to_row().map(|s| s.start), Some(1));
                assert_eq!(processor.active_highlights, [1]);
                assert_eq!(processor.highlight_side, Some(Right));

                assert_eq!(processor.get_scroll_to_row(), None);
                assert_eq!(processor.highlight_side, Some(Right));

                processor.update_find(FindCtx::new("bar", &ctx));
                assert_eq!(processor.get_scroll_to_row().map(|s| s.start), Some(1));
                assert_eq!(processor.highlight_side, None);
            }
        }
    }
}
