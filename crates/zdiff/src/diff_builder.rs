use crate::{
    diff_ir::{DiffIR, DiffOp, DiffResult, diff_ir_to_no_ws},
    lexer::{RawTokenTrait, TokenKind},
};

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
    pub pivot_lines: Option<(usize, usize)>,
}
impl Default for DiffBuilderOptions {
    fn default() -> Self {
        Self {
            ignore_whitespace: false,
            highlight_rows: true,
            ghost_rows: true,
            keyword_highlight: true,
            pivot_lines: None,
        }
    }
}

impl DiffBuilderOptions {
    pub fn need_invalidation(old: &Self, new: &Self) -> bool {
        let mut ret = old.ghost_rows != new.ghost_rows
            || old.highlight_rows != new.highlight_rows
            || old.ignore_whitespace != new.ignore_whitespace
            || old.keyword_highlight != new.keyword_highlight;
        if !ret {
            ret = matches!(new.pivot_lines, Some((p1, p2)) if p1 > 0 && p2 > 0)
                && old.pivot_lines != new.pivot_lines;
        }

        ret
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
    buf: Vec<(DiffResult, Color32)>,
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

    fn push(&mut self, val: DiffResult, color: Color32) {
        self.buf.push((val, color));
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

    pub fn handle_match(&mut self, diff_result: DiffResult) {
        assert!(matches!(diff_result.operation, DiffOp::Equal(_)));

        let token_idx = diff_result
            .token_source_idx
            .expect("Equal op must have source index");
        let token = &self.tokens_source.expect("Source was None")[token_idx as usize];

        let color = self.get_color(token.as_ref().kind.is_keyword());
        let is_newline = token.as_ref().kind == TokenKind::Newline;

        self.left.push(diff_result.clone(), color);
        self.right.push(diff_result, color);

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

        let side = if is_deletion {
            &mut self.left
        } else {
            &mut self.right
        };
        if !diff_result.hide_in_diff {
            side.active_diff = true;
        }

        let color = if is_deletion {
            self.theme.del
        } else {
            self.theme.ins
        };
        side.push(diff_result.clone(), color);

        if self.options.ghost_rows {
            self.apply_ghosts(is_deletion, diff_result);
        }

        if is_newline {
            if self.options.ghost_rows {
                // Force flush is needed for better new line handling for ghosting
                self.emit_row(true, true, is_deletion, !is_deletion);
            } else {
                self.emit_row(is_deletion, !is_deletion, is_deletion, !is_deletion);
            }
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

    pub fn finish(mut self) -> Vec<DiffRow> {
        if !self.left.buf.is_empty() || !self.right.buf.is_empty() {
            let inc_l = self
                .left
                .buf
                .iter()
                .any(|(r, _)| r.operation != DiffOp::Insert);
            let inc_r = self
                .right
                .buf
                .iter()
                .any(|(r, _)| r.operation != DiffOp::Delete);
            self.emit_row(true, true, inc_l, inc_r);
        }
        self.rows
    }

    fn apply_ghosts(&mut self, last_was_deletion: bool, result: DiffResult) {
        let ghost_color = self.theme.ghost;
        if last_was_deletion {
            self.right.push(result, ghost_color);
        } else {
            self.left.push(result, ghost_color);
        }
    }
}

pub fn build_diff_rows<'a, T: RawTokenTrait>(
    mut diff_ir: DiffIR,
    tokens_source: Option<&'a [T]>,
    tokens_target: Option<&'a [T]>,
    options: &DiffBuilderOptions,
    estimated_num_rows: usize,
) -> Vec<DiffRow> {
    if options.ignore_whitespace {
        diff_ir = diff_ir_to_no_ws(diff_ir, tokens_source, tokens_target);
    }

    let mut builder =
        DiffBuilder::with_capacity(tokens_source, tokens_target, options, estimated_num_rows);
    for diff_result in diff_ir.entries {
        match &diff_result.operation {
            DiffOp::Equal(_) => builder.handle_match(diff_result),
            DiffOp::Delete => builder.handle_diff(diff_result, true),
            DiffOp::Insert => builder.handle_diff(diff_result, false),
        }
    }

    builder.finish()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::AtomicBool};

    use super::*;
    use crate::lexer::{LexerDefault, RawToken};

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
            estimated_num_rows: usize,
        ) -> Self {
            let t1: Vec<RawToken> = LexerDefault::<RawToken>::new(s1).collect();
            let t2: Vec<RawToken> = LexerDefault::<RawToken>::new(s2).collect();
            let diff_ir = DiffIR::new(&path, false, Arc::new(AtomicBool::new(false))).unwrap();
            let rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options, estimated_num_rows);

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
                    // Determine which token array and which index to use
                    let (src_tokens, src_text, idx) = match res.operation {
                        DiffOp::Equal(..) | DiffOp::Delete => (
                            l_tokens,
                            s1,
                            res.token_source_idx
                                .expect("Equal/Delete must have source index"),
                        ),
                        DiffOp::Insert => (
                            r_tokens,
                            s2,
                            res.token_target_idx.expect("Insert must have target index"),
                        ),
                    };

                    let token_raw = &src_tokens[idx as usize];
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
            4,
        );

        harness.assert_row(0, 1, 1, "\t#define hello_there\n", "\t#define world_here\n");
        harness.assert_row(1, 2, 2, "\t// Comment\n", "\t// Comment\n");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::cached_file::CachedFile;
    use crate::diff_ir::DiffIR;
    use crate::lexer::{LEXER_MODE_DEFAULT, LexerDefault, RawToken};
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
            &DiffBuilderOptions {
                ignore_whitespace: false,
                ghost_rows: false,
                ..Default::default()
            },
            f1.metadata.num_lines().max(f2.metadata.num_lines()),
        );

        let mut left_res = String::new();
        let mut right_res = String::new();

        for row in rows {
            if let LineContent::Code { tokens, .. } = row.left {
                for (res, _) in tokens {
                    if res.operation != DiffOp::Insert {
                        let idx = res.token_source_idx.expect("Source index missing");
                        left_res
                            .push_str(&f1.contents[f1.tokens[idx as usize].as_ref().span.clone()]);
                    }
                }
            }
            if let LineContent::Code { tokens, .. } = row.right {
                for (res, _) in tokens {
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
