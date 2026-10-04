use std::ops::Range;

use rayon::prelude::*;

use crate::{
    diff_ir::{DiffIR, DiffOp, DiffResult, diff_ir_hide_ignored},
    ignore::{IgnoreMask, IgnoreOptions},
    lexer::{RawTokenTrait, TokenKind},
};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Color32(pub [u8; 4]);

impl Color32 {
    pub const TRANSPARENT: Self = Self([0, 0, 0, 0]);
    pub const WHITE: Self = Self([255, 255, 255, 255]);
    pub const BLACK: Self = Self([0, 0, 0, 255]);
    pub const GRAY: Self = Self([128, 128, 128, 255]);
}

impl From<[u8; 4]> for Color32 {
    fn from(arr: [u8; 4]) -> Self {
        Self(arr)
    }
}

#[derive(Debug, Clone)]
pub struct DiffRow {
    pub left: LineContent,
    pub right: LineContent,
}

pub type IsGhost = bool;
#[derive(Debug, Clone)]
pub enum LineContent {
    Code {
        tokens: Vec<(DiffResult, Color32, IsGhost)>,
        line_num: i32,
        bg: Color32,
    },
    Void,
    Collapsed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct PivotLines {
    pub left: usize,
    pub right: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiffBuilderOptions {
    #[cfg_attr(feature = "serde", serde(flatten))]
    pub ignore: IgnoreOptions,
    pub highlight_rows: bool,
    pub ghost_rows: bool,
    pub keyword_highlight: bool,
    pub pivot_lines: Option<PivotLines>,
    pub diff_only_with_extra_rows: Option<usize>,
}
impl Default for DiffBuilderOptions {
    fn default() -> Self {
        Self {
            ignore: IgnoreOptions::default(),
            highlight_rows: true,
            ghost_rows: true,
            keyword_highlight: true,
            pivot_lines: None,
            diff_only_with_extra_rows: None,
        }
    }
}

impl DiffBuilderOptions {
    pub fn need_invalidation(old: &Self, new: &Self) -> bool {
        let mut ret = old.ghost_rows != new.ghost_rows
            || old.highlight_rows != new.highlight_rows
            || old.ignore != new.ignore
            || old.keyword_highlight != new.keyword_highlight;
        if !ret {
            ret = matches!(new.pivot_lines, Some(PivotLines{left: p1, right: p2 }) if p1 > 0 && p2 > 0)
                && old.pivot_lines != new.pivot_lines;
        }

        ret
    }
}

/// Color of tokens an ignore pattern matched. Public so the GUI can find them among the row colors
/// and paint them over its syntax highlighting.
pub const DIMMED: Color32 = Color32([128, 128, 128, 110]);

struct DiffTheme {
    ghost: Color32,
    kw: Color32,
    del: Color32,
    ins: Color32,
    del_bg: Color32,
    ins_bg: Color32,
    dimmed: Color32,
}

impl Default for DiffTheme {
    fn default() -> Self {
        Self {
            ghost: [150, 150, 150, 80].into(),
            kw: [86, 156, 214, 255].into(),
            del: [255, 100, 100, 255].into(),
            ins: [100, 255, 100, 255].into(),
            del_bg: [255, 0, 0, 20].into(),
            ins_bg: [0, 255, 0, 20].into(),
            dimmed: DIMMED,
        }
    }
}

struct SideState {
    buf: Vec<(DiffResult, Color32, IsGhost)>,
    line_num: i32,
    active_diff: bool,
}

impl SideState {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            buf: Vec::with_capacity(capacity),
            line_num: 1,
            active_diff: false,
        }
    }

    fn push(&mut self, val: DiffResult, color: Color32, is_ghost: IsGhost) {
        self.buf.push((val, color, is_ghost));
    }

    fn has_real_tokens(&self) -> bool {
        self.buf.iter().any(|(_, _, is_ghost)| !is_ghost)
    }

    fn flush(&mut self, line_num: i32, bg_color: Color32) -> LineContent {
        if self.buf.is_empty() {
            LineContent::Void
        } else {
            let tokens = self.buf.drain(..).collect();
            LineContent::Code {
                tokens,
                line_num,
                bg: bg_color,
            }
        }
    }
}

pub struct DiffBuilder<'a, 'b, T: RawTokenTrait> {
    tokens_source: Option<&'a [T]>,
    tokens_target: Option<&'a [T]>,
    options: &'b DiffBuilderOptions,
    theme: DiffTheme,
    rows: Vec<DiffRow>,
    /// Per token of (source, target), true when an ignore pattern matched it. Empty: none.
    dimmed: (&'a [bool], &'a [bool]),
    left: SideState,
    right: SideState,
}

impl<'a, 'b, T: RawTokenTrait> DiffBuilder<'a, 'b, T> {
    pub fn with_capacity(
        tokens_source: Option<&'a [T]>,
        tokens_target: Option<&'a [T]>,
        options: &'b DiffBuilderOptions,
        capacity: usize,
    ) -> Self {
        Self {
            tokens_source,
            tokens_target,
            options,
            theme: DiffTheme::default(),
            rows: Vec::with_capacity(capacity),
            dimmed: (&[], &[]),
            left: SideState::with_capacity(64),
            right: SideState::with_capacity(64),
        }
    }

    pub fn new(
        tokens_source: Option<&'a [T]>,
        tokens_target: Option<&'a [T]>,
        options: &'b DiffBuilderOptions,
    ) -> Self {
        let num_tokens =
            tokens_source.map_or(0, |s| s.len()) + tokens_target.map_or(0, |t| t.len());
        let capacity = num_tokens / 10;
        Self::with_capacity(tokens_source, tokens_target, options, capacity)
    }

    fn get_color(&self, is_keyword: bool) -> Color32 {
        if self.options.keyword_highlight && is_keyword {
            self.theme.kw
        } else {
            Color32::GRAY
        }
    }

    /// `color`, or the dimmed color when an ignore pattern matched the token.
    fn dim(&self, dimmed: &[bool], token_idx: Option<u32>, color: Color32) -> Color32 {
        match token_idx {
            Some(i) if dimmed.get(i as usize).copied().unwrap_or(false) => self.theme.dimmed,
            _ => color,
        }
    }

    pub fn handle_match(&mut self, diff_result: DiffResult) {
        assert!(matches!(diff_result.operation, DiffOp::Equal(_)));

        let token_idx = diff_result
            .token_source_idx
            .expect("Equal op must have source index");
        let token = &self.tokens_source.expect("Source was None")[token_idx as usize];

        let color = self.get_color(token.as_ref().kind.is_keyword());
        let is_newline = token.as_ref().kind == TokenKind::Newline;
        // A pattern match depends on the line around the token, so each side has its own flag.
        let left_color = self.dim(self.dimmed.0, diff_result.token_source_idx, color);
        let right_color = self.dim(self.dimmed.1, diff_result.token_target_idx, color);

        self.left.push(diff_result.clone(), left_color, false);
        self.right.push(diff_result, right_color, false);

        if is_newline {
            self.emit_row(true, true, true, true);
        }
    }

    pub fn handle_diff(&mut self, diff_result: DiffResult, is_deletion: bool) {
        assert!(matches!(
            diff_result.operation,
            DiffOp::Delete | DiffOp::Insert
        ));

        let token = if is_deletion {
            let idx = diff_result
                .token_source_idx
                .expect("Delete must have source index");
            &self.tokens_source.expect("Source is None")[idx as usize]
        } else {
            let idx = diff_result
                .token_target_idx
                .expect("Insert must have target index");
            &self.tokens_target.expect("Target is none")[idx as usize]
        };

        let is_newline = token.as_ref().kind == TokenKind::Newline;

        let color = if is_deletion {
            self.dim(self.dimmed.0, diff_result.token_source_idx, self.theme.del)
        } else {
            self.dim(self.dimmed.1, diff_result.token_target_idx, self.theme.ins)
        };

        let side = if is_deletion {
            &mut self.left
        } else {
            &mut self.right
        };
        if !diff_result.hide_in_diff {
            side.active_diff = true;
        }

        side.push(diff_result.clone(), color, false);

        if self.options.ghost_rows {
            self.apply_ghosts(is_deletion, diff_result);
        }

        if is_newline {
            // The ghost of this newline must not end the other side's real line early: the other
            // side is flushed only when it holds nothing but ghosts, which then form a ghost-only row.
            let other = if is_deletion { &self.right } else { &self.left };
            let flush_other = self.options.ghost_rows && !other.has_real_tokens();
            self.emit_row(
                is_deletion || flush_other,
                !is_deletion || flush_other,
                is_deletion,
                !is_deletion,
            );
        }
    }

    fn emit_row(&mut self, flush_left: bool, flush_right: bool, inc_left: bool, inc_right: bool) {
        let left_num = if inc_left {
            let n = self.left.line_num;
            self.left.line_num += 1;
            n
        } else {
            -1
        };

        let right_num = if inc_right {
            let n = self.right.line_num;
            self.right.line_num += 1;
            n
        } else {
            -1
        };

        let left = if flush_left {
            let active = self.left.active_diff && self.options.highlight_rows;
            let color = if active {
                self.theme.del_bg
            } else {
                Color32::TRANSPARENT
            };
            let content = self.left.flush(left_num, color);
            self.left.active_diff = false;
            content
        } else {
            LineContent::Void
        };

        let right = if flush_right {
            let active = self.right.active_diff && self.options.highlight_rows;
            let color = if active {
                self.theme.ins_bg
            } else {
                Color32::TRANSPARENT
            };
            let content = self.right.flush(right_num, color);
            self.right.active_diff = false;
            content
        } else {
            LineContent::Void
        };

        self.rows.push(DiffRow { left, right });
    }

    pub fn finish(self) -> Vec<DiffRow> {
        self.finish_counted().0
    }

    /// The rows, and how many lines of (source, target) they numbered.
    fn finish_counted(mut self) -> (Vec<DiffRow>, i32, i32) {
        if !self.left.buf.is_empty() || !self.right.buf.is_empty() {
            let inc_l = self
                .left
                .buf
                .iter()
                .any(|(r, _, _)| r.operation != DiffOp::Insert);
            let inc_r = self
                .right
                .buf
                .iter()
                .any(|(r, _, _)| r.operation != DiffOp::Delete);
            self.emit_row(true, true, inc_l, inc_r);
        }
        (self.rows, self.left.line_num - 1, self.right.line_num - 1)
    }

    fn apply_ghosts(&mut self, last_was_deletion: bool, result: DiffResult) {
        let ghost_color = self.theme.ghost;
        if last_was_deletion {
            self.right.push(result, ghost_color, true);
        } else {
            self.left.push(result, ghost_color, true);
        }
    }
}

/// `text_*` is the text a side's token spans index into; ignore patterns are matched on it.
pub fn build_diff_rows<'a, T: RawTokenTrait>(
    mut diff_ir: DiffIR,
    tokens_source: Option<&'a [T]>,
    tokens_target: Option<&'a [T]>,
    text_source: &str,
    text_target: &str,
    options: &DiffBuilderOptions,
    estimated_num_rows: usize,
) -> Vec<DiffRow> {
    let ignore = options.ignore.mask(
        tokens_source.unwrap_or_default(),
        text_source,
        tokens_target.unwrap_or_default(),
        text_target,
    );
    diff_ir = diff_ir_hide_ignored(diff_ir, &ignore);

    // Rows are built in independent chunks in parallel. The stage is dominated by allocating and
    // first touching each row's token Vec, which scales to about 8 threads on the Windows heap
    // and not much further; below MIN_CHUNK_ENTRIES per chunk the split costs more than it saves.
    const MAX_CHUNKS: usize = 8;
    const MIN_CHUNK_ENTRIES: usize = 16 * 1024;
    let num_chunks = (diff_ir.entries.len() / MIN_CHUNK_ENTRIES).clamp(1, MAX_CHUNKS);
    build_diff_rows_chunked(
        &diff_ir.entries,
        tokens_source,
        tokens_target,
        &ignore,
        options,
        estimated_num_rows,
        num_chunks,
    )
}

/// Rows of `entries` (ignored entries already hidden), built in at most `num_chunks` chunks.
/// The rows don't depend on the chunk count.
fn build_diff_rows_chunked<T: RawTokenTrait>(
    entries: &[DiffResult],
    tokens_source: Option<&[T]>,
    tokens_target: Option<&[T]>,
    ignore: &IgnoreMask,
    options: &DiffBuilderOptions,
    estimated_num_rows: usize,
    num_chunks: usize,
) -> Vec<DiffRow> {
    let ranges = chunk_ranges(entries, tokens_source, num_chunks);
    let build_chunk = |range: &Range<usize>| {
        let capacity = estimated_num_rows * range.len() / entries.len().max(1) + 1;
        let mut builder =
            DiffBuilder::with_capacity(tokens_source, tokens_target, options, capacity);
        builder.dimmed = (&ignore.matched_source, &ignore.matched_target);
        for diff_result in &entries[range.clone()] {
            match &diff_result.operation {
                DiffOp::Equal(_) => builder.handle_match(diff_result.clone()),
                DiffOp::Delete => builder.handle_diff(diff_result.clone(), true),
                DiffOp::Insert => builder.handle_diff(diff_result.clone(), false),
            }
        }
        if range.end != entries.len() {
            let (left, right) = (&builder.left, &builder.right);
            assert!(
                left.buf.is_empty() && right.buf.is_empty(),
                "chunk {range:?} must end at a seam"
            );
            assert!(!left.active_diff && !right.active_diff);
        }
        builder.finish_counted()
    };
    if ranges.len() == 1 {
        return build_chunk(&ranges[0]).0;
    }

    let chunks: Vec<_> = ranges.par_iter().map(build_chunk).collect();
    let mut rows = Vec::with_capacity(chunks.iter().map(|(rows, ..)| rows.len()).sum());
    let (mut left_offset, mut right_offset) = (0, 0);
    for (chunk_rows, left_lines, right_lines) in chunks {
        rows.extend(chunk_rows.into_iter().map(|mut row| {
            offset_line_num(&mut row.left, left_offset);
            offset_line_num(&mut row.right, right_offset);
            row
        }));
        left_offset += left_lines;
        right_offset += right_lines;
    }
    rows
}

/// A chunk's line numbers start at 1. Rows without a line (-1) keep it.
fn offset_line_num(content: &mut LineContent, offset: i32) {
    if let LineContent::Code { line_num, .. } = content {
        if *line_num > 0 {
            *line_num += offset;
        }
    }
}

/// Splits `entries` into at most `num_chunks` ranges of about equal length. Every range but the
/// last ends with an Equal Newline (a seam): the builder flushes both sides there, so the next
/// range starts from a fresh builder and only the line numbers carry over.
fn chunk_ranges<T: RawTokenTrait>(
    entries: &[DiffResult],
    tokens_source: Option<&[T]>,
    num_chunks: usize,
) -> Vec<Range<usize>> {
    let Some(tokens_source) = tokens_source else {
        return vec![0..entries.len()];
    };
    let is_seam = |entry: &DiffResult| {
        matches!(entry.operation, DiffOp::Equal(_)) && {
            let idx = entry
                .token_source_idx
                .expect("Equal op must have source index");
            tokens_source[idx as usize].as_ref().kind == TokenKind::Newline
        }
    };

    let mut ranges = Vec::with_capacity(num_chunks);
    let mut start = 0;
    for i in 1..num_chunks {
        let target = (entries.len() * i / num_chunks).max(start);
        let Some(seam) = entries[target..].iter().position(is_seam) else {
            break;
        };
        let end = target + seam + 1;
        if end < entries.len() {
            ranges.push(start..end);
            start = end;
        }
    }
    ranges.push(start..entries.len());
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_harness::DiffTestHarness;

    #[test]
    fn test_build_diff_rows_header_edit() {
        let s1 = "\t#define hello_there\n\t// Comment\n";
        let s2 = "\t#define world_here\n\t// Comment\n";
        let path = vec![
            (0, 0),
            (1, 1),
            (2, 1),
            (2, 2),
            (3, 3),
            (4, 4),
            (5, 5),
            (6, 6),
        ];

        let harness = DiffTestHarness::new(
            s1,
            s2,
            path,
            DiffBuilderOptions {
                ghost_rows: false,
                ..Default::default()
            },
            4,
        );

        harness.assert_row(0, 1, 1, "\t#define hello_there\n", "\t#define world_here\n");
        harness.assert_row(1, 2, 2, "\t// Comment\n", "\t// Comment\n");
    }

    mod chunks {
        use std::sync::{Arc, atomic::AtomicBool};

        use super::*;
        use crate::{
            ignore::IgnorePatterns,
            lexer::{LexerDefault, RawToken},
            myers::{MyersDiffAlgorithm, myers_diff_path},
        };

        // Hunks that add and remove lines, a deleted last line without a newline, and equal
        // lines between them as seams.
        const S1: &str = "fn f() {\n    let a = 1; // old\n    b();\n    c();\n}\nx\ny\nend";
        const S2: &str = "fn f() {\n    let a = 2; // new\n\n    added();\n    b();\n}\nx\nz\ny\n";

        fn lex(s: &str) -> Vec<RawToken> {
            LexerDefault::<RawToken>::new(s).collect()
        }

        fn diff_ir(t1: &[RawToken], t2: &[RawToken], options: &DiffBuilderOptions) -> DiffIR {
            let cmp = |a: &RawToken, b: &RawToken| {
                a.kind == b.kind && S1[a.span.clone()] == S2[b.span.clone()]
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let mask = options.ignore.mask(t1, S1, t2, S2);
            let path = myers_diff_path(
                MyersDiffAlgorithm::Linear,
                t1,
                t2,
                cmp,
                &mask,
                cancel.clone(),
            )
            .expect("not cancelled");
            DiffIR::new(&path, true, cancel).expect("not cancelled")
        }

        #[test]
        fn ranges_cover_the_ir_and_all_but_the_last_end_at_an_equal_newline() {
            let (t1, t2) = (lex(S1), lex(S2));
            let ir = diff_ir(&t1, &t2, &DiffBuilderOptions::default());
            let mut max_ranges = 0;
            for num_chunks in 1..=ir.entries.len() + 1 {
                let ranges = chunk_ranges(&ir.entries, Some(&t1), num_chunks);
                assert!(ranges.len() <= num_chunks, "{num_chunks}: {ranges:?}");
                assert_eq!(ranges.first().unwrap().start, 0, "{num_chunks}");
                assert_eq!(ranges.last().unwrap().end, ir.entries.len(), "{num_chunks}");
                for pair in ranges.windows(2) {
                    assert_eq!(pair[0].end, pair[1].start, "{num_chunks}: {ranges:?}");
                    assert!(!pair[0].is_empty(), "{num_chunks}: {ranges:?}");
                    let last = &ir.entries[pair[0].end - 1];
                    assert!(matches!(last.operation, DiffOp::Equal(_)), "{num_chunks}");
                    let token = &t1[last.token_source_idx.unwrap() as usize];
                    assert_eq!(token.kind, TokenKind::Newline, "{num_chunks}");
                }
                max_ranges = max_ranges.max(ranges.len());
            }
            // With enough chunks every equal newline before the last entry ends a range.
            let seams = ir.entries[..ir.entries.len() - 1]
                .iter()
                .filter(|e| {
                    matches!(e.operation, DiffOp::Equal(_))
                        && t1[e.token_source_idx.unwrap() as usize].kind == TokenKind::Newline
                })
                .count();
            assert!(seams >= 4, "the fixture has equal lines between its hunks");
            assert_eq!(max_ranges, seams + 1);
            // Without source tokens there is no seam to find.
            assert_eq!(
                chunk_ranges::<RawToken>(&ir.entries, None, 4),
                [0..ir.entries.len()]
            );
        }

        #[test]
        fn rows_are_identical_for_every_chunk_count() {
            let (t1, t2) = (lex(S1), lex(S2));
            let ignore_all = IgnoreOptions {
                whitespace: true,
                comments: true,
                patterns: IgnorePatterns::new(r"\d"),
            };
            for (ignore, ghost_rows) in [
                (IgnoreOptions::default(), false),
                (IgnoreOptions::default(), true),
                (ignore_all.clone(), false),
                (ignore_all, true),
            ] {
                let options = DiffBuilderOptions {
                    ignore,
                    ghost_rows,
                    ..Default::default()
                };
                let ir = diff_ir(&t1, &t2, &options);
                // Small enough for build_diff_rows to build it as one chunk.
                let rows = build_diff_rows(
                    ir.clone(),
                    Some(&t1[..]),
                    Some(&t2[..]),
                    S1,
                    S2,
                    &options,
                    1,
                );
                let reference = format!("{rows:#?}");
                let ignore = options.ignore.mask(&t1, S1, &t2, S2);
                let ir = diff_ir_hide_ignored(ir, &ignore);
                let build = |num_chunks| {
                    let rows = build_diff_rows_chunked(
                        &ir.entries,
                        Some(&t1[..]),
                        Some(&t2[..]),
                        &ignore,
                        &options,
                        1,
                        num_chunks,
                    );
                    format!("{rows:#?}")
                };
                assert_eq!(build(1), reference, "{options:?}");
                for num_chunks in 2..=ir.entries.len() + 1 {
                    assert_eq!(
                        build(num_chunks),
                        reference,
                        "{options:?}, {num_chunks} chunks"
                    );
                }
            }
        }
    }

    mod line_then_token {
        use std::sync::{Arc, atomic::AtomicBool};

        use super::*;
        use crate::{
            ignore::IgnorePatterns,
            lexer::{LexerDefault, RawToken},
            myers::{MyersDiffAlgorithm, myers_diff_path},
            test_harness::DiffTestHarness,
        };

        const ALGORITHMS: [MyersDiffAlgorithm; 3] = [
            MyersDiffAlgorithm::Trace,
            MyersDiffAlgorithm::Linear,
            MyersDiffAlgorithm::LinearMT,
        ];

        /// Rows built from the line-then-token diff; the harness lexes with the same lexer.
        fn harness<'a>(
            algorithm: MyersDiffAlgorithm,
            s1: &'a str,
            s2: &'a str,
            ghost_rows: bool,
        ) -> DiffTestHarness<'a> {
            let options = DiffBuilderOptions {
                ghost_rows,
                ..Default::default()
            };
            harness_with(algorithm, s1, s2, options)
        }

        /// Like `harness`, with the diff ignoring what `options.ignore` ignores.
        fn harness_with<'a>(
            algorithm: MyersDiffAlgorithm,
            s1: &'a str,
            s2: &'a str,
            options: DiffBuilderOptions,
        ) -> DiffTestHarness<'a> {
            let t1: Vec<RawToken> = LexerDefault::<RawToken>::new(s1).collect();
            let t2: Vec<RawToken> = LexerDefault::<RawToken>::new(s2).collect();
            let cmp = |a: &RawToken, b: &RawToken| {
                a.kind == b.kind && s1[a.span.clone()] == s2[b.span.clone()]
            };
            let path = myers_diff_path(
                algorithm,
                &t1,
                &t2,
                cmp,
                &options.ignore.mask(&t1, s1, &t2, s2),
                Arc::new(AtomicBool::new(false)),
            )
            .expect("not cancelled");
            DiffTestHarness::new(s1, s2, path, options, 8)
        }

        #[test]
        fn line_inserted_mid_hunk_gets_its_own_row() {
            let s1 = "let a = 1;\nlet b = 2;\nlet c = 3;\nlet d = 4;\n";
            let s2 = "let a = 10;\nlet b = 20;\nnew();\nlet c = 30;\nlet d = 40;\n";
            for algorithm in ALGORITHMS {
                let h = harness(algorithm, s1, s2, false);
                h.assert_row(0, 1, 1, "let a = 1;\n", "let a = 10;\n");
                h.assert_row(1, 2, 2, "let b = 2;\n", "let b = 20;\n");
                h.assert_row(2, -1, 3, "VOID", "new();\n");
                h.assert_row(3, 3, 4, "let c = 3;\n", "let c = 30;\n");
                h.assert_row(4, 4, 5, "let d = 4;\n", "let d = 40;\n");

                // Ghost tokens inline: each row carries the other side's changed tokens.
                let h = harness(algorithm, s1, s2, true);
                h.assert_row(0, 1, 1, "let a = 110;\n", "let a = 110;\n");
                h.assert_row(1, 2, 2, "let b = 220;\n", "let b = 220;\n");
                h.assert_row(2, -1, 3, "new();\n", "new();\n");
                h.assert_row(3, 3, 4, "let c = 330;\n", "let c = 330;\n");
                h.assert_row(4, 4, 5, "let d = 440;\n", "let d = 440;\n");
            }
        }

        #[test]
        fn equal_lines_between_a_pure_insert_and_a_delete_at_eof() {
            let s1 = "one\ntwo\nthree\nfour\nfive";
            let s2 = "one\ntwo\nthree\nadded\nfour\n";
            for algorithm in ALGORITHMS {
                for ghost_rows in [false, true] {
                    let h = harness(algorithm, s1, s2, ghost_rows);
                    let (ghost_added, ghost_five) = match ghost_rows {
                        false => ("VOID", "VOID"),
                        true => ("added\n", "five"),
                    };
                    h.assert_row(0, 1, 1, "one\n", "one\n");
                    h.assert_row(1, 2, 2, "two\n", "two\n");
                    h.assert_row(2, 3, 3, "three\n", "three\n");
                    h.assert_row(3, -1, 4, ghost_added, "added\n");
                    h.assert_row(4, 4, 5, "four\n", "four\n");
                    h.assert_row(5, 5, -1, "five", ghost_five);
                }
            }
        }

        #[test]
        fn ignored_comment_changes_are_hidden_and_form_no_diff_span() {
            let s1 = "fn f() {\n    let a = 1; // old\n    /* note */ b();\n    c();\n}\n";
            let s2 = "fn f() {\n    let a = 1; // new text\n    /* a longer note */ b();\n    c(); // added\n}\n";
            // A row is a diff span when it shows a change that isn't hidden (precompute_diff_spans
            // in zdiff-gui).
            let has_visible_change = |content: &LineContent| match content {
                LineContent::Code { tokens, .. } => tokens.iter().any(|(res, _, _)| {
                    !res.hide_in_diff && !matches!(res.operation, DiffOp::Equal(_))
                }),
                _ => false,
            };
            for algorithm in ALGORITHMS {
                for ghost_rows in [false, true] {
                    let options = DiffBuilderOptions {
                        ignore: IgnoreOptions {
                            comments: true,
                            ..Default::default()
                        },
                        ghost_rows,
                        ..Default::default()
                    };
                    let h = harness_with(algorithm, s1, s2, options);
                    h.assert_row(0, 1, 1, "fn f() {\n", "fn f() {\n");
                    // Hidden edits still carry ghosts, as hidden whitespace does.
                    let (l1, r1, l2, r2, l3, r3) = match ghost_rows {
                        false => (
                            "    let a = 1; // old\n",
                            "    let a = 1; // new text\n",
                            "    /* note */ b();\n",
                            "    /* a longer note */ b();\n",
                            "    c();\n",
                            "    c(); // added\n",
                        ),
                        true => (
                            "    let a = 1; // old// new text\n",
                            "    let a = 1; // old// new text\n",
                            "    /* a longer note */ b();\n",
                            "    /* a longer note */ b();\n",
                            "    c(); // added\n",
                            "    c(); // added\n",
                        ),
                    };
                    h.assert_row(1, 2, 2, l1, r1);
                    h.assert_row(2, 3, 3, l2, r2);
                    h.assert_row(3, 4, 4, l3, r3);
                    h.assert_row(4, 5, 5, "}\n", "}\n");
                    assert_eq!(h.rows().len(), 5, "{algorithm:?}, ghosts {ghost_rows}");
                    for (idx, row) in h.rows().iter().enumerate() {
                        assert!(
                            !has_visible_change(&row.left) && !has_visible_change(&row.right),
                            "{algorithm:?}, ghosts {ghost_rows}: row {idx} is a diff span"
                        );
                    }

                    // With the option off the same rows are diff spans.
                    let h = harness(algorithm, s1, s2, ghost_rows);
                    let spans = h
                        .rows()
                        .iter()
                        .filter(|row| {
                            has_visible_change(&row.left) || has_visible_change(&row.right)
                        })
                        .count();
                    assert_eq!(spans, 3, "{algorithm:?}, ghosts {ghost_rows}");
                }
            }
        }

        #[test]
        fn lines_differing_only_in_matched_text_form_no_diff_span_and_are_dimmed() {
            let s1 = "[12:00:01] start\nkeep(1);\n[12:00:02] done\n";
            let s2 = "[12:00:05] start\nkeep(1);\n[12:00:09] done\n";
            let (t1, t2): (Vec<RawToken>, Vec<RawToken>) = (
                LexerDefault::<RawToken>::new(s1).collect(),
                LexerDefault::<RawToken>::new(s2).collect(),
            );
            let has_visible_change = |content: &LineContent| match content {
                LineContent::Code { tokens, .. } => tokens.iter().any(|(res, _, _)| {
                    !res.hide_in_diff && !matches!(res.operation, DiffOp::Equal(_))
                }),
                _ => false,
            };
            // The timestamp is bytes 1..9 of its line; nothing else matches, not even `1`.
            let in_timestamp = |text: &str, token: &RawToken| {
                let line_start = text[..token.span.start].rfind('\n').map_or(0, |i| i + 1);
                text[line_start..].starts_with('[')
                    && token.span.start >= line_start + 1
                    && token.span.end <= line_start + 9
            };
            let dimmed = DiffTheme::default().dimmed;
            for algorithm in ALGORITHMS {
                for ghost_rows in [false, true] {
                    let options = DiffBuilderOptions {
                        ignore: IgnoreOptions {
                            patterns: IgnorePatterns::new(r"\d\d:\d\d:\d\d"),
                            ..Default::default()
                        },
                        ghost_rows,
                        ..Default::default()
                    };
                    let h = harness_with(algorithm, s1, s2, options);
                    if !ghost_rows {
                        h.assert_row(0, 1, 1, "[12:00:01] start\n", "[12:00:05] start\n");
                        h.assert_row(1, 2, 2, "keep(1);\n", "keep(1);\n");
                        h.assert_row(2, 3, 3, "[12:00:02] done\n", "[12:00:09] done\n");
                    }
                    assert_eq!(h.rows().len(), 3, "{algorithm:?}, ghosts {ghost_rows}");
                    for (idx, row) in h.rows().iter().enumerate() {
                        assert!(
                            !has_visible_change(&row.left) && !has_visible_change(&row.right),
                            "{algorithm:?}, ghosts {ghost_rows}: row {idx} is a diff span"
                        );
                        for (content, text, tokens, is_left) in
                            [(&row.left, s1, &t1, true), (&row.right, s2, &t2, false)]
                        {
                            let LineContent::Code {
                                tokens: row_tokens, ..
                            } = content
                            else {
                                panic!("row {idx} is not code");
                            };
                            for (res, color, _) in row_tokens.iter().filter(|(_, _, g)| !g) {
                                let token_idx = if is_left {
                                    res.token_source_idx
                                } else {
                                    res.token_target_idx
                                };
                                let token = &tokens[token_idx.unwrap() as usize];
                                assert_eq!(
                                    *color == dimmed,
                                    in_timestamp(text, token),
                                    "{algorithm:?}, ghosts {ghost_rows}: row {idx} {:?}",
                                    &text[token.span.clone()]
                                );
                            }
                        }
                    }

                    // Without patterns the timestamp rows are diff spans.
                    let h = harness(algorithm, s1, s2, ghost_rows);
                    let spans = h
                        .rows()
                        .iter()
                        .filter(|row| {
                            has_visible_change(&row.left) || has_visible_change(&row.right)
                        })
                        .count();
                    assert_eq!(spans, 2, "{algorithm:?}, ghosts {ghost_rows}");
                }
            }
        }
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::cached_file::CachedFile;
    use crate::diff_ir::DiffIR;
    use crate::lexer::{LEXER_MODE_DEFAULT, RawToken};
    use crate::myers::{
        myers_backtrack, myers_diff_linear, myers_diff_linear_mt, myers_diff_trace,
    };
    use std::fs::File;
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;
    use tempfile::tempdir;

    fn run_reconstruction_test(s1: &str, s2: &str) {
        let dir = tempdir().unwrap();
        let p1 = dir.path().join("file1.rs");
        let p2 = dir.path().join("file2.rs");

        File::create(&p1).unwrap().write_all(s1.as_bytes()).unwrap();
        File::create(&p2).unwrap().write_all(s2.as_bytes()).unwrap();

        let f1 = CachedFile::<RawToken>::new(p1.clone().into(), p1, LEXER_MODE_DEFAULT).unwrap();
        let f2 = CachedFile::<RawToken>::new(p2.clone().into(), p2, LEXER_MODE_DEFAULT).unwrap();

        let cmp = |t1: &RawToken, t2: &RawToken| {
            f1.contents[t1.as_ref().span.clone()] == f2.contents[t2.as_ref().span.clone()]
        };

        const MYERS_LINEAR: bool = true;
        let path = if MYERS_LINEAR {
            const MYERS_LINEAR_MT: bool = false;
            if MYERS_LINEAR_MT {
                myers_diff_linear_mt(
                    &f1.tokens,
                    &f2.tokens,
                    cmp,
                    Arc::new(AtomicBool::new(false)),
                )
                .expect("Myers linear MT failed")
            } else {
                myers_diff_linear(
                    &f1.tokens,
                    &f2.tokens,
                    cmp,
                    Arc::new(AtomicBool::new(false)),
                )
                .expect("Myers linear failed")
            }
        } else {
            let trace = myers_diff_trace(&f1.tokens, &f2.tokens, cmp);
            myers_backtrack(
                trace,
                f1.tokens.len() as i32,
                f2.tokens.len() as i32,
                Arc::new(AtomicBool::new(false)),
            )
            .expect("Myers backtrack failed")
        };

        let rows = build_diff_rows(
            DiffIR::new(&path, false, Arc::new(AtomicBool::new(false))).unwrap(),
            Some(&f1.tokens),
            Some(&f2.tokens),
            &f1.contents,
            &f2.contents,
            &DiffBuilderOptions {
                ignore: IgnoreOptions::default(),
                ghost_rows: false,
                ..Default::default()
            },
            f1.metadata.num_lines().max(f2.metadata.num_lines()),
        );

        let mut left_res = String::new();
        let mut right_res = String::new();

        for row in rows {
            if let LineContent::Code { tokens, .. } = row.left {
                for (res, _, _) in tokens {
                    if res.operation != DiffOp::Insert {
                        let idx = res.token_source_idx.expect("Source index missing");
                        left_res
                            .push_str(&f1.contents[f1.tokens[idx as usize].as_ref().span.clone()]);
                    }
                }
            }
            if let LineContent::Code { tokens, .. } = row.right {
                for (res, _, _) in tokens {
                    if res.operation != DiffOp::Delete {
                        let idx = res.token_target_idx.expect("Target index missing");
                        right_res
                            .push_str(&f2.contents[f2.tokens[idx as usize].as_ref().span.clone()]);
                    }
                }
            }
        }

        assert_eq!(s1, left_res, "Source reconstruction failed");
        assert_eq!(s2, right_res, "Target reconstruction failed");
    }

    #[test]
    fn test_reconstruct_basic_edit() {
        run_reconstruction_test(
            "fn main() {\n    let x = 10;\n}\n",
            "fn main() {\n    let x = 20;\n    let y = 30;\n}\n",
        );
    }

    #[test]
    fn test_reconstruct_empty_to_content() {
        run_reconstruction_test("", "println!(\"hello world\");\n");
    }

    #[test]
    fn test_reconstruct_trailing_newlines() {
        run_reconstruction_test("line\n", "line\n\n\n");
    }

    #[test]
    fn test_reconstruct_complex_whitespace() {
        run_reconstruction_test("\t\tindent\n    spaces\n", "\t\tindent;\n    spaces;\n");
    }

    #[test]
    fn test_reconstruct_simple_ignore_whitespace() {
        run_reconstruction_test("pub trait Processor {", "pub trait \nProcessor {");
    }
}
