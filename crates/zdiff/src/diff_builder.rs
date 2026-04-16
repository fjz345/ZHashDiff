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

        self.left.push(
            diff_result.clone(),
            color,
            token.as_ref().kind.is_whitespace(),
        );
        self.right
            .push(diff_result, color, token.as_ref().kind.is_whitespace());

        if token.as_ref().kind == TokenKind::Newline {
            self.emit_row();
        }
    }

    pub fn handle_diff(&mut self, diff_result: DiffResult, is_deletion: bool) {
        assert!(matches!(
            diff_result.operation,
            DiffOp::Delete | DiffOp::Insert
        ));
        let token = if diff_result.operation == DiffOp::Delete {
            &self.tokens_source.expect("Source is None")[diff_result.token_idx as usize]
        } else {
            &self.tokens_target.expect("Target is none")[diff_result.token_idx as usize]
        };
        let ws = token.as_ref().kind.is_whitespace();
        if !self.options.ignore_whitespace || !ws {
            if is_deletion {
                self.left.active_diff = true;
            } else {
                self.right.active_diff = true;
            }
        }

        let target = if is_deletion {
            &mut self.left
        } else {
            &mut self.right
        };
        let color = if is_deletion {
            self.theme.del
        } else {
            self.theme.ins
        };

        target.push(diff_result.clone(), color, ws);

        if token.as_ref().kind == TokenKind::Newline {
            self.emit_row();
        }
    }

    fn emit_row(&mut self) {
        if self.options.ghost_rows {
            self.apply_ghosts();
        }

        let hi = self.options.highlight_rows;
        let left_row = self
            .left
            .flush(self.left.active_diff && hi, self.theme.del_bg);
        let right_row = self
            .right
            .flush(self.right.active_diff && hi, self.theme.ins_bg);

        if !matches!(left_row, LineContent::Void) {
            self.left.line_num += 1;
        }
        if !matches!(right_row, LineContent::Void) {
            self.right.line_num += 1;
        }

        self.rows.push(DiffRow {
            left: left_row,
            right: right_row,
        });
        self.left.active_diff = false;
        self.right.active_diff = false;
    }

    fn apply_ghosts(&mut self) {
        let l_empty = self.left.buf.is_empty();
        let r_empty = self.right.buf.is_empty();

        if l_empty && !r_empty {
            let mut started = false;
            for (val, _, is_ws) in &self.right.buf {
                let color = if *is_ws && !started {
                    Color32::BLACK
                } else {
                    self.theme.ghost
                };
                if !*is_ws {
                    started = true;
                }
                self.left.buf.push((val.clone(), color, *is_ws));
            }
        } else if r_empty && !l_empty {
            let mut started = false;
            for (val, _, is_ws) in &self.left.buf {
                let color = if *is_ws && !started {
                    Color32::TRANSPARENT
                } else {
                    self.theme.ghost
                };
                if !*is_ws {
                    started = true;
                }
                self.right.buf.push((val.clone(), color, *is_ws));
            }
        }
    }

    pub fn finish(mut self) -> Vec<DiffRow> {
        if !self.left.buf.is_empty() || !self.right.buf.is_empty() {
            self.emit_row();
        }

        self.rows
    }
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
    use crate::{
        diff_ir::{self, generate_ir},
        lexer::RawToken,
    };

    use super::*;

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
        let get_data = |content: &LineContent,
                        source_text: &str,
                        target_text: &str,
                        source_tokens: &[RawToken],
                        target_tokens: &[RawToken]| match content {
            LineContent::Code {
                tokens, line_num, ..
            } => {
                let text = tokens
                    .iter()
                    .map(|(res, _)| {
                        let text = match res.operation {
                            diff_ir::DiffOp::Equal | diff_ir::DiffOp::Delete => {
                                let token = &source_tokens[res.token_idx as usize];
                                &source_text[token.as_ref().span.clone()]
                            }
                            diff_ir::DiffOp::Insert => {
                                let token = &target_tokens[res.token_idx as usize];
                                &target_text[token.as_ref().span.clone()]
                            }
                        };
                        text
                    })
                    .collect::<String>();
                (text, *line_num)
            }
            LineContent::Void => ("VOID".to_string(), -1),
        };

        let (act_l_text, act_l_num) = get_data(&row.left, s1, s2, l_tokens, r_tokens);
        let (act_r_text, act_r_num) = get_data(&row.right, s1, s2, l_tokens, r_tokens);

        if act_l_text != l_text
            || act_r_text != r_text
            || act_l_num != l_line
            || act_r_num != r_line
        {
            let mut report = String::new();
            report.push_str(&format!("\nFAIL: Row Index {}\n", idx));
            report.push_str(&format!(
                "{:<5} | {:<5} | {:<40} | {:<5} | {:<40}\n",
                "SIDE", "L-NUM", "LEFT TEXT", "R-NUM", "RIGHT TEXT"
            ));
            report.push_str(&"-".repeat(105));
            report.push('\n');

            report.push_str(&format!(
                "{:<5} | {:<5} | {:<40?} | {:<5} | {:<40?}\n",
                "EXP", l_line, l_text, r_line, r_text
            ));
            report.push_str(&format!(
                "{:<5} | {:<5} | {:<40?} | {:<5} | {:<40?}\n",
                "ACT", act_l_num, act_l_text, act_r_num, act_r_text
            ));

            panic!("{}", report);
        }
    }

    #[test]
    fn test_build_diff_rows_header_edit() {
        let s1 = "\t#define hello_there\n\t// Comment\n";
        let s2 = "\t#define world_here\n\t// Comment\n";

        let mut lex1 = Lexer::<RawToken>::new(s1);
        let mut lex2 = Lexer::<RawToken>::new(s2);
        let t1 = lex1.parse();
        let t2 = lex2.parse();

        // path: (x, y)
        let path = vec![
            (0, 0),
            (1, 1), // \t match
            (2, 1), // Del hello_there
            (2, 2), // Ins world_here
            (3, 3),
            (4, 4),
            (5, 5),
            (6, 6),
        ];

        let options = DiffBuilderOptions {
            keyword_highlight: true,
            highlight_rows: true,
            ghost_rows: false,
            ignore_whitespace: false,
        };

        let diff_ir = generate_ir(&path);
        let rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options);

        println!("\n--- BUILT DIFF ROWS VISUALIZATION ---");
        println!(
            "{:<3} | {:<5} | {:<30} | {:<5} | {:<30}",
            "IDX", "L#", "LEFT", "R#", "RIGHT"
        );

        for (i, row) in rows.iter().enumerate() {
            let l_disp = match &row.left {
                LineContent::Code { tokens, .. } => {
                    let collected_tokens: Vec<_> = tokens.iter().map(|(s, _)| s.clone()).collect();
                    format!("{:?}", collected_tokens)
                }
                _ => "VOID".into(),
            };

            let r_disp = match &row.right {
                LineContent::Code { tokens, .. } => {
                    let collected_tokens: Vec<_> = tokens.iter().map(|(s, _)| s.clone()).collect();
                    format!("{:?}", collected_tokens)
                }
                _ => "VOID".into(),
            };

            println!(
                "{:<3} | {:<5} | {:<30} | {:<5} | {:<30}",
                i,
                i + 1,
                l_disp,
                i + 1,
                r_disp
            );
        }

        assert_row_content(
            0,
            &rows[0],
            1,
            1,
            "\t#define hello_there\n",
            "\t#define world_here\n",
            &t1,
            &t2,
            &s1,
            &s2,
        );
        assert_row_content(
            1,
            &rows[1],
            2,
            2,
            "\t// Comment\n",
            "\t// Comment\n",
            &t1,
            &t2,
            &s1,
            &s2,
        );
    }

    #[test]
    fn test_build_diff_rows_ghost_enabled() {
        let s1 = "deleted_line\nmatch\n";
        let s2 = "match\n";

        let mut lex1 = Lexer::<RawToken>::new(s1);
        let mut lex2 = Lexer::<RawToken>::new(s2);
        let t1 = lex1.parse();
        let t2 = lex2.parse();

        // path: (x, y)
        // 0,0 -> 2,0 : Delete "deleted_line" and "\n" from left
        // 2,0 -> 4,2 : Match "match" and "\n"
        let path = vec![
            (0, 0),
            (1, 0),
            (2, 0), // Delete "deleted_line", "\n"
            (3, 1),
            (4, 2), // Match "match", "\n"
        ];

        let options = DiffBuilderOptions {
            keyword_highlight: true,
            highlight_rows: true,
            ghost_rows: true,
            ignore_whitespace: false,
        };

        let diff_ir = generate_ir(&path);
        let rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options);

        println!("\n--- GHOST ROWS VISUALIZATION ---");
        println!(
            "{:<3} | {:<5} | {:<30} | {:<5} | {:<30}",
            "IDX", "L#", "LEFT (REAL/GHOST)", "R#", "RIGHT (GHOST/REAL)"
        );
        for (i, row) in rows.iter().enumerate() {
            let (l_text, l_num) = match &row.left {
                LineContent::Code {
                    tokens, line_num, ..
                } => (
                    // Collect into a Vec to satisfy Debug {:?} formatting
                    tokens.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>(),
                    *line_num,
                ),
                _ => (vec![], -1), // Match type with a Vec
            };

            let (r_text, r_num) = match &row.right {
                LineContent::Code {
                    tokens, line_num, ..
                } => (
                    tokens.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>(),
                    *line_num,
                ),
                _ => (vec![], -1),
            };

            println!(
                "{:<3} | {:<5} | {:<30?} | {:<5} | {:<30?}",
                i, l_num, l_text, r_num, r_text
            );
        }

        assert_row_content(
            0,
            &rows[0],
            1,
            1,
            "deleted_line\n",
            "deleted_line\n",
            &t1,
            &t2,
            &s1,
            &s2,
        );

        assert_row_content(1, &rows[1], 2, 2, "match\n", "match\n", &t1, &t2, &s1, &s2);
    }

    #[test]
    fn test_build_diff_rows_ignore_whitespace() {
        let s1 = "ImGuiChildFlags_Border\n";
        let s2 = "ImGuiChildFlags_Borders,  // Renamed in 1.91.1\n";

        let mut lex1 = Lexer::<RawToken>::new(s1);
        let mut lex2 = Lexer::<RawToken>::new(s2);
        let t1 = lex1.parse();
        let t2 = lex2.parse();

        // path: (x, y)
        let path = vec![
            (0, 0),
            (1, 0), // Delete "ImGuiChildFlags_Border"
            (1, 1), // Insert "ImGuiChildFlags_Borders"
            (1, 2), // Insert ","
            (1, 3), // Insert "  " (Whitespace)
            (1, 4), // Insert "// Renamed..." (Comment)
            (2, 5), // Match "\n"
        ];

        let options = DiffBuilderOptions {
            keyword_highlight: true,
            highlight_rows: true,
            ghost_rows: true,
            ignore_whitespace: true,
        };

        let diff_ir = generate_ir(&path);
        let rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options);

        println!("\n--- WHITESPACE IGNORE VISUALIZATION ---");
        for (i, row) in rows.iter().enumerate() {
            let l_text = match &row.left {
                LineContent::Code { tokens, .. } => {
                    tokens
                        .iter()
                        .map(|(s, _)| {
                            // Replace 'token' with the actual field name in DiffResult
                            // that contains your RawToken/string data
                            format!("{:?}", s)
                        })
                        .collect::<String>()
                }
                _ => "VOID".into(),
            };

            let r_text = match &row.right {
                LineContent::Code { tokens, .. } => {
                    tokens
                        .iter()
                        .map(|(s, _)| {
                            // Replace 'token' with the actual field name in DiffResult
                            // that contains your RawToken/string data
                            format!("{:?}", s)
                        })
                        .collect::<String>()
                }
                _ => "VOID".into(),
            };

            // ... repeat for r_text ...
        }

        assert_eq!(
            rows.len(),
            1,
            "Should have collapsed the diff into a single row"
        );

        assert_row_content(
            0,
            &rows[0],
            1,
            1,
            "ImGuiChildFlags_Border\n",
            "ImGuiChildFlags_Borders,  // Renamed in 1.91.1\n",
            &t1,
            &t2,
            &s1,
            &s2,
        );
    }
}
