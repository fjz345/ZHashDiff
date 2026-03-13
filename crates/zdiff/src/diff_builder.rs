use eframe::egui;
use egui::Color32;
use serde::{Deserialize, Serialize};

use crate::lexer::{Lexer, RawToken, TokenKind};

#[derive(Debug, Clone)]
pub struct DiffRow {
    pub left: LineContent,
    pub right: LineContent,
}

#[derive(Debug, Clone)]
pub enum LineContent {
    Code {
        tokens: Vec<(String, egui::Color32)>,
        line_num: i32,
        bg: egui::Color32,
    },
    Void,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
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

enum DiffOp<'a> {
    Match { 
        t1: &'a RawToken, v1: &'a str, 
        t2: &'a RawToken, v2: &'a str 
    },
    Deletion { t: &'a RawToken, v: &'a str },
    Insertion { t: &'a RawToken, v: &'a str },
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
            ghost: Color32::from_rgba_unmultiplied(150, 150, 150, 80),
            kw: Color32::from_rgb(86, 156, 214),
            del: Color32::from_rgb(255, 100, 100),
            ins: Color32::from_rgb(100, 255, 100),
            del_bg: Color32::from_rgba_unmultiplied(255, 0, 0, 20),
            ins_bg: Color32::from_rgba_unmultiplied(0, 255, 0, 20),
        }
    }
}

struct SideState {
    buf: Vec<(String, Color32, bool)>, // String, Color, is_whitespace
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

    fn push(&mut self, val: String, color: Color32, is_ws: bool) {
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
                bg: if has_diff { bg_color } else { Color32::TRANSPARENT },
            }
        }
    }
}

pub struct DiffBuilder<'a> {
    options: &'a DiffBuilderOptions,
    theme: DiffTheme,
    rows: Vec<DiffRow>,
    left: SideState,
    right: SideState,
}

impl<'a> DiffBuilder<'a> {
    pub fn new(options: &'a DiffBuilderOptions) -> Self {
        Self {
            options,
            theme: DiffTheme::default(),
            rows: Vec::new(),
            left: SideState::new(),
            right: SideState::new(),
        }
    }

    fn get_color(&self, tok: &RawToken, is_keyword: bool) -> Color32 {
        if self.options.keyword_highlight && is_keyword { self.theme.kw } else { Color32::GRAY }
    }

    pub fn handle_match(&mut self, t1: &RawToken, v1: &str, t2: &RawToken, v2: &str) {
        let c1 = self.get_color(t1, t1.kind.is_keyword());
        let c2 = self.get_color(t2, t2.kind.is_keyword());

        self.left.push(v1.to_string(), c1, t1.kind.is_whitespace());
        self.right.push(v2.to_string(), c2, t2.kind.is_whitespace());

        if t1.kind == TokenKind::Newline {
            self.emit_row();
        }
    }

    pub fn handle_diff(&mut self, tok: &RawToken, val: &str, is_deletion: bool) {
        let ws = tok.kind.is_whitespace();
        if !self.options.ignore_whitespace || !ws {
            if is_deletion { self.left.active_diff = true; } else { self.right.active_diff = true; }
        }

        let target = if is_deletion { &mut self.left } else { &mut self.right };
        let color = if is_deletion { self.theme.del } else { self.theme.ins };
        target.push(val.to_string(), color, ws);

        if tok.kind == TokenKind::Newline {
            self.emit_row();
        }
    }

    fn emit_row(&mut self) {
        // Apply ghosting only if one side is empty and the other has content
        if self.options.ghost_rows {
            self.apply_ghosts();
        }

        let hi = self.options.highlight_rows;
        let left_row = self.left.flush(self.left.active_diff && hi, self.theme.del_bg);
        let right_row = self.right.flush(self.right.active_diff && hi, self.theme.ins_bg);

        // Increment line numbers for any side that produced a row (real or ghost)
        if !matches!(left_row, LineContent::Void) { self.left.line_num += 1; }
        if !matches!(right_row, LineContent::Void) { self.right.line_num += 1; }

        self.rows.push(DiffRow { left: left_row, right: right_row });
        self.left.active_diff = false;
        self.right.active_diff = false;
    }

    fn apply_ghosts(&mut self) {
        let l_empty = self.left.buf.is_empty();
        let r_empty = self.right.buf.is_empty();

        if l_empty && !r_empty {
            let mut started = false;
            for (val, _, is_ws) in &self.right.buf {
                let color = if *is_ws && !started { Color32::TRANSPARENT } else { self.theme.ghost };
                if !*is_ws { started = true; }
                self.left.buf.push((val.clone(), color, *is_ws));
            }
        } else if r_empty && !l_empty {
            let mut started = false;
            for (val, _, is_ws) in &self.left.buf {
                let color = if *is_ws && !started { Color32::TRANSPARENT } else { self.theme.ghost };
                if !*is_ws { started = true; }
                self.right.buf.push((val.clone(), color, *is_ws));
            }
        }
    }

    pub fn finish(self) -> Vec<DiffRow> {
        self.rows
    }
}

pub fn build_diff_rows(
    path: &[(i32, i32)],
    t1: &[RawToken],
    t2: &[RawToken],
    lex1: &Lexer,
    lex2: &Lexer,
    options: &DiffBuilderOptions,
) -> Vec<DiffRow> {
    let mut builder = DiffBuilder::new(options);

    for window in path.windows(2) {
        let (x1, y1) = (window[0].0 as usize, window[0].1 as usize);
        let (x2, y2) = (window[1].0 as usize, window[1].1 as usize);

        if x2 > x1 && y2 > y1 {
            builder.handle_match(&t1[x1], lex1.token_value(&t1[x1]), &t2[y1], lex2.token_value(&t2[y1]));
        } else {
            let is_del = x2 > x1;
            let (tok, lex) = if is_del { (&t1[x1], lex1) } else { (&t2[y1], lex2) };
            builder.handle_diff(tok, lex.token_value(tok), is_del);
        }
    }

    builder.finish()
}

pub fn build_single_file_rows(
    tokens: &[RawToken],
    lexer: &Lexer,
    options: &DiffBuilderOptions,
    is_left_side: bool,
) -> Vec<DiffRow> {
    let n = tokens.len() as i32;
    // Create a path that only moves in the direction of the provided file
    let path: Vec<(i32, i32)> = if is_left_side {
        (0..=n).map(|i| (i, 0)).collect()
    } else {
        (0..=n).map(|i| (0, i)).collect()
    };

    let empty = Vec::new();
    let empty_lex = Lexer::new("");

    if is_left_side {
        build_diff_rows(&path, tokens, &empty, lexer, &empty_lex, options)
    } else {
        build_diff_rows(&path, &empty, tokens, &empty_lex, lexer, options)
    }
}