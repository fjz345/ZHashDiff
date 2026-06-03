use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
};

use zcommon::hash::hash_contents;
#[cfg(debug_assertions)]
use zdiff::universal_path::UniversalPath;
use zdiff::{
    cached_file::CachedFile,
    diff_builder::{DiffBuilderOptions, DiffRow, LineContent, build_diff_rows},
    diff_ir::{DiffIR, DiffOp},
    lexer::RawToken,
    myers::{MyersDiffAlgorithm, myers_count_add_deletes, myers_diff_path},
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

pub type PrecomputedFileRows = (Vec<usize>, Vec<usize>); // line mapping from DiffRow index to DiffRow line number
pub type ScrollSpan = (usize, Option<usize>); // Span with optional end

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
    pub precomputed_file_rows: PrecomputedFileRows,
    // Myers
    pub diff_rows: Vec<DiffRow>,
    pub num_add_deletes: (u32, u32),

    pub update_diff_rows_input: UpdateDiffRowsInput,
}

fn default_result_channel() -> (mpsc::Sender<DiffCtx>, mpsc::Receiver<DiffCtx>) {
    mpsc::channel()
}

#[derive(Debug)]
pub struct DiffProcessor {
    result_channel: (Sender<DiffCtx>, Receiver<DiffCtx>),

    ctx: Option<DiffCtx>,
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
            result_channel: default_result_channel(),
            ctx: None,
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

#[derive(Debug, Clone, Default)]
pub struct FindCtx {
    _found_lines_1: Vec<usize>,
    _found_lines_2: Vec<usize>,
    cached_found_lines: Vec<usize>,
}
impl FindCtx {
    pub fn new(
        find_input: &str,
        file_1: Option<&CachedFile<RawToken>>,
        file_2: Option<&CachedFile<RawToken>>,
        precomputed_file_rows: &PrecomputedFileRows,
    ) -> Self {
        Self::create_find_ctx(find_input, file_1, file_2, precomputed_file_rows)
    }

    fn get_all_found_lines(found_lines_1: &Vec<usize>, found_lines_2: &Vec<usize>) -> Vec<usize> {
        let mut all_found_lines = found_lines_1.clone();
        all_found_lines.extend(found_lines_2.clone());
        all_found_lines.dedup();
        all_found_lines.sort();
        all_found_lines
    }

    fn create_find_ctx(
        find_input: &str,
        file_1: Option<&CachedFile<RawToken>>,
        file_2: Option<&CachedFile<RawToken>>,
        precomputed_file_rows: &PrecomputedFileRows,
    ) -> Self {
        let mut find_found_lines_1: Vec<usize> = Vec::new();
        let mut find_found_lines_2: Vec<usize> = Vec::new();

        if let Some(file) = file_1 {
            find_found_lines_1 = file
                .content_search(&find_input)
                .into_iter()
                .map(|f| precomputed_file_rows.0[f])
                .collect()
        }

        if let Some(file) = file_2 {
            find_found_lines_2 = file
                .content_search(&find_input)
                .into_iter()
                .map(|f| precomputed_file_rows.0[f])
                .collect()
        }
        log::debug!("Found (in #1): {:?}", find_found_lines_1);
        log::debug!("Found (in #2): {:?}", find_found_lines_2);

        let cached_found_lines =
            Self::get_all_found_lines(&find_found_lines_1, &find_found_lines_2);
        let find_ctx = Self {
            _found_lines_1: find_found_lines_1,
            _found_lines_2: find_found_lines_2,
            cached_found_lines,
        };
        log::debug!("create_find_ctx: {:?}", find_ctx);
        find_ctx
    }
}

impl DiffProcessor {
    pub fn reset_ctx(&mut self) {
        self.ctx = None;
    }

    pub fn is_in_progress(&self) -> bool {
        self.in_progress_input.is_some()
    }

    pub fn cancel_in_progress(&mut self) {
        if self.is_in_progress() {
            log::info!("Cancel flag set to true");
            self.cancel_flag.store(true, Ordering::Release);
        }
    }

    pub fn request_update(&mut self, input: UpdateDiffRowsInput) {
        if input.file_1.is_none() && input.file_2.is_none() {
            log::debug!("request_update called with no valid input");
            return;
        }

        if self.in_progress_input.as_ref() == Some(&input) {
            log::debug!("request_update called with already in_progress input, will be cancelled");
        }

        self.cancel_in_progress();
        self.cancel_flag = Arc::new(AtomicBool::new(false));
        let cancel_flag_copy = self.cancel_flag.clone();

        self.result_channel = mpsc::channel();
        let tx = self.result_channel.0.clone();

        let input_copy = input.clone();
        self.in_progress_input = Some(input);

        let builder = std::thread::Builder::new().name("DiffCtxTHREAD".into());
        let handle = builder.spawn(move || {
            log::info!(
                "Spawned thread for DiffCtx\nSource: {}, Target: {}",
                input_copy
                    .file_1
                    .as_ref()
                    .and_then(|f| Some(format!("{}", f.path)))
                    .unwrap_or_default(),
                input_copy
                    .file_2
                    .as_ref()
                    .and_then(|f| Some(format!("{}", f.path)))
                    .unwrap_or_default(),
            );
            let ctx = Self::compute_diff(input_copy, cancel_flag_copy);

            let send_result = match ctx {
                Some(ctx) => tx.send(ctx),
                None => {
                    log::info!("Diff computation was cancelled");
                    Ok(())
                }
            };
            match send_result {
                Ok(_) => {}
                Err(e) => log::error!("Failed to send diff result: {e}"),
            }
        });
        match handle {
            Ok(_handle) => {}
            Err(e) => {
                log::error!("Failed to spawn thread: {e}");
                self.in_progress_input = None;
            }
        }
    }

    pub fn poll_diff_channel(&mut self) {
        while let Ok(new_ctx) = self.result_channel.1.try_recv() {
            if self.in_progress_input.as_ref() == Some(&new_ctx.update_diff_rows_input) {
                self.ctx = Some(new_ctx);
                self.in_progress_input = None;
            }
        }
    }

    pub fn update_goto(&mut self, line_number: Option<usize>) {
        log::info!("Goto to line: {:?}", line_number);
        self.goto_line_number = line_number;
    }

    pub fn update_find(&mut self, find_ctx: FindCtx) {
        self.find_ctx = find_ctx;
        if self.find_ctx.cached_found_lines.len() > 0 {
            self.find_cursor
                .set_max(self.find_ctx.cached_found_lines.len().saturating_sub(1));
            self.find_cursor.set(0);
        }
    }

    pub fn get_scroll_to_row(&mut self) -> Option<ScrollSpan> {
        let conflict_scroll_to_row = self.conflict_scroll_to_row();
        let conflict_scroll_to_row = if conflict_scroll_to_row != self.last_conflict_scroll_to_row {
            self.last_conflict_scroll_to_row = conflict_scroll_to_row;
            conflict_scroll_to_row
        } else {
            None
        };
        let goto_scroll_to_rows = self
            .goto_line_number
            .and_then(|f| Some((f.saturating_sub(1), None)));
        let goto_scroll_to_rows = if goto_scroll_to_rows != self.last_goto_scroll_to_row {
            self.last_goto_scroll_to_row = goto_scroll_to_rows;
            goto_scroll_to_rows
        } else {
            None
        };
        let find_scroll_to_rows = self.find_scroll_to_row();
        let find_scroll_to_rows = if find_scroll_to_rows != self.last_find_scroll_to_row {
            self.last_find_scroll_to_row = find_scroll_to_rows;
            find_scroll_to_rows
        } else {
            None
        };

        let scroll_to_rows =
            find_scroll_to_rows.or_else(|| goto_scroll_to_rows.or_else(|| conflict_scroll_to_row));

        if let Some((start, maybe_end)) = &scroll_to_rows {
            self.active_highlights.clear();
            if let Some(end) = maybe_end {
                self.active_highlights.extend(*start..=*end);
            } else {
                self.active_highlights.push(*start);
            }
        }

        scroll_to_rows
    }

    pub fn conflict_scroll_to_row(&self) -> Option<ScrollSpan> {
        let mut ret = None;
        if let Some(diff_ctx) = self.get_diff_ctx() {
            if self.conflict_cursor.get() > 0 {
                let conflict_idx_span =
                    diff_ctx.precomputed_diffs[self.conflict_cursor.get().saturating_sub(1)];
                ret = Some((conflict_idx_span.0, Some(conflict_idx_span.1)));
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
        Some((find_idx_1.unwrap_or_default(), None))
    }

    pub fn get_diff_ctx(&self) -> Option<&DiffCtx> {
        self.ctx.as_ref()
    }

    fn compute_diff(input: UpdateDiffRowsInput, cancel_flag: Arc<AtomicBool>) -> Option<DiffCtx> {
        update_diff_rows(input, cancel_flag)
    }
}

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
    let found_diff_row_pivot_index_1 = precomputed_file_rows.0.get(pivot_lines.0.saturating_sub(1));
    let found_diff_row_pivot_index_2 = precomputed_file_rows.1.get(pivot_lines.1.saturating_sub(1));
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

fn update_diff_rows(input: UpdateDiffRowsInput, cancel_flag: Arc<AtomicBool>) -> Option<DiffCtx> {
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
                precompute_file_rows(&diff_rows, line_count_1, line_count_2);
            apply_pivot(&mut diff_rows, pivot_lines, &precomputed_file_rows);
        }
    }

    if cancel_flag.load(Ordering::Relaxed) {
        log::debug!("cancel_flag: pivot_lines");
        return None;
    }

    let mut precomputed_diffs = precompute_diff_spans(&diff_rows);

    if cancel_flag.load(Ordering::Relaxed) {
        log::debug!("cancel_flag: precomputed_diffs");
        return None;
    }

    let precomputed_file_rows = precompute_file_rows(&diff_rows, line_count_1, line_count_2);

    if cancel_flag.load(Ordering::Relaxed) {
        log::debug!("cancel_flag: precompute_file_rows");
        return None;
    }

    if let Some(diff_only_rows) = options.diff_only_with_extra_rows {
        let mut keep_indices = vec![false; diff_rows.len()];

        for &(start, end) in &precomputed_diffs {
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

        // recompute precomputed_diffs
        precomputed_diffs = precompute_diff_spans(&diff_rows);
    }

    if cancel_flag.load(Ordering::Relaxed) {
        log::debug!("cancel_flag: diff_only_with_extra_rows");
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
