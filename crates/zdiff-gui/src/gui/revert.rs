//! Whole-hunk revert. A hunk is a diff span from conflict navigation. Reverting it replaces the
//! target file's text for the hunk with the other file's text, as one edit on raw byte spans.

use std::{
    collections::HashSet,
    io::{self, Write},
    ops::Range,
    path::{Path, PathBuf},
};

use zcommon::hash::hash_contents;
use zdiff::{
    cached_file::CachedFile,
    diff_builder::{DiffRow, LineContent},
    diff_ir::{DiffIR, DiffResult},
    lexer::RawToken,
    universal_path::UniversalPath,
};

use crate::{
    diff_ctx::{DiffSpan, MinimalDiffCtx},
    p4::{P4Runner, escape_local_path},
};

/// The file a revert writes into. It receives the other file's text for the hunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevertTarget {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevertRequest {
    /// Index into the diff spans.
    pub hunk: usize,
    pub target: RevertTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevertRefusal {
    MissingFile,
    NotLocalTarget,
    TempTarget,
    PivotActive,
    NoSuchHunk,
}

impl std::fmt::Display for RevertRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RevertRefusal::MissingFile => "both files must be loaded",
            RevertRefusal::NotLocalTarget => "the target is not a local file",
            RevertRefusal::TempTarget => "the target is a temporary file",
            RevertRefusal::PivotActive => {
                "pivot lines are set, so rows don't pair the files by the diff"
            }
            RevertRefusal::NoSuchHunk => "the hunk is not in the current diff",
        })
    }
}

/// Byte range of a hunk in each file. An empty range is an insertion point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HunkBytes {
    pub left: Range<usize>,
    pub right: Range<usize>,
}

/// New target contents: `target` with `target_range` replaced by `other[other_range]`.
pub fn plan_revert(
    target: &str,
    target_range: Range<usize>,
    other: &str,
    other_range: Range<usize>,
) -> String {
    let other = &other[other_range];
    let mut new = String::with_capacity(target.len() - target_range.len() + other.len());
    new.push_str(&target[..target_range.start]);
    new.push_str(other);
    new.push_str(&target[target_range.end..]);
    new
}

/// Byte ranges of the hunk drawn on `hunk_rows`.
///
/// Rows are a lossy view of the alignment: ghost rows draw one file's tokens on the other
/// side, and diff-only mode drops unchanged rows, so a neighbouring row can't place a pure
/// insert. The IR is the alignment itself: the hunk is the IR range from its first to its last
/// entry, and each file's range is that file's tokens inside it. Tokens tile their file, so an
/// empty range still sits exactly where the files align.
pub fn hunk_bytes(
    ir: &DiffIR,
    hunk_rows: &[DiffRow],
    left: &CachedFile<RawToken>,
    right: &CachedFile<RawToken>,
) -> Option<HunkBytes> {
    let mut left_idx = HashSet::new();
    let mut right_idx = HashSet::new();
    for side in hunk_rows.iter().flat_map(|row| [&row.left, &row.right]) {
        if let LineContent::Code { tokens, .. } = side {
            for (res, _, _) in tokens {
                left_idx.extend(res.token_source_idx);
                right_idx.extend(res.token_target_idx);
            }
        }
    }
    let in_hunk = |e: &DiffResult| {
        e.token_source_idx.is_some_and(|i| left_idx.contains(&i))
            || e.token_target_idx.is_some_and(|i| right_idx.contains(&i))
    };
    let first = ir.entries.iter().position(in_hunk)?;
    let last = ir.entries.iter().rposition(in_hunk)?;

    // Each file's tokens appear in the IR once and in order, so the number of them before an
    // IR position is the index of the next one.
    let count = |entries: &[DiffResult]| {
        let left = entries
            .iter()
            .filter(|e| e.token_source_idx.is_some())
            .count();
        let right = entries
            .iter()
            .filter(|e| e.token_target_idx.is_some())
            .count();
        (left, right)
    };
    let (left_start, right_start) = count(&ir.entries[..first]);
    let (left_len, right_len) = count(&ir.entries[first..=last]);
    let byte = |file: &CachedFile<RawToken>, token: usize| {
        file.tokens
            .get(token)
            .map_or(file.contents.len(), |t| t.span.start)
    };
    Some(HunkBytes {
        left: byte(left, left_start)..byte(left, left_start + left_len),
        right: byte(right, right_start)..byte(right, right_start + right_len),
    })
}

/// Index of the hunk whose first row is `row`. Revert buttons sit on that row only.
pub fn hunk_starting_at(spans: &[DiffSpan], row: usize) -> Option<usize> {
    spans
        .binary_search_by_key(&row, |span| *span.rows().start())
        .ok()
}

/// A target inside `temp_root` is a throwaway copy (e.g. one p4 made for an external diff).
pub fn check_revert(
    ctx: &MinimalDiffCtx,
    target: RevertTarget,
    temp_root: &Path,
) -> Result<(), RevertRefusal> {
    let (Some(left), Some(right)) = (&ctx.input.file_1, &ctx.input.file_2) else {
        return Err(RevertRefusal::MissingFile);
    };
    let target_file = match target {
        RevertTarget::Left => left,
        RevertTarget::Right => right,
    };
    if target_file.path.as_path().is_none() {
        return Err(RevertRefusal::NotLocalTarget);
    }
    if target_file.path.is_temp(temp_root) {
        return Err(RevertRefusal::TempTarget);
    }
    // Same condition as the pivot alignment in the rows stage. Pivoted rows shift one side,
    // so a span pairs unrelated parts of the files.
    if matches!(ctx.input.options.pivot_lines, Some(p) if p.left > 0 && p.right > 0) {
        return Err(RevertRefusal::PivotActive);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRevert {
    pub path: PathBuf,
    pub contents: String,
    /// Hash of the target bytes the plan was made from, for the stale check.
    pub loaded_hash: String,
}

/// The path to write and its new contents. Plans against the files the rows were built from.
pub fn plan_hunk_revert(
    ctx: &MinimalDiffCtx,
    request: RevertRequest,
    temp_root: &Path,
) -> Result<PlannedRevert, RevertRefusal> {
    check_revert(ctx, request.target, temp_root)?;
    let (Some(left), Some(right)) = (&ctx.input.file_1, &ctx.input.file_2) else {
        unreachable!("check_revert requires both files");
    };
    let span = ctx
        .precomputed_diffs
        .get(request.hunk)
        .ok_or(RevertRefusal::NoSuchHunk)?;
    let hunk = hunk_bytes(&ctx.diff_ir, &ctx.diff_rows[span.rows()], left, right)
        .ok_or(RevertRefusal::NoSuchHunk)?;
    let (target, target_range, other, other_range) = match request.target {
        RevertTarget::Left => (left, hunk.left, right, hunk.right),
        RevertTarget::Right => (right, hunk.right, left, hunk.left),
    };
    let path = target
        .path
        .as_path()
        .expect("check_revert requires a local target")
        .to_path_buf();
    let contents = plan_revert(&target.contents, target_range, &other.contents, other_range);
    Ok(PlannedRevert {
        path,
        contents,
        loaded_hash: target.hash.clone(),
    })
}

/// Why a guarded write left the target untouched.
#[derive(Debug)]
pub enum WriteRefusal {
    TempTarget,
    /// The bytes on disk are not the ones the revert was planned from.
    Stale,
    ReadOnly,
    /// Read-only, and p4 doesn't manage it (or couldn't be asked), so `p4 edit` can't help.
    ReadOnlyNotP4Managed,
    /// `p4 edit` failed or left the file read-only. Holds p4's output.
    P4Edit(String),
    Io(io::Error),
}

impl std::fmt::Display for WriteRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteRefusal::TempTarget => f.write_str("the target is a temporary file"),
            WriteRefusal::Stale => f.write_str("the file changed on disk since it was loaded"),
            WriteRefusal::ReadOnly => f.write_str("the file is read-only"),
            WriteRefusal::ReadOnlyNotP4Managed => {
                f.write_str("the file is read-only and not Perforce-managed")
            }
            WriteRefusal::P4Edit(e) => write!(f, "p4 edit failed: {}", e.trim()),
            WriteRefusal::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Replaces `path` with `contents`, but only if it is not under `temp_root`, still has the
/// bytes hashed as `loaded_hash`, and is writable. Depot targets can't get here: they have no
/// local path (see `check_revert`). Returns the bytes it replaced.
///
/// The contents go to a sibling temp file that is synced and then renamed over the target, so a
/// crash leaves either the old or the new file, never a truncated one. The temp file is deleted
/// on every failure.
pub fn write_guarded(
    path: &Path,
    contents: &[u8],
    loaded_hash: &str,
    temp_root: &Path,
) -> Result<Vec<u8>, WriteRefusal> {
    // Canonical paths, so a short (8.3) name of the temp dir still matches. The buttons only
    // ask the lexical classifier; this is the check that can't be fooled by an alias.
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if UniversalPath::from(canonical(path)).is_temp(&canonical(temp_root)) {
        return Err(WriteRefusal::TempTarget);
    }
    let not_found_is_stale = |e: io::Error| match e.kind() {
        io::ErrorKind::NotFound => WriteRefusal::Stale,
        _ => WriteRefusal::Io(e),
    };
    // Same hash function as CachedFile, so an untouched file always matches. The hashed bytes
    // are the ones undo restores, read once so the check and the record can't disagree.
    let before = std::fs::read(path).map_err(not_found_is_stale)?;
    if hash_contents(&before) != loaded_hash {
        return Err(WriteRefusal::Stale);
    }
    let permissions = std::fs::metadata(path)
        .map_err(not_found_is_stale)?
        .permissions();
    if permissions.readonly() {
        return Err(WriteRefusal::ReadOnly);
    }

    let dir = path.parent().unwrap_or(Path::new("."));
    let mut temp = tempfile::Builder::new()
        .prefix(".zdiff-revert-")
        .tempfile_in(dir)
        .map_err(WriteRefusal::Io)?;
    temp.write_all(contents).map_err(WriteRefusal::Io)?;
    temp.as_file().sync_all().map_err(WriteRefusal::Io)?;
    // The temp file is created with restrictive permissions; keep the target's.
    std::fs::set_permissions(temp.path(), permissions).map_err(WriteRefusal::Io)?;
    temp.persist(path).map_err(|e| WriteRefusal::Io(e.error))?;
    Ok(before)
}

/// A written revert: the target's bytes before and after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertRecord {
    pub path: PathBuf,
    pub before: Vec<u8>,
    pub after: Vec<u8>,
}

impl RevertRecord {
    fn new(planned: PlannedRevert, before: Vec<u8>) -> Self {
        Self {
            path: planned.path,
            before,
            after: planned.contents.into_bytes(),
        }
    }
}

#[derive(Debug)]
pub enum RevertWrite {
    Written(RevertRecord),
    /// The target is read-only and Perforce-managed: ask the user before `p4 edit`.
    NeedsP4Edit(PendingP4Edit),
}

/// A revert waiting for the user to allow `p4 edit` on its target. Dropping it declines.
#[derive(Debug)]
pub struct PendingP4Edit {
    planned: PlannedRevert,
}

impl PendingP4Edit {
    pub fn path(&self) -> &Path {
        &self.planned.path
    }

    /// Runs `p4 edit` on the target, then writes. A failed edit writes nothing.
    pub fn confirm(
        self,
        temp_root: &Path,
        p4: &impl P4Runner,
    ) -> Result<RevertRecord, WriteRefusal> {
        let planned = self.planned;
        let edited = p4
            .run(&["edit", &p4_file_arg(&planned.path)])
            .map_err(WriteRefusal::P4Edit)?;
        // The exit code alone isn't trusted: an edit that left the file read-only failed. The
        // guarded write also catches changes made while the prompt was open.
        match write_guarded(
            &planned.path,
            planned.contents.as_bytes(),
            &planned.loaded_hash,
            temp_root,
        ) {
            Ok(before) => Ok(RevertRecord::new(planned, before)),
            Err(WriteRefusal::ReadOnly) => Err(WriteRefusal::P4Edit(edited)),
            Err(e) => Err(e),
        }
    }
}

fn p4_file_arg(path: &Path) -> String {
    escape_local_path(&path.to_string_lossy())
}

/// Asks p4 whether it tracks `path`. An error answer (not in the client view, no server, no p4)
/// counts as not managed and is logged.
fn is_p4_managed(path: &Path, p4: &impl P4Runner) -> bool {
    match p4.run(&["fstat", &p4_file_arg(path)]) {
        Ok(out) => out.lines().any(|l| l.starts_with("... depotFile ")),
        Err(e) => {
            log::warn!("p4 fstat {}: {}", path.display(), e.trim());
            false
        }
    }
}

/// Guarded write of a planned revert. A read-only target that p4 manages asks for `p4 edit`
/// instead of being refused.
pub fn write_revert(
    planned: PlannedRevert,
    temp_root: &Path,
    p4: &impl P4Runner,
) -> Result<RevertWrite, WriteRefusal> {
    match write_guarded(
        &planned.path,
        planned.contents.as_bytes(),
        &planned.loaded_hash,
        temp_root,
    ) {
        Ok(before) => Ok(RevertWrite::Written(RevertRecord::new(planned, before))),
        Err(WriteRefusal::ReadOnly) if is_p4_managed(&planned.path, p4) => {
            Ok(RevertWrite::NeedsP4Edit(PendingP4Edit { planned }))
        }
        Err(WriteRefusal::ReadOnly) => Err(WriteRefusal::ReadOnlyNotP4Managed),
        Err(e) => Err(e),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryStep {
    Undo,
    Redo,
}

/// Undo and redo of the reverts made in one diff. Reverts into either file share one stack, in
/// the order they were made, and it is cleared when either side's path changes. Not persisted.
#[derive(Debug, Default)]
pub struct RevertHistory {
    pair: Option<(UniversalPath, UniversalPath)>,
    undo: Vec<RevertRecord>,
    redo: Vec<RevertRecord>,
}

impl RevertHistory {
    /// Clears the history when the diff now shows another file on either side.
    pub fn observe_pair(&mut self, left: &UniversalPath, right: &UniversalPath) {
        let same_pair = self
            .pair
            .as_ref()
            .is_some_and(|(l, r)| l == left && r == right);
        if !same_pair {
            *self = Self {
                pair: Some((left.clone(), right.clone())),
                ..Default::default()
            };
        }
    }

    /// A new revert can't be redone over, so it clears the redo stack.
    pub fn record(&mut self, record: RevertRecord) {
        self.redo.clear();
        self.undo.push(record);
    }

    /// Writes the latest undo (or redo) through `write(path, contents, expected)`, where
    /// `expected` is what the file must still hold. The record moves to the other stack only
    /// when the write succeeds, so a refused step stays and can be retried. `None` when there
    /// is nothing to step.
    pub fn step<E>(
        &mut self,
        step: HistoryStep,
        write: impl FnOnce(&Path, &[u8], &[u8]) -> Result<(), E>,
    ) -> Option<(PathBuf, Result<(), E>)> {
        let (from, to) = match step {
            HistoryStep::Undo => (&mut self.undo, &mut self.redo),
            HistoryStep::Redo => (&mut self.redo, &mut self.undo),
        };
        let record = from.last()?;
        let (contents, expected) = match step {
            HistoryStep::Undo => (&record.before, &record.after),
            HistoryStep::Redo => (&record.after, &record.before),
        };
        let result = write(&record.path, contents, expected);
        let path = record.path.clone();
        if result.is_ok() {
            to.extend(from.pop());
        }
        Some((path, result))
    }
}

/// The write for `RevertHistory::step`: a guarded write that requires the file to still hold
/// exactly `expected`.
pub fn write_history_step(
    temp_root: &Path,
) -> impl FnOnce(&Path, &[u8], &[u8]) -> Result<(), WriteRefusal> + '_ {
    move |path: &Path, contents: &[u8], expected: &[u8]| {
        write_guarded(path, contents, &hash_contents(expected), temp_root).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::Arc,
        time::{Duration, Instant},
    };

    use zcommon::hash::hash_file_mmap;
    use zdiff::{
        diff_builder::{DiffBuilderOptions, PivotLines},
        lexer::{LEXER_MODE_DEFAULT, LEXER_MODE_GREEDY, LEXER_MODE_NEWLINE, LEXER_MODE_TOKENIZE},
        universal_path::UniversalPath,
    };

    use super::*;
    use crate::diff_ctx::{DiffProcessor, UpdateDiffRowsInput};

    const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

    /// The fixtures are tempfiles in the OS temp dir; an empty root makes none of them temp.
    fn no_temp_root() -> &'static Path {
        Path::new("")
    }

    fn cached(
        display: UniversalPath,
        physical: &Path,
        lexer_mode: u8,
    ) -> Arc<CachedFile<RawToken>> {
        Arc::new(CachedFile::new(display, physical, lexer_mode).unwrap())
    }

    fn diff_files(
        left: Arc<CachedFile<RawToken>>,
        right: Arc<CachedFile<RawToken>>,
        options: DiffBuilderOptions,
    ) -> MinimalDiffCtx {
        let mut processor = DiffProcessor::default();
        processor.request_update(UpdateDiffRowsInput {
            file_1: Some(left),
            file_2: Some(right),
            options,
            ..Default::default()
        });
        let start = Instant::now();
        loop {
            processor.update();
            if !processor.is_in_progress() {
                return processor.get_minimal_diff_ctx().expect("diff failed");
            }
            assert!(start.elapsed() < SETTLE_TIMEOUT, "diff never completed");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// A pair of files on disk, diffed like the app does.
    struct Pair {
        _dir: tempfile::TempDir,
        left: PathBuf,
        right: PathBuf,
        options: DiffBuilderOptions,
        lexer_mode: u8,
    }

    impl Pair {
        fn new(left: &str, right: &str) -> Self {
            Self::with_options(left, right, DiffBuilderOptions::default())
        }

        fn with_options(left: &str, right: &str, options: DiffBuilderOptions) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let (l, r) = (dir.path().join("left.txt"), dir.path().join("right.txt"));
            std::fs::write(&l, left).unwrap();
            std::fs::write(&r, right).unwrap();
            Self {
                _dir: dir,
                left: l,
                right: r,
                options,
                lexer_mode: LEXER_MODE_DEFAULT,
            }
        }

        fn diff(&self) -> MinimalDiffCtx {
            diff_files(
                cached(
                    UniversalPath::from(self.left.clone()),
                    &self.left,
                    self.lexer_mode,
                ),
                cached(
                    UniversalPath::from(self.right.clone()),
                    &self.right,
                    self.lexer_mode,
                ),
                self.options.clone(),
            )
        }

        fn path(&self, target: RevertTarget) -> &Path {
            match target {
                RevertTarget::Left => &self.left,
                RevertTarget::Right => &self.right,
            }
        }

        /// Plans the revert of `hunk` and checks it targets the right file. Nothing is written.
        fn plan(&self, hunk: usize, target: RevertTarget) -> String {
            let planned =
                plan_hunk_revert(&self.diff(), RevertRequest { hunk, target }, no_temp_root())
                    .expect("revert refused");
            assert_eq!(planned.path, self.path(target));
            planned.contents
        }

        fn read(&self, target: RevertTarget) -> String {
            std::fs::read_to_string(self.path(target)).unwrap()
        }
    }

    use RevertTarget::{Left, Right};

    #[test]
    fn replace_hunk_takes_the_other_files_lines() {
        let pair = Pair::new("a\nb\nc\n", "a\nB\nc\n");
        assert_eq!(pair.diff().precomputed_diffs.len(), 1);
        assert_eq!(pair.plan(0, Right), "a\nb\nc\n");
        assert_eq!(pair.plan(0, Left), "a\nB\nc\n");
    }

    #[test]
    fn replace_hunk_inside_a_line_reverts_the_whole_change() {
        let pair = Pair::new("let x = foo(1, 2);\n", "let x = bar(1, 3);\n");
        assert_eq!(pair.plan(0, Right), "let x = foo(1, 2);\n");
        assert_eq!(pair.plan(0, Left), "let x = bar(1, 3);\n");
    }

    #[test]
    fn insert_hunk_is_inserted_into_the_target_and_deleted_the_other_way() {
        let pair = Pair::new("a\nc\n", "a\nb1\nb2\nc\n");
        assert_eq!(pair.plan(0, Left), "a\nb1\nb2\nc\n");
        assert_eq!(pair.plan(0, Right), "a\nc\n");
    }

    #[test]
    fn delete_hunk_is_removed_from_the_target_and_restored_the_other_way() {
        let pair = Pair::new("a\nb\nc\n", "a\nc\n");
        assert_eq!(pair.plan(0, Left), "a\nc\n");
        assert_eq!(pair.plan(0, Right), "a\nb\nc\n");
    }

    #[test]
    fn hunk_at_the_start_of_the_file() {
        let pair = Pair::new("x\ny\na\nb\n", "a\nb\n");
        assert_eq!(pair.plan(0, Left), "a\nb\n");
        assert_eq!(pair.plan(0, Right), "x\ny\na\nb\n");
    }

    #[test]
    fn hunk_at_the_end_of_the_file_without_a_trailing_newline() {
        let pair = Pair::new("a\nb\nc", "a\nb\nd");
        assert_eq!(pair.plan(0, Left), "a\nb\nd");
        assert_eq!(pair.plan(0, Right), "a\nb\nc");

        let pair = Pair::new("a\nb", "a\nb\nc\nd");
        assert_eq!(pair.plan(0, Left), "a\nb\nc\nd");
        assert_eq!(pair.plan(0, Right), "a\nb");
    }

    #[test]
    fn crlf_outside_and_inside_the_hunk_is_preserved() {
        let pair = Pair::new("a\r\nb\r\nc\r\n", "a\r\nX\r\nY\r\nc\r\n");
        assert_eq!(pair.plan(0, Right), "a\r\nb\r\nc\r\n");
        assert_eq!(pair.plan(0, Left), "a\r\nX\r\nY\r\nc\r\n");
    }

    #[test]
    fn empty_target_file() {
        let pair = Pair::new("", "a\nb\n");
        assert_eq!(pair.plan(0, Left), "a\nb\n");
        assert_eq!(pair.plan(0, Right), "");
    }

    #[test]
    fn only_the_hunk_changes() {
        let left = "keep 1\nold A\nkeep 2\nkeep 3\nold B\r\nkeep 4\n";
        let right = "keep 1\nnew A\nkeep 2\nkeep 3\nnew B\r\nkeep 4\n";
        let pair = Pair::new(left, right);
        assert_eq!(pair.diff().precomputed_diffs.len(), 2);
        assert_eq!(
            pair.plan(1, Right),
            "keep 1\nnew A\nkeep 2\nkeep 3\nold B\r\nkeep 4\n"
        );
        assert_eq!(
            pair.plan(0, Left),
            "keep 1\nnew A\nkeep 2\nkeep 3\nold B\r\nkeep 4\n"
        );
    }

    /// Replaces the cached-file token revert test, same inputs: the hunk is the whole changed
    /// line, so each side becomes the other.
    #[test]
    fn define_line_hunk_reverts_in_both_directions() {
        let s1 = "\t#define hello_there\n\t// Comment\n";
        let s2 = "\t#define world_here\n\t// Comment\n";
        let pair = Pair::new(s1, s2);
        assert_eq!(pair.plan(0, Left), s2);
        assert_eq!(pair.plan(0, Right), s1);
    }

    /// Diff-only with no context rows drops the unchanged rows without a placeholder, so no
    /// drawn row tells where a pure insert sits in the file that lacks it.
    #[test]
    fn insert_after_dropped_rows_in_diff_only_mode_lands_where_the_files_align() {
        let options = DiffBuilderOptions {
            diff_only_with_extra_rows: Some(0),
            ..Default::default()
        };
        let pair = Pair::with_options(
            "same 1\nsame 2\nsame 3\n",
            "same 1\nsame 2\nadded\nsame 3\n",
            options,
        );
        let ctx = pair.diff();
        assert_eq!(ctx.precomputed_diffs.len(), 1);
        assert_eq!(*ctx.precomputed_diffs[0].rows().start(), 0);
        assert_eq!(pair.plan(0, Left), "same 1\nsame 2\nadded\nsame 3\n");
        assert_eq!(pair.plan(0, Right), "same 1\nsame 2\nsame 3\n");
    }

    /// Writes reverts of the first hunk until no hunk is left. Each round must remove at least
    /// one hunk, and the target must end byte-identical to the other file.
    fn revert_until_equal(
        left: &str,
        right: &str,
        target: RevertTarget,
        options: DiffBuilderOptions,
        lexer_mode: u8,
    ) {
        let pair = Pair {
            lexer_mode,
            ..Pair::with_options(left, right, options)
        };
        let other = match target {
            Left => right,
            Right => left,
        };
        let mut hunks = pair.diff().precomputed_diffs.len();
        assert!(hunks > 0);
        while hunks > 0 {
            let planned = plan_hunk_revert(
                &pair.diff(),
                RevertRequest { hunk: 0, target },
                no_temp_root(),
            )
            .unwrap();
            std::fs::write(planned.path, planned.contents).unwrap();
            let after = pair.diff().precomputed_diffs.len();
            assert!(
                after < hunks,
                "{hunks} hunks before the revert, {after} after: {:?}",
                pair.read(target)
            );
            hunks = after;
        }
        assert_eq!(pair.read(target), other);
    }

    #[test]
    fn reverting_every_hunk_makes_the_target_equal_the_other_file() {
        let left = "first\r\nshared 1\r\nfn a() { 1 }\r\nshared 2\r\ngone 1\r\ngone 2\r\nshared 3\r\n\ttail x";
        let right = "shared 1\r\nfn a() { 2 }\r\nfn b() {}\r\nshared 2\r\nshared 3\r\n\tadded\r\n\ttail y\n";
        // Byte ranges come from token spans, so every lexer mode must tile the file.
        for lexer_mode in [LEXER_MODE_GREEDY, LEXER_MODE_TOKENIZE, LEXER_MODE_NEWLINE] {
            for ghost_rows in [true, false] {
                for diff_only in [None, Some(0), Some(2)] {
                    let options = DiffBuilderOptions {
                        ghost_rows,
                        diff_only_with_extra_rows: diff_only,
                        ..Default::default()
                    };
                    revert_until_equal(left, right, Left, options.clone(), lexer_mode);
                    revert_until_equal(left, right, Right, options, lexer_mode);
                }
            }
        }
    }

    #[test]
    fn multi_line_replace_right_before_a_pure_insert_with_ghost_rows() {
        let left = "a\nold 1\nold 2\nb\nc\n";
        let right = "a\nnew 1 x\nnew 2 y\nnew 3\nb\ninserted\nc\n";
        let options = DiffBuilderOptions::default();
        revert_until_equal(left, right, Left, options.clone(), LEXER_MODE_DEFAULT);
        revert_until_equal(left, right, Right, options, LEXER_MODE_DEFAULT);
    }

    #[test]
    fn active_pivot_refuses_and_plans_nothing() {
        let options = DiffBuilderOptions {
            pivot_lines: Some(PivotLines { left: 2, right: 3 }),
            ..Default::default()
        };
        let pair = Pair::with_options("a\nb\nc\n", "x\na\nb\nC\n", options);
        let ctx = pair.diff();
        assert!(!ctx.precomputed_diffs.is_empty());
        for target in [Left, Right] {
            assert_eq!(
                check_revert(&ctx, target, no_temp_root()),
                Err(RevertRefusal::PivotActive)
            );
            assert_eq!(
                plan_hunk_revert(&ctx, RevertRequest { hunk: 0, target }, no_temp_root()),
                Err(RevertRefusal::PivotActive)
            );
        }
    }

    #[test]
    fn depot_target_is_refused_and_the_local_side_still_reverts() {
        let pair = Pair::new("a\nb\n", "a\nB\n");
        let depot = cached(
            UniversalPath::new("//depot/main/left.txt#3"),
            &pair.left,
            LEXER_MODE_DEFAULT,
        );
        let local = cached(
            UniversalPath::from(pair.right.clone()),
            &pair.right,
            LEXER_MODE_DEFAULT,
        );
        let ctx = diff_files(depot, local, DiffBuilderOptions::default());

        assert_eq!(
            check_revert(&ctx, Left, no_temp_root()),
            Err(RevertRefusal::NotLocalTarget)
        );
        assert_eq!(
            plan_hunk_revert(
                &ctx,
                RevertRequest {
                    hunk: 0,
                    target: Left
                },
                no_temp_root()
            ),
            Err(RevertRefusal::NotLocalTarget)
        );
        assert_eq!(check_revert(&ctx, Right, no_temp_root()), Ok(()));
        assert_eq!(
            plan_hunk_revert(
                &ctx,
                RevertRequest {
                    hunk: 0,
                    target: Right
                },
                no_temp_root()
            ),
            Ok(PlannedRevert {
                path: pair.right.clone(),
                contents: "a\nb\n".to_string(),
                loaded_hash: hash_file_mmap(&pair.right).unwrap(),
            })
        );
    }

    #[test]
    fn temp_target_is_refused_and_the_other_side_still_reverts() {
        let temp_root = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let temp_copy = temp_root.path().join("p4v").join("file#3.txt");
        std::fs::create_dir_all(temp_copy.parent().unwrap()).unwrap();
        std::fs::write(&temp_copy, "a\nb\n").unwrap();
        let local = workspace.path().join("file.txt");
        std::fs::write(&local, "a\nB\n").unwrap();
        let ctx = diff_files(
            cached(
                UniversalPath::from(temp_copy.clone()),
                &temp_copy,
                LEXER_MODE_DEFAULT,
            ),
            cached(
                UniversalPath::from(local.clone()),
                &local,
                LEXER_MODE_DEFAULT,
            ),
            DiffBuilderOptions::default(),
        );
        let request = |target| RevertRequest { hunk: 0, target };

        assert_eq!(
            check_revert(&ctx, Left, temp_root.path()),
            Err(RevertRefusal::TempTarget)
        );
        assert_eq!(
            plan_hunk_revert(&ctx, request(Left), temp_root.path()),
            Err(RevertRefusal::TempTarget)
        );
        assert_eq!(check_revert(&ctx, Right, temp_root.path()), Ok(()));
        assert_eq!(
            plan_hunk_revert(&ctx, request(Right), temp_root.path())
                .unwrap()
                .contents,
            "a\nb\n"
        );
        // Swapped sides: the temp copy is the right file now.
        let swapped = diff_files(
            cached(
                UniversalPath::from(local.clone()),
                &local,
                LEXER_MODE_DEFAULT,
            ),
            cached(
                UniversalPath::from(temp_copy.clone()),
                &temp_copy,
                LEXER_MODE_DEFAULT,
            ),
            DiffBuilderOptions::default(),
        );
        assert_eq!(check_revert(&swapped, Left, temp_root.path()), Ok(()));
        assert_eq!(
            check_revert(&swapped, Right, temp_root.path()),
            Err(RevertRefusal::TempTarget)
        );
    }

    #[test]
    fn missing_side_or_hunk_is_refused() {
        let pair = Pair::new("a\nb\n", "a\nB\n");
        let left = cached(
            UniversalPath::from(pair.left.clone()),
            &pair.left,
            LEXER_MODE_DEFAULT,
        );
        let mut processor = DiffProcessor::default();
        processor.request_update(UpdateDiffRowsInput {
            file_1: Some(left),
            ..Default::default()
        });
        let start = Instant::now();
        while processor.is_in_progress() {
            assert!(start.elapsed() < SETTLE_TIMEOUT);
            std::thread::sleep(Duration::from_millis(1));
            processor.update();
        }
        let one_sided = processor.get_minimal_diff_ctx().unwrap();
        assert_eq!(
            check_revert(&one_sided, Right, no_temp_root()),
            Err(RevertRefusal::MissingFile)
        );

        let ctx = pair.diff();
        assert_eq!(
            plan_hunk_revert(
                &ctx,
                RevertRequest {
                    hunk: 1,
                    target: Left
                },
                no_temp_root()
            ),
            Err(RevertRefusal::NoSuchHunk)
        );
    }

    #[test]
    fn buttons_row_is_the_first_row_of_each_hunk() {
        let pair = Pair::new(
            "s\nold 1\nold 2\ns\ns\nold 3\ns\n",
            "s\nnew 1\nnew 2\ns\ns\nnew 3\ns\n",
        );
        let ctx = pair.diff();
        let starts: Vec<usize> = ctx
            .precomputed_diffs
            .iter()
            .map(|s| *s.rows().start())
            .collect();
        assert_eq!(starts.len(), 2);
        for row in 0..ctx.diff_rows.len() {
            let expected = starts.iter().position(|&s| s == row);
            assert_eq!(
                hunk_starting_at(&ctx.precomputed_diffs, row),
                expected,
                "row {row}"
            );
        }
        assert!(ctx.precomputed_diffs[0].rows().count() > 1);
    }

    mod guarded_write {
        use super::*;

        /// A target file in its own directory, and a temp root that doesn't contain it.
        struct Target {
            dir: tempfile::TempDir,
            temp_root: tempfile::TempDir,
            path: PathBuf,
            loaded_hash: String,
        }

        impl Target {
            fn new(contents: &[u8]) -> Self {
                let dir = tempfile::tempdir().unwrap();
                let path = dir.path().join("target.txt");
                std::fs::write(&path, contents).unwrap();
                let loaded_hash = hash_file_mmap(&path).unwrap();
                Self {
                    dir,
                    temp_root: tempfile::tempdir().unwrap(),
                    path,
                    loaded_hash,
                }
            }

            fn write(&self, contents: &[u8]) -> Result<Vec<u8>, WriteRefusal> {
                write_guarded(
                    &self.path,
                    contents,
                    &self.loaded_hash,
                    self.temp_root.path(),
                )
            }

            fn read(&self) -> Vec<u8> {
                std::fs::read(&self.path).unwrap()
            }

            /// The target is the only file in its directory: no sibling temp file was left.
            fn assert_no_leftovers(&self) {
                let entries: Vec<_> = std::fs::read_dir(self.dir.path())
                    .unwrap()
                    .map(|e| e.unwrap().file_name())
                    .collect();
                assert_eq!(entries, ["target.txt"]);
            }

            fn set_readonly(&self, readonly: bool) {
                let mut permissions = std::fs::metadata(&self.path).unwrap().permissions();
                permissions.set_readonly(readonly);
                std::fs::set_permissions(&self.path, permissions).unwrap();
            }
        }

        #[test]
        fn successful_write_replaces_the_contents_exactly() {
            let target = Target::new(b"old\r\nline\n");
            let new = "new \u{e5}\u{e4}\u{f6}\r\nline\n\u{1F600}".as_bytes();
            target.write(new).unwrap();
            assert_eq!(target.read(), new);
            target.assert_no_leftovers();
        }

        #[test]
        fn target_inside_the_temp_root_is_refused_and_the_file_is_untouched() {
            let target = Target::new(b"loaded\n");
            let dir = target.dir.path();
            let roots = [
                dir.to_path_buf(),
                std::fs::canonicalize(dir).unwrap(),
                PathBuf::from(dir.to_string_lossy().replace('\\', "/").to_uppercase()),
            ];
            for root in roots {
                let result = write_guarded(&target.path, b"reverted\n", &target.loaded_hash, &root);
                assert!(
                    matches!(result, Err(WriteRefusal::TempTarget)),
                    "{root:?}: {result:?}"
                );
                assert_eq!(target.read(), b"loaded\n");
                target.assert_no_leftovers();
            }
        }

        #[test]
        fn stale_hash_is_refused_and_the_file_is_untouched() {
            let target = Target::new(b"loaded\n");
            std::fs::write(&target.path, b"edited elsewhere\n").unwrap();
            let result = target.write(b"reverted\n");
            assert!(matches!(result, Err(WriteRefusal::Stale)), "{result:?}");
            assert_eq!(target.read(), b"edited elsewhere\n");
            target.assert_no_leftovers();
        }

        #[test]
        fn read_only_file_is_refused_and_untouched() {
            let target = Target::new(b"loaded\n");
            target.set_readonly(true);
            let result = target.write(b"reverted\n");
            target.set_readonly(false);
            assert!(matches!(result, Err(WriteRefusal::ReadOnly)), "{result:?}");
            assert_eq!(target.read(), b"loaded\n");
            target.assert_no_leftovers();
        }

        /// The replace itself fails after the temp file exists: another handle holds the target
        /// open without delete sharing.
        #[cfg(windows)]
        #[test]
        fn failed_replace_leaves_no_temp_file_and_the_file_untouched() {
            use std::os::windows::fs::OpenOptionsExt;

            const FILE_SHARE_READ: u32 = 1;
            let target = Target::new(b"loaded\n");
            let holder = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ)
                .open(&target.path)
                .unwrap();
            let result = target.write(b"reverted\n");
            drop(holder);
            assert!(matches!(result, Err(WriteRefusal::Io(_))), "{result:?}");
            assert_eq!(target.read(), b"loaded\n");
            target.assert_no_leftovers();
        }

        mod p4_edit {
            use super::*;
            use crate::p4::FakeP4;

            const FSTAT_MANAGED: &str = "... depotFile //depot/main/target.txt\n\
                ... clientFile C:\\ws\\target.txt\n... headRev 3\n... haveRev 3\n";

            fn set_readonly(path: &Path, readonly: bool) {
                let mut permissions = std::fs::metadata(path).unwrap().permissions();
                permissions.set_readonly(readonly);
                std::fs::set_permissions(path, permissions).unwrap();
            }

            impl Target {
                fn planned(&self, contents: &str) -> PlannedRevert {
                    PlannedRevert {
                        path: self.path.clone(),
                        contents: contents.to_string(),
                        loaded_hash: self.loaded_hash.clone(),
                    }
                }

                fn p4_path(&self) -> String {
                    self.path.to_string_lossy().into_owned()
                }

                fn revert(&self, p4: &impl P4Runner) -> Result<RevertWrite, WriteRefusal> {
                    write_revert(self.planned("reverted\n"), self.temp_root.path(), p4)
                }
            }

            fn pending(result: Result<RevertWrite, WriteRefusal>) -> PendingP4Edit {
                match result {
                    Ok(RevertWrite::NeedsP4Edit(pending)) => pending,
                    Ok(RevertWrite::Written(_)) => panic!("written without p4 edit"),
                    Err(e) => panic!("refused: {e:?}"),
                }
            }

            /// Answers fstat as managed and edit with `edit`, after running `on_edit`.
            fn managed(
                on_edit: impl Fn(),
                edit: Result<&'static str, &'static str>,
            ) -> FakeP4<impl Fn(&[&str]) -> Result<String, String>> {
                FakeP4::new(move |args: &[&str]| match args[0] {
                    "fstat" => Ok(FSTAT_MANAGED.to_string()),
                    "edit" => {
                        on_edit();
                        edit.map(str::to_string).map_err(str::to_string)
                    }
                    other => panic!("unexpected p4 {other}"),
                })
            }

            #[test]
            fn writable_target_is_written_without_asking_p4() {
                let target = Target::new(b"loaded\n");
                let p4 = FakeP4::new(|_: &[&str]| panic!("p4 must not run"));
                let result = target.revert(&p4);
                assert!(matches!(result, Ok(RevertWrite::Written(_))), "{result:?}");
                assert_eq!(target.read(), b"reverted\n");
                assert!(p4.calls().is_empty());
            }

            #[test]
            fn managed_read_only_target_asks_for_p4_edit_and_writes_nothing() {
                let target = Target::new(b"loaded\n");
                target.set_readonly(true);
                let p4 = managed(|| panic!("edit before the prompt"), Ok(""));
                let result = target.revert(&p4);
                target.set_readonly(false);

                let pending = pending(result);
                assert_eq!(pending.path(), target.path);
                assert_eq!(target.read(), b"loaded\n");
                assert_eq!(p4.calls(), [["fstat".to_string(), target.p4_path()]]);
                target.assert_no_leftovers();
            }

            /// Managed is p4's answer for this path, not a guess from the path.
            #[test]
            fn read_only_target_p4_does_not_manage_is_refused() {
                let target = Target::new(b"loaded\n");
                target.set_readonly(true);
                let not_in_view: Vec<Box<dyn Fn() -> Result<String, String>>> = vec![
                    Box::new(|| Err("target.txt - file(s) not in client view.\n".to_string())),
                    // Some p4 warnings exit 0 with no tagged output.
                    Box::new(|| Ok(String::new())),
                ];
                let mut results = Vec::new();
                for answer in not_in_view {
                    let p4 = FakeP4::new(move |args: &[&str]| {
                        assert_eq!(args[0], "fstat");
                        answer()
                    });
                    results.push((target.revert(&p4), p4.calls()));
                }
                target.set_readonly(false);

                for (result, calls) in results {
                    assert!(
                        matches!(result, Err(WriteRefusal::ReadOnlyNotP4Managed)),
                        "{result:?}"
                    );
                    assert_eq!(calls, [["fstat".to_string(), target.p4_path()]]);
                }
                assert_eq!(target.read(), b"loaded\n");
                target.assert_no_leftovers();
            }

            #[test]
            fn confirmed_edit_checks_out_then_writes() {
                let target = Target::new(b"loaded\r\n");
                target.set_readonly(true);
                // Like p4 edit, a successful edit makes the file writable.
                let path = target.path.clone();
                let p4 = managed(
                    move || set_readonly(&path, false),
                    Ok("//depot/main/target.txt#3 - opened for edit\n"),
                );
                let pending = pending(target.revert(&p4));
                let result = pending.confirm(target.temp_root.path(), &p4);
                target.set_readonly(false);

                // A revert through p4 edit is undoable like any other.
                assert_eq!(
                    result.unwrap(),
                    RevertRecord {
                        path: target.path.clone(),
                        before: b"loaded\r\n".to_vec(),
                        after: b"reverted\n".to_vec(),
                    }
                );
                assert_eq!(target.read(), b"reverted\n");
                assert_eq!(
                    p4.calls(),
                    [
                        ["fstat".to_string(), target.p4_path()],
                        ["edit".to_string(), target.p4_path()]
                    ]
                );
                target.assert_no_leftovers();
            }

            #[test]
            fn failed_edit_writes_nothing_and_surfaces_the_p4_error() {
                let target = Target::new(b"loaded\n");
                target.set_readonly(true);
                let p4 = managed(
                    || {},
                    Err("//depot/main/target.txt - can't edit exclusive file already opened\n"),
                );
                let pending = pending(target.revert(&p4));
                let result = pending.confirm(target.temp_root.path(), &p4);
                target.set_readonly(false);

                let Err(refusal @ WriteRefusal::P4Edit(_)) = result else {
                    panic!("{result:?}");
                };
                assert!(
                    refusal
                        .to_string()
                        .contains("can't edit exclusive file already opened"),
                    "{refusal}"
                );
                assert_eq!(target.read(), b"loaded\n");
                target.assert_no_leftovers();
            }

            /// p4 reported success but the file is still read-only: report p4's output, not a
            /// bare read-only refusal.
            #[test]
            fn edit_that_leaves_the_file_read_only_is_a_failed_edit() {
                let target = Target::new(b"loaded\n");
                target.set_readonly(true);
                let p4 = managed(|| {}, Ok("target.txt - file(s) not on client.\n"));
                let pending = pending(target.revert(&p4));
                let result = pending.confirm(target.temp_root.path(), &p4);
                target.set_readonly(false);

                let Err(refusal @ WriteRefusal::P4Edit(_)) = result else {
                    panic!("{result:?}");
                };
                assert!(refusal.to_string().contains("not on client"), "{refusal}");
                assert_eq!(target.read(), b"loaded\n");
                target.assert_no_leftovers();
            }

            #[test]
            fn declined_prompt_writes_nothing_and_runs_no_edit() {
                let target = Target::new(b"loaded\n");
                target.set_readonly(true);
                let p4 = managed(|| panic!("edit after decline"), Ok(""));
                let pending = pending(target.revert(&p4));
                drop(pending);
                target.set_readonly(false);

                assert_eq!(target.read(), b"loaded\n");
                assert_eq!(p4.calls(), [["fstat".to_string(), target.p4_path()]]);
                target.assert_no_leftovers();
            }
        }

        mod undo {
            use super::*;
            use crate::p4::FakeP4;
            use HistoryStep::{Redo, Undo};

            /// Writes a revert to `contents` like the app does, planned from the bytes on disk
            /// (the app reloads after every write), and records it.
            fn revert(target: &Target, history: &mut RevertHistory, contents: &str) {
                let planned = PlannedRevert {
                    path: target.path.clone(),
                    contents: contents.to_string(),
                    loaded_hash: hash_file_mmap(&target.path).unwrap(),
                };
                let p4 = FakeP4::new(|_: &[&str]| panic!("p4 must not run"));
                match write_revert(planned, target.temp_root.path(), &p4) {
                    Ok(RevertWrite::Written(record)) => history.record(record),
                    other => panic!("{other:?}"),
                }
            }

            fn step(
                target: &Target,
                history: &mut RevertHistory,
                step: HistoryStep,
            ) -> Result<(), WriteRefusal> {
                let (path, result) = history
                    .step(step, write_history_step(target.temp_root.path()))
                    .expect("nothing to step");
                assert_eq!(path, target.path);
                result
            }

            #[test]
            fn undo_restores_the_exact_bytes_and_redo_reapplies() {
                // BOM, CRLF, non-ASCII, emoji and no final newline.
                let original: &[u8] =
                    b"\xEF\xBB\xBFfirst \xC3\xA5\xC3\xA4\r\nsecond \xF0\x9F\x98\x80\r\nlast";
                let target = Target::new(original);
                let mut history = RevertHistory::default();
                revert(&target, &mut history, "one\r\n");
                revert(&target, &mut history, "two\n");

                step(&target, &mut history, Undo).unwrap();
                assert_eq!(target.read(), b"one\r\n");
                step(&target, &mut history, Undo).unwrap();
                assert_eq!(target.read(), original);
                step(&target, &mut history, Redo).unwrap();
                assert_eq!(target.read(), b"one\r\n");
                step(&target, &mut history, Redo).unwrap();
                assert_eq!(target.read(), b"two\n");
                target.assert_no_leftovers();
            }

            #[test]
            fn undo_is_refused_after_an_external_change() {
                let target = Target::new(b"loaded\n");
                let mut history = RevertHistory::default();
                revert(&target, &mut history, "reverted\n");
                std::fs::write(&target.path, b"edited elsewhere\n").unwrap();

                let result = step(&target, &mut history, Undo);
                assert!(matches!(result, Err(WriteRefusal::Stale)), "{result:?}");
                assert_eq!(target.read(), b"edited elsewhere\n");
                target.assert_no_leftovers();
            }

            #[test]
            fn redo_is_refused_after_an_external_change() {
                let target = Target::new(b"loaded\n");
                let mut history = RevertHistory::default();
                revert(&target, &mut history, "reverted\n");
                step(&target, &mut history, Undo).unwrap();
                std::fs::write(&target.path, b"edited elsewhere\n").unwrap();

                let result = step(&target, &mut history, Redo);
                assert!(matches!(result, Err(WriteRefusal::Stale)), "{result:?}");
                assert_eq!(target.read(), b"edited elsewhere\n");
                target.assert_no_leftovers();
            }
        }
    }

    mod history {
        use super::*;
        use HistoryStep::{Redo, Undo};

        fn record(path: &str, before: &str, after: &str) -> RevertRecord {
            RevertRecord {
                path: PathBuf::from(path),
                before: before.into(),
                after: after.into(),
            }
        }

        /// A write: (path, contents written, contents the file had to hold).
        fn write(path: &str, contents: &str, expected: &str) -> Option<(PathBuf, String, String)> {
            Some((PathBuf::from(path), contents.into(), expected.into()))
        }

        /// Steps with a writer that succeeds and returns what it was asked to write.
        fn step(
            history: &mut RevertHistory,
            step: HistoryStep,
        ) -> Option<(PathBuf, String, String)> {
            let mut written = None;
            let result = history.step(step, |path, contents, expected| {
                let text = |b: &[u8]| String::from_utf8(b.to_vec()).unwrap();
                written = Some((path.to_path_buf(), text(contents), text(expected)));
                Ok::<(), ()>(())
            });
            match result {
                None => assert!(written.is_none()),
                Some((path, result)) => {
                    assert_eq!(result, Ok(()));
                    assert_eq!(Some(&path), written.as_ref().map(|w| &w.0));
                }
            }
            written
        }

        #[test]
        fn undo_and_redo_walk_the_reverts_in_order() {
            let mut history = RevertHistory::default();
            history.record(record("a", "a0", "a1"));
            history.record(record("b", "b0", "b1"));
            history.record(record("a", "a1", "a2"));

            assert_eq!(step(&mut history, Undo), write("a", "a1", "a2"));
            assert_eq!(step(&mut history, Undo), write("b", "b0", "b1"));
            assert_eq!(step(&mut history, Undo), write("a", "a0", "a1"));
            assert_eq!(step(&mut history, Undo), None);

            assert_eq!(step(&mut history, Redo), write("a", "a1", "a0"));
            assert_eq!(step(&mut history, Redo), write("b", "b1", "b0"));
            assert_eq!(step(&mut history, Redo), write("a", "a2", "a1"));
            assert_eq!(step(&mut history, Redo), None);

            assert_eq!(step(&mut history, Undo), write("a", "a1", "a2"));
        }

        #[test]
        fn a_new_revert_clears_redo() {
            let mut history = RevertHistory::default();
            history.record(record("a", "a0", "a1"));
            history.record(record("a", "a1", "a2"));
            assert_eq!(step(&mut history, Undo), write("a", "a1", "a2"));

            history.record(record("a", "a1", "x"));
            assert_eq!(step(&mut history, Redo), None);
            assert_eq!(step(&mut history, Undo), write("a", "a1", "x"));
            assert_eq!(step(&mut history, Undo), write("a", "a0", "a1"));
            assert_eq!(step(&mut history, Undo), None);
        }

        #[test]
        fn a_refused_step_keeps_the_record_for_a_retry() {
            let mut history = RevertHistory::default();
            history.record(record("a", "a0", "a1"));

            let refused = history.step(Undo, |_, _, _| Err("changed"));
            assert_eq!(refused, Some((PathBuf::from("a"), Err("changed"))));
            assert_eq!(step(&mut history, Redo), None);
            assert_eq!(step(&mut history, Undo), write("a", "a0", "a1"));

            let refused = history.step(Redo, |_, _, _| Err("changed"));
            assert_eq!(refused, Some((PathBuf::from("a"), Err("changed"))));
            assert_eq!(step(&mut history, Undo), None);
            assert_eq!(step(&mut history, Redo), write("a", "a1", "a0"));
        }

        #[test]
        fn a_path_change_on_either_side_clears_both_stacks() {
            let a = UniversalPath::from(PathBuf::from("C:/ws/a.txt"));
            let b = UniversalPath::from(PathBuf::from("C:/ws/b.txt"));
            let c = UniversalPath::new("//depot/main/c.txt#2");
            let with_undo_and_redo = || {
                let mut history = RevertHistory::default();
                history.observe_pair(&a, &b);
                history.record(record("a", "a0", "a1"));
                history.record(record("a", "a1", "a2"));
                assert_eq!(step(&mut history, Undo), write("a", "a1", "a2"));
                history
            };

            // The same pair, every frame, keeps both stacks.
            let mut history = with_undo_and_redo();
            history.observe_pair(&a, &b);
            history.observe_pair(&a, &b);
            assert_eq!(step(&mut history, Redo), write("a", "a2", "a1"));
            assert_eq!(step(&mut history, Undo), write("a", "a1", "a2"));
            assert_eq!(step(&mut history, Undo), write("a", "a0", "a1"));

            // Another file on either side, or the sides swapped, is a new diff.
            for (left, right) in [(&a, &c), (&c, &b), (&b, &a)] {
                let mut history = with_undo_and_redo();
                history.observe_pair(left, right);
                assert_eq!(step(&mut history, Undo), None);
                assert_eq!(step(&mut history, Redo), None);
            }
        }
    }
}
