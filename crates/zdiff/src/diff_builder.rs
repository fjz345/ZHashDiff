use std::{
    io,
    ops::Range,
    path::{Path, PathBuf},
};

use crate::{
    diff_ir::{DiffIR, DiffOp, DiffResult},
    hash::hash_file,
    lexer::{Lexer, RawToken, RawTokenTrait, TokenKind},
    read_file_contents,
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
pub struct CachedFile<T: RawTokenTrait> {
    pub path: PathBuf,
    pub hash: String,
    pub contents: String,
    pub tokens: Vec<T>,
}

impl<T: RawTokenTrait> CachedFile<T> {
    pub fn read_content_span(&self, span: Range<usize>) -> &str {
        &self.contents[span]
    }
}

impl<T: RawTokenTrait> CachedFile<T> {
    pub fn new(path: impl AsRef<Path>) -> io::Result<Self> {
        let contents = read_file_contents(&path)?;
        let hash = hash_file(&path)?;
        let tokens = Lexer::<T>::new(&contents).map(T::from).collect();
        let path = path.as_ref().to_path_buf();
        Ok(Self {
            path,
            hash,
            contents,
            tokens,
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
        // Apply ghosting only if one side is empty and the other has content
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

        // Increment line numbers for any side that produced a row (real or ghost)
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

    pub fn finish(self) -> Vec<DiffRow> {
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
