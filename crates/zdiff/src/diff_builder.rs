use std::{
    io,
    ops::Range,
    path::{Path, PathBuf},
};

use crate::{
    diff_ir::{DiffIR, DiffOp, DiffResult},
    lexer::{Lexer, RawTokenTrait, TokenKind},
    read_file_contents,
};
use zcommon::hash::hash_file;

#[derive(Copy, Clone, Debug)]
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

#[derive(Debug, Clone)]
pub enum LineContent {
    Code {
        tokens: Vec<(DiffResult, Color32)>,
        line_num: i32,
        bg: Color32,
    },
    Void,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct DiffBuilderOptions {
    pub ignore_whitespace: bool,
    pub highlight_rows: bool,
    pub ghost_rows: bool,
    pub keyword_highlight: bool,
}
impl Default for DiffBuilderOptions {
    fn default() -> Self {
        Self {
            ignore_whitespace: false,
            highlight_rows: true,
            ghost_rows: true,
            keyword_highlight: true,
        }
    }
}

struct DiffTheme {
    ghost: Color32,
    kw: Color32,
    del: Color32,
    ins: Color32,
    del_bg: Color32,
    ins_bg: Color32,
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
        }
    }
}

struct SideState {
    buf: Vec<(DiffResult, Color32, bool)>, // String, Color, is_whitespace
    line_num: i32,
    active_diff: bool,
}

impl SideState {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            line_num: 1,
            active_diff: false,
        }
    }

    fn push(&mut self, val: DiffResult, color: Color32, is_ws: bool) {
        self.buf.push((val, color, is_ws));
    }

    fn flush(&mut self, has_diff: bool, bg_color: Color32) -> LineContent {
        if self.buf.is_empty() {
            LineContent::Void
        } else {
            let tokens = self.buf.drain(..).map(|(s, c, _)| (s, c)).collect();
            LineContent::Code {
                tokens,
                line_num: self.line_num,
                bg: if has_diff {
                    bg_color
                } else {
                    Color32::TRANSPARENT
                },
            }
        }
    }
}

#[derive(Debug, Default)]
pub struct FileMetadata {
    pub line_starts: Vec<usize>,
}

impl FileMetadata {
    pub fn new(contents: &str) -> Self {
        let line_starts = std::iter::once(0)
            .chain(contents.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        Self { line_starts }
    }

    pub fn get_line_index(&self, byte_offset: usize) -> usize {
        match self.line_starts.binary_search(&byte_offset) {
            Ok(line) => line,
            Err(line) => line - 1,
        }
    }
}

#[derive(Debug, Default)]
pub struct CachedFile<T: RawTokenTrait> {
    pub path: PathBuf,
    pub hash: String,
    pub contents: String,
    pub tokens: Vec<T>,
    pub metadata: FileMetadata,
}

impl<T: RawTokenTrait> CachedFile<T> {
    pub fn read_content_span(&self, span: Range<usize>) -> &str {
        &self.contents[span]
    }

    // returns vec of lines that match
    pub fn content_search(&self, query: &str) -> Vec<usize> {
        if query.is_empty() {
            return vec![];
        }
        self.contents
            .match_indices(query)
            .map(|(offset, _)| self.metadata.get_line_index(offset))
            .collect()
    }
}

impl<T: RawTokenTrait> CachedFile<T> {
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let contents = read_file_contents(&path)?;
        let hash = hash_file(&path)?;
        let tokens = Lexer::<T>::new(&contents).map(T::from).collect();
        let path = path.as_ref().to_path_buf();
        let metadata = FileMetadata::new(&contents);
        Ok(Self {
            path,
            hash,
            contents,
            tokens,
            metadata,
        })
    }
}

pub struct DiffBuilder<'a, 'b, T: RawTokenTrait> {
    tokens_source: Option<&'a [T]>,
    tokens_target: Option<&'a [T]>,
    options: &'b DiffBuilderOptions,
    theme: DiffTheme,
    rows: Vec<DiffRow>,
    left: SideState,
    right: SideState,
}

impl<'a, 'b, T: RawTokenTrait> DiffBuilder<'a, 'b, T> {
    pub fn new(
        tokens_source: Option<&'a [T]>,
        tokens_target: Option<&'a [T]>,
        options: &'b DiffBuilderOptions,
    ) -> Self {
        Self {
            tokens_source,
            tokens_target,
            options,
            theme: DiffTheme::default(),
            rows: Vec::new(),
            left: SideState::new(),
            right: SideState::new(),
        }
    }

    fn get_color(&self, is_keyword: bool) -> Color32 {
        if self.options.keyword_highlight && is_keyword {
            self.theme.kw
        } else {
            Color32::GRAY
        }
    }

    pub fn handle_match(&mut self, diff_result: DiffResult) {
        assert!(matches!(diff_result.operation, DiffOp::Equal));
        let token = &self.tokens_source.expect("Source was None")[diff_result.token_idx as usize];
        let color = self.get_color(token.as_ref().kind.is_keyword());
        let is_ws = token.as_ref().kind.is_whitespace();
        let is_newline = token.as_ref().kind == TokenKind::Newline;

        self.left.push(diff_result.clone(), color, is_ws);
        self.right.push(diff_result, color, is_ws);

        if is_newline {
            // We must flush both sides, but independently.
            // If one side had more tokens than the other due to previous diffs,
            // this ensures they stay in their respective "staircase" slots.
            self.emit_row(true, true);
        }
    }

    pub fn handle_diff(&mut self, diff_result: DiffResult, is_deletion: bool) {
        let token = if is_deletion {
            &self.tokens_source.expect("Source is None")[diff_result.token_idx as usize]
        } else {
            &self.tokens_target.expect("Target is none")[diff_result.token_idx as usize]
        };

        let is_ws = token.as_ref().kind.is_whitespace();
        let is_newline = token.as_ref().kind == TokenKind::Newline;

        // FIX: Only skip if it's a diff-injected whitespace that isn't a newline.
        if self.options.ignore_whitespace && is_ws && !is_newline {
            return;
        }

        if !is_ws {
            if is_deletion {
                self.left.active_diff = true;
            } else {
                self.right.active_diff = true;
            }
        }

        let color = if is_deletion {
            self.theme.del
        } else {
            self.theme.ins
        };

        if is_deletion {
            self.left.push(diff_result, color, is_ws);
            if is_newline {
                self.emit_row(true, false);
            }
        } else {
            self.right.push(diff_result, color, is_ws);
            if is_newline {
                self.emit_row(false, true);
            }
        }
    }

    fn emit_row(&mut self, flush_left: bool, flush_right: bool) {
        let hi = self.options.highlight_rows;

        // Logic check: We only create a row if the side being flushed actually has content.
        // This prevents the "empty row" bug that shifts line numbers.
        let left = if flush_left && !self.left.buf.is_empty() {
            let side = &mut self.left;
            let content = side.flush(side.active_diff && hi, self.theme.del_bg);
            side.line_num += 1;
            side.active_diff = false;
            content
        } else {
            LineContent::Void
        };

        let right = if flush_right && !self.right.buf.is_empty() {
            let side = &mut self.right;
            let content = side.flush(side.active_diff && hi, self.theme.ins_bg);
            side.line_num += 1;
            side.active_diff = false;
            content
        } else {
            LineContent::Void
        };

        if !matches!(left, LineContent::Void) || !matches!(right, LineContent::Void) {
            self.rows.push(DiffRow { left, right });
        }
    }

    pub fn finish(mut self) -> Vec<DiffRow> {
        // Final flush for trailing text without newlines
        if !self.left.buf.is_empty() || !self.right.buf.is_empty() {
            self.emit_row(true, true);
        }
        self.rows
    }

    // fn apply_ghosts(&mut self) -> bool {
    //     let l_empty = self.left.buf.is_empty();
    //     let r_empty = self.right.buf.is_empty();

    //     if l_empty && !r_empty {
    //         let mut started = false;
    //         for (val, _, is_ws) in &self.right.buf {
    //             let color = if *is_ws && !started {
    //                 Color32::BLACK
    //             } else {
    //                 self.theme.ghost
    //             };
    //             if !*is_ws {
    //                 started = true;
    //             }
    //             self.left.buf.push((val.clone(), color, *is_ws));
    //         }
    //         return true;
    //     } else if r_empty && !l_empty {
    //         let mut started = false;
    //         for (val, _, is_ws) in &self.left.buf {
    //             let color = if *is_ws && !started {
    //                 Color32::TRANSPARENT
    //             } else {
    //                 self.theme.ghost
    //             };
    //             if !*is_ws {
    //                 started = true;
    //             }
    //             self.right.buf.push((val.clone(), color, *is_ws));
    //         }
    //         return true;
    //     }
    //     false
    // }
}

pub fn build_diff_rows<'a, T: RawTokenTrait>(
    diff_ir: DiffIR,
    tokens_source: Option<&'a [T]>,
    tokens_target: Option<&'a [T]>,
    options: &DiffBuilderOptions,
) -> Vec<DiffRow> {
    let mut builder = DiffBuilder::new(tokens_source, tokens_target, options);
    for diff_result in diff_ir.entries {
        match &diff_result.operation {
            DiffOp::Equal => builder.handle_match(diff_result),
            DiffOp::Delete => builder.handle_diff(diff_result, true),
            DiffOp::Insert => builder.handle_diff(diff_result, false),
        }
    }
    builder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        diff_ir::{self},
        lexer::RawToken,
    };

    struct DiffTestHarness<'a> {
        s1: &'a str,
        s2: &'a str,
        t1: Vec<RawToken>,
        t2: Vec<RawToken>,
        rows: Vec<DiffRow>,
    }

    impl<'a> DiffTestHarness<'a> {
        fn new(
            s1: &'a str,
            s2: &'a str,
            path: Vec<(i32, i32)>,
            options: DiffBuilderOptions,
        ) -> Self {
            let t1: Vec<RawToken> = Lexer::<RawToken>::new(s1).collect();
            let t2: Vec<RawToken> = Lexer::<RawToken>::new(s2).collect();
            let diff_ir = DiffIR::new(&path);
            let rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options);

            Self {
                s1,
                s2,
                t1,
                t2,
                rows,
            }
        }

        fn assert_row(&self, idx: usize, l_num: i32, r_num: i32, l_text: &str, r_text: &str) {
            let row = self.rows.get(idx).unwrap_or_else(|| {
                panic!(
                    "Expected row at index {}, but only {} rows exist.",
                    idx,
                    self.rows.len()
                )
            });

            assert_row_content(
                idx, row, l_num, r_num, l_text, r_text, &self.t1, &self.t2, self.s1, self.s2,
            );
        }
    }

    fn assert_row_content(
        idx: usize,
        row: &DiffRow,
        l_line: i32,
        r_line: i32,
        l_text: &str,
        r_text: &str,
        l_tokens: &[RawToken],
        r_tokens: &[RawToken],
        s1: &str,
        s2: &str,
    ) {
        let extract_details = |content: &LineContent| match content {
            LineContent::Code {
                tokens, line_num, ..
            } => {
                let mut text = String::new();
                let mut debug_tokens = Vec::new();

                for (res, _) in tokens {
                    let (src_tokens, src_text) = match res.operation {
                        diff_ir::DiffOp::Equal | diff_ir::DiffOp::Delete => (l_tokens, s1),
                        diff_ir::DiffOp::Insert => (r_tokens, s2),
                    };
                    let token_raw = &src_tokens[res.token_idx as usize];
                    let val = &src_text[token_raw.as_ref().span.clone()];

                    text.push_str(val);
                    debug_tokens.push(format!(
                        "[{:?}: {:?}{}]",
                        res.operation,
                        val.replace('\n', "\\n"),
                        if token_raw.as_ref().kind.is_whitespace() {
                            " (WS)"
                        } else {
                            ""
                        }
                    ));
                }
                (text, *line_num, debug_tokens.join(" "))
            }
            LineContent::Void => ("VOID".to_string(), -1, "VOID".to_string()),
        };

        let (act_l_text, act_l_num, act_l_debug) = extract_details(&row.left);
        let (act_r_text, act_r_num, act_r_debug) = extract_details(&row.right);

        if act_l_text != l_text
            || act_r_text != r_text
            || act_l_num != l_line
            || act_r_num != r_line
        {
            panic!(
                "\nFAIL: Row Index {}\n\
                 {:-<105}\n\
                 {:<5} | {:<5} | {:<40} | {:<5} | {:<40}\n\
                 {:-<105}\n\
                 {:<5} | {:<5} | {:<40?} | {:<5} | {:<40?}\n\
                 {:<5} | {:<5} | {:<40?} | {:<5} | {:<40?}\n\
                 {:-<105}\n\
                 DEBUG TOKENS (ACTUAL):\n\
                 LEFT:  {}\n\
                 RIGHT: {}\n",
                idx,
                "-",
                "SIDE",
                "L-NUM",
                "LEFT TEXT",
                "R-NUM",
                "RIGHT TEXT",
                "-",
                "EXP",
                l_line,
                l_text,
                r_line,
                r_text,
                "ACT",
                act_l_num,
                act_l_text,
                act_r_num,
                act_r_text,
                "-",
                act_l_debug,
                act_r_debug
            );
        }
    }

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
        );

        harness.assert_row(0, 1, 1, "\t#define hello_there\n", "\t#define world_here\n");
        harness.assert_row(1, 2, 2, "\t// Comment\n", "\t// Comment\n");
    }

    // #[test]
    // fn test_build_diff_rows_ghost_enabled() {
    //     let s1 = "deleted_line\nmatch\n";
    //     let s2 = "match\n";
    //     let path = vec![(0, 0), (1, 0), (2, 0), (3, 1), (4, 2)];

    //     let harness = DiffTestHarness::new(
    //         s1,
    //         s2,
    //         path,
    //         DiffBuilderOptions {
    //             ghost_rows: true,
    //             ..Default::default()
    //         },
    //     );

    //     // The deleted line flushes a row. Ghosting inserts the deleted tokens into the target buffer.
    //     harness.assert_row(0, 1, 1, "deleted_line\n", "deleted_line\n");
    //     harness.assert_row(1, 2, 2, "match\n", "match\n");
    // }

    // #[test]
    // fn test_build_diff_rows_respects_whitespace() {
    //     let s1 = "ImGuiChildFlags_Border\n";
    //     let s2 = "ImGuiChildFlags_Border,  // COMMENT\n";
    //     // Path adjusted to include the whitespace tokens in s2
    //     let path = vec![(0, 0), (1, 0), (1, 1), (1, 2), (1, 3), (1, 4), (2, 5)];

    //     // TEST 1: Literal (ignore_whitespace: false)
    //     let harness_literal = DiffTestHarness::new(
    //         s1,
    //         s2,
    //         path.clone(),
    //         DiffBuilderOptions {
    //             ignore_whitespace: false,
    //             ..Default::default()
    //         },
    //     );

    //     harness_literal.assert_row(
    //         0,
    //         1,
    //         1,
    //         "ImGuiChildFlags_Border\n",
    //         "ImGuiChildFlags_Border,  // COMMENT\n", // Must be literal
    //     );

    //     // TEST 2: Neutralized (ignore_whitespace: true)
    //     let harness_ignored = DiffTestHarness::new(
    //         s1,
    //         s2,
    //         path,
    //         DiffBuilderOptions {
    //             ignore_whitespace: true,
    //             ..Default::default()
    //         },
    //     );

    //     harness_ignored.assert_row(
    //         0,
    //         1,
    //         1,
    //         "ImGuiChildFlags_Border\n",
    //         "ImGuiChildFlags_Border,// COMMENT\n", // Spaces neutralized/collapsed
    //     );
    // }

    // #[test]
    // fn test_build_diff_rows_staircase_regression() {
    //     // Scenario: Source has one line, Target splits it into two.
    //     // This is exactly what happened with your 'id: u64' example.
    //     let s1 = "id: u64,\ninner: Arc,\n";
    //     let s2 = "id:\n    usize,\ninner: Arc,\n";

    //     // Manually constructed path to simulate the edit:
    //     // 1. Match "id:"
    //     // 2. Delete " " and "u64,"
    //     // 3. Insert "\n" (This is the trigger!)
    //     // 4. Insert "    " and "usize,"
    //     // 5. Match "\n"
    //     // 6. Match "inner: Arc,\n"
    //     let path = vec![
    //         (0, 0),
    //         (1, 1), // Match "id:"
    //         (2, 1), // Delete " "
    //         (3, 1), // Delete "u64,"
    //         (3, 2), // Insert "\n" -> CURRENT BUG: This flushes 'id: u64,' + 'id:\n'
    //         (3, 3), // Insert "    "
    //         (3, 4), // Insert "usize,"
    //         (4, 5), // Match "\n"
    //         (5, 6), // Match "inner:"
    //         (6, 7), // Match " "
    //         (7, 8), // Match "Arc,"
    //         (8, 9), // Match "\n"
    //     ];

    //     let harness = DiffTestHarness::new(
    //         s1,
    //         s2,
    //         path,
    //         DiffBuilderOptions {
    //             ghost_rows: false,
    //             ..Default::default()
    //         },
    //     );

    //     // --- EXPECTED BEHAVIOR ---
    //     // Row 0: VOID on left, "id:\n" on right
    //     // Row 1: "id: u64,\n" on left, "    usize,\n" on right
    //     // Row 2: "inner: Arc,\n" on left, "inner: Arc,\n" on right

    //     // --- CURRENT BUGGY BEHAVIOR ---
    //     // Row 0: "id: u64," on left, "id:\n" on right
    //     // Row 1: "inner: Arc," on left, "    usize,\n" on right  <-- SHIFT!

    //     harness.assert_row(0, -1, 1, "VOID", "id:\n");
    //     harness.assert_row(1, 1, 2, "id: u64,\n", "    usize,\n");
    //     harness.assert_row(2, 2, 3, "inner: Arc,\n", "inner: Arc,\n");
    // }

    // #[test]
    // fn test_whitespace_and_newline_behavior() {
    //     // Scenario 1: Source has a standard use and trait.
    //     // Scenario 2: Target inserts a newline after 'use' and an extra space in the trait.
    //     let s1 = "use std::collections::HashMap;\npub trait NewProcessor {\n";
    //     let s2 = "use \nstd::collections::HashMap;\npub trait  Processor {\n";

    //     // Manually constructed path to replicate the reported bugged behavior:
    //     // 1. Match "use "
    //     // 2. Insert "\n" -> This triggers the premature flush in the current code.
    //     // 3. Match the rest of the HashMap line.
    //     // 4. Match "pub trait "
    //     // 5. Insert " " -> This causes the double space when ignore_whitespace is true.
    //     // 6. Match/Diff "Processor {" and "\n"
    //     let path = vec![
    //         (0, 0),
    //         (1, 1), // "use "
    //         (1, 2), // Insert "\n"
    //         (2, 3),
    //         (3, 4),
    //         (4, 5),
    //         (5, 6),
    //         (6, 7),
    //         (7, 8),
    //         (8, 9), // "std::collections::HashMap;\n"
    //         (9, 10),
    //         (10, 11),
    //         (11, 12),
    //         (12, 13), // "pub trait "
    //         (12, 14), // Insert " "
    //         (13, 15), // "Processor"
    //         (14, 16),
    //         (15, 17),
    //         (16, 18), // " {\n"
    //     ];

    //     let harness = DiffTestHarness::new(
    //         s1,
    //         s2,
    //         path,
    //         DiffBuilderOptions {
    //             ignore_whitespace: true,
    //             ghost_rows: false,
    //             ..Default::default()
    //         },
    //     );

    //     // --- EXPECTED BEHAVIOR (FIXED) ---

    //     // 1. The inserted newline should not orphan "use " if ignore_whitespace is true.
    //     // It should ideally be part of a single logical change or collapsed.
    //     harness.assert_row(
    //         0,
    //         1,
    //         1,
    //         "use std::collections::HashMap;\n",
    //         "use \nstd::collections::HashMap;\n",
    //     );

    //     // 2. The extra space in "pub trait  Processor" should be dropped/collapsed
    //     // because ignore_whitespace is enabled.
    //     harness.assert_row(
    //         1,
    //         2,
    //         2,
    //         "pub trait NewProcessor {\n",
    //         "pub trait Processor {\n",
    //     );
    // }
}
