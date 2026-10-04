//! Whole-hunk revert. A hunk is a diff span from conflict navigation. Reverting it replaces the
//! target file's text for the hunk with the other file's text, as one edit on raw byte spans.

use std::{
    collections::HashSet,
    io::{self, Write},
    ops::Range,
    path::{Path, PathBuf},
};

use zcommon::hash::hash_file_mmap;
use zdiff::{
    cached_file::CachedFile,
    diff_builder::{DiffRow, LineContent},
    diff_ir::{DiffIR, DiffResult},
    lexer::RawToken,
};

use crate::diff_ctx::{DiffSpan, MinimalDiffCtx};

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
    PivotActive,
    NoSuchHunk,
}

impl std::fmt::Display for RevertRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            RevertRefusal::MissingFile => "both files must be loaded",
            RevertRefusal::NotLocalTarget => "the target is not a local file",
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

pub fn check_revert(ctx: &MinimalDiffCtx, target: RevertTarget) -> Result<(), RevertRefusal> {
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
) -> Result<PlannedRevert, RevertRefusal> {
    check_revert(ctx, request.target)?;
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
    Io(io::Error),
}

impl std::fmt::Display for WriteRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriteRefusal::TempTarget => f.write_str("the target is a temporary file"),
            WriteRefusal::Stale => f.write_str("the file changed on disk since it was loaded"),
            WriteRefusal::ReadOnly => f.write_str("the file is read-only"),
            WriteRefusal::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Replaces `path` with `contents`, but only if it is not under `temp_root`, still has the
/// bytes hashed as `loaded_hash`, and is writable. Depot targets can't get here: they have no
/// local path (see `check_revert`).
///
/// The contents go to a sibling temp file that is synced and then renamed over the target, so a
/// crash leaves either the old or the new file, never a truncated one. The temp file is deleted
/// on every failure.
pub fn write_guarded(
    path: &Path,
    contents: &[u8],
    loaded_hash: &str,
    temp_root: &Path,
) -> Result<(), WriteRefusal> {
    // Canonical paths, so a short (8.3) or differently cased temp dir still matches. Issue 05's
    // temp classifier replaces this check.
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if canonical(path).starts_with(canonical(temp_root)) {
        return Err(WriteRefusal::TempTarget);
    }
    let not_found_is_stale = |e: io::Error| match e.kind() {
        io::ErrorKind::NotFound => WriteRefusal::Stale,
        _ => WriteRefusal::Io(e),
    };
    // Same hash function as CachedFile, so an untouched file always matches.
    if hash_file_mmap(path).map_err(not_found_is_stale)? != loaded_hash {
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        path::Path,
        sync::Arc,
        time::{Duration, Instant},
    };

    use zdiff::{
        diff_builder::{DiffBuilderOptions, PivotLines},
        lexer::{LEXER_MODE_DEFAULT, LEXER_MODE_GREEDY, LEXER_MODE_NEWLINE, LEXER_MODE_TOKENIZE},
        universal_path::UniversalPath,
    };

    use super::*;
    use crate::diff_ctx::{DiffProcessor, UpdateDiffRowsInput};

    const SETTLE_TIMEOUT: Duration = Duration::from_secs(30);

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
            let planned = plan_hunk_revert(&self.diff(), RevertRequest { hunk, target })
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
            let planned =
                plan_hunk_revert(&pair.diff(), RevertRequest { hunk: 0, target }).unwrap();
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
            assert_eq!(check_revert(&ctx, target), Err(RevertRefusal::PivotActive));
            assert_eq!(
                plan_hunk_revert(&ctx, RevertRequest { hunk: 0, target }),
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

        assert_eq!(check_revert(&ctx, Left), Err(RevertRefusal::NotLocalTarget));
        assert_eq!(
            plan_hunk_revert(
                &ctx,
                RevertRequest {
                    hunk: 0,
                    target: Left
                }
            ),
            Err(RevertRefusal::NotLocalTarget)
        );
        assert_eq!(check_revert(&ctx, Right), Ok(()));
        assert_eq!(
            plan_hunk_revert(
                &ctx,
                RevertRequest {
                    hunk: 0,
                    target: Right
                }
            ),
            Ok(PlannedRevert {
                path: pair.right.clone(),
                contents: "a\nb\n".to_string(),
                loaded_hash: hash_file_mmap(&pair.right).unwrap(),
            })
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
            check_revert(&one_sided, Right),
            Err(RevertRefusal::MissingFile)
        );

        let ctx = pair.diff();
        assert_eq!(
            plan_hunk_revert(
                &ctx,
                RevertRequest {
                    hunk: 1,
                    target: Left
                }
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

            fn write(&self, contents: &[u8]) -> Result<(), WriteRefusal> {
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
    }
}
