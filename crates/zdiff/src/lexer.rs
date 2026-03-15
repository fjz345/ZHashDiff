use std::{marker::PhantomData, ops::Range};

pub trait RawTokenTrait: Clone + AsRef<RawToken> + From<RawToken> + Send + Sync + 'static {}
impl<T> RawTokenTrait for T where T: Clone + AsRef<RawToken> + From<RawToken> + Send + Sync + 'static
{}

#[derive(Debug, PartialEq, Copy, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum TokenKind {
    Unknown,
    Identifier,
    Number,
    String,
    Symbol,
    Whitespace,
    Tab,
    Comment,
    Newline,
    Keyword,
    Preprocessor,
}

impl TokenKind {
    pub fn is_keyword(&self) -> bool {
        matches!(self, TokenKind::Keyword)
    }
    pub fn is_whitespace(&self) -> bool {
        matches!(
            self,
            TokenKind::Whitespace | TokenKind::Tab | TokenKind::Newline
        )
    }
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RawToken {
    pub kind: TokenKind,
    pub span: Range<usize>,
}

impl AsRef<RawToken> for RawToken {
    fn as_ref(&self) -> &RawToken {
        &self
    }
}

impl<T: RawTokenTrait> Lexer<'_, T> {
    pub fn read_content_span(&self, span: Range<usize>) -> &str {
        &self.source[span]
    }
}

#[derive(Debug, Clone)]
pub struct Lexer<'a, T: RawTokenTrait> {
    source: &'a str,
    cursor: usize,
    phantom_data: PhantomData<T>,
}

impl<'a, T: RawTokenTrait> Lexer<'a, T> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            cursor: 0,
            phantom_data: PhantomData,
        }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.cursor..].chars().next()
    }

    fn consume(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.cursor += c.len_utf8();
        Some(c)
    }

    pub fn parse(&mut self) -> Vec<T> {
        self.map(T::from) // Convert RawToken -> T
            .collect()
    }

    pub fn token_value(&self, token: &T) -> &str {
        &self.source[token.as_ref().span.clone()]
    }

    pub fn reconstruct_source(&self, tokens: &[T]) -> String {
        tokens.iter().map(|t| self.token_value(t)).collect()
    }
}

const KEYWORDS: &[&str] = &[
    "abstract",
    "as",
    "async",
    "await",
    "become",
    "bool",
    "box",
    "break",
    "byte",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "crate",
    "default",
    "do",
    "dyn",
    "else",
    "enum",
    "extern",
    "false",
    "final",
    "fn",
    "for",
    "if",
    "impl",
    "in",
    "interface",
    "let",
    "loop",
    "macro",
    "macro_rules",
    "match",
    "mod",
    "move",
    "mut",
    "new",
    "override",
    "priv",
    "pub",
    "ref",
    "return",
    "self",
    "Self",
    "static",
    "struct",
    "super",
    "switch",
    "throw",
    "trait",
    "true",
    "try",
    "type",
    "typeof",
    "union",
    "unsafe",
    "unsized",
    "use",
    "virtual",
    "where",
    "while",
    "yield",
];

impl<'a, T: RawTokenTrait + From<RawToken>> Iterator for Lexer<'a, T> {
    type Item = RawToken;

    fn next(&mut self) -> Option<Self::Item> {
        let c = self.peek()?;
        let start = self.cursor;

        let kind = match c {
            '\r' => {
                self.consume();
                if self.peek() == Some('\n') {
                    self.consume();
                }
                TokenKind::Newline
            }
            '\n' => {
                self.consume();
                TokenKind::Newline
            }
            '\t' => {
                self.consume();
                TokenKind::Tab
            }
            _ if c.is_whitespace() => {
                self.consume();
                while self.peek().map_or(false, |next| {
                    next.is_whitespace() && next != '\n' && next != '\r'
                }) {
                    self.consume();
                }
                TokenKind::Whitespace
            }
            '/' if self.source[self.cursor..].starts_with("//") => {
                self.consume(); // /
                self.consume(); // /
                while let Some(next_c) = self.peek() {
                    if next_c == '\n' {
                        break;
                    }
                    self.consume();
                }
                TokenKind::Comment
            }
            '/' if self.source[self.cursor..].starts_with("/*") => {
                self.consume(); // /
                self.consume(); // *
                while let Some(next_c) = self.peek() {
                    if next_c == '*' && self.source[self.cursor..].starts_with("*/") {
                        self.consume(); // *
                        self.consume(); // /
                        break;
                    }
                    self.consume();
                }
                TokenKind::Comment
            }
            '"' => {
                self.consume();
                while let Some(next_c) = self.peek() {
                    if next_c == '"' {
                        self.consume();
                        break;
                    }
                    self.consume();
                }
                TokenKind::String
            }
            _ if c.is_alphabetic()
                || c == '_'
                || (c > '\x7f' && !c.is_control() && !c.is_whitespace()) =>
            {
                let start = self.cursor;
                self.consume();

                while self.peek().map_or(false, |next| {
                    next.is_alphanumeric()
                        || next == '_'
                        || (next > '\x7f' && !next.is_control() && !next.is_whitespace())
                }) {
                    self.consume();
                }

                let word = &self.source[start..self.cursor];
                if KEYWORDS.binary_search(&word).is_ok() {
                    TokenKind::Keyword
                } else {
                    TokenKind::Identifier
                }
            }
            _ if c.is_digit(10) => {
                self.consume();
                while self.peek().map_or(false, |next| next.is_digit(10)) {
                    self.consume();
                }
                TokenKind::Number
            }
            '#' => {
                self.consume();
                while let Some(next_c) = self.peek() {
                    if next_c == '\n' || next_c == '\r' {
                        break;
                    }
                    self.consume();
                }
                TokenKind::Preprocessor
            }
            _ if "!@#$%^&*()-=+[]{}|;:'<>,.?/".contains(c) => {
                let start_index = self.cursor;

                let operators = [
                    ">>", "<<", "==", "!=", ">=", "<=", "&&", "||", "->", "::", "+=", "-=", "*=",
                    "/=",
                ];

                let mut matched_operator = false;
                for op in operators {
                    if self.source[start_index..].starts_with(op) {
                        for _ in 0..op.len() {
                            self.consume();
                        }
                        matched_operator = true;
                        break;
                    }
                }

                if !matched_operator {
                    self.consume();
                }
                TokenKind::Symbol
            }
            _ => {
                self.consume();
                TokenKind::Unknown
            }
        };

        Some(RawToken {
            kind,
            span: start..self.cursor,
        })
    }
}

pub fn visualize_diff_grid<'a, T: RawTokenTrait + From<RawToken>>(
    lexer1: &Lexer<'a, T>,
    tokens1: &[T],
    lexer2: &Lexer<'a, T>,
    tokens2: &[T],
) {
    let n = tokens1.len();
    let m = tokens2.len();
    let col_w = 8;

    // ANSI Colors
    let blue = "\x1b[34m";
    let green = "\x1b[32m";
    let gray = "\x1b[90m";
    let reset = "\x1b[0m";

    let label = |val: &str| {
        let escaped = val.replace("\n", "\\n").replace(" ", "·");
        if escaped.len() > col_w - 1 {
            format!("{}…", &escaped[..col_w - 2])
        } else {
            escaped
        }
    };

    // 1. Horizontal Header
    print!("{:>width$} ", "", width = col_w);
    for t in tokens1 {
        print!(
            " {}{:^width$}{}",
            blue,
            label(lexer1.token_value(t)),
            reset,
            width = col_w - 1
        );
    }
    println!("\n");

    for j in 0..=m {
        // --- Line A: Nodes and Horizontal Edges ---
        print!("{:>width$} ", "", width = col_w);
        for i in 0..=n {
            print!("{}┼{}", gray, reset);
            if i < n {
                print!("{}{}{}", gray, "─".repeat(col_w - 1), reset);
            }
        }
        println!();

        // --- Line B: Vertical Edges, Diagonals, and Vertical Labels ---
        if j < m {
            print!(
                "{}{:>width$}{} ",
                blue,
                label(lexer2.token_value(&tokens2[j])),
                reset,
                width = col_w
            );

            for i in 0..=n {
                print!("{}│{}", gray, reset);
                if i < n {
                    let is_match = tokens1[i].as_ref().kind == tokens2[j].as_ref().kind
                        && lexer1.token_value(&tokens1[i]) == lexer2.token_value(&tokens2[j]);

                    if is_match {
                        let pad = (col_w - 2) / 2;
                        print!(
                            "{}{}{}{}{}{}",
                            " ".repeat(pad),
                            green,
                            "\\",
                            reset,
                            " ".repeat(col_w - 2 - pad),
                            ""
                        );
                    } else {
                        print!("{}", " ".repeat(col_w - 1));
                    }
                }
            }
            println!();
        }
    }
}

pub fn visualize_diff_grid_with_path<'a, F, T: RawTokenTrait + From<RawToken>>(
    lexer1: &Lexer<'a, T>,
    tokens1: &[T],
    lexer2: &Lexer<'a, T>,
    tokens2: &[T],
    path: &[(i32, i32)],
    mut cmp: F,
) where
    F: FnMut(&T, &T) -> bool,
{
    let (n, m) = (tokens1.len() as i32, tokens2.len() as i32);
    let col_w = 8;
    let (blue, green, gray, yellow, reset) =
        ("\x1b[34m", "\x1b[32m", "\x1b[90m", "\x1b[33m", "\x1b[0m");

    let is_on_path = |x: i32, y: i32| path.contains(&(x, y));

    let label = |val: &str| {
        let escaped = val.replace("\n", "\\n").replace(" ", "·");
        if escaped.len() > col_w - 1 {
            format!("{}…", &escaped[..col_w - 2])
        } else {
            escaped
        }
    };

    // 1. Horizontal Header
    print!("{:>width$} ", "", width = col_w);
    for t in tokens1 {
        print!(
            " {:^width$}",
            format!("{}{}{}", blue, label(lexer1.token_value(t)), reset),
            width = col_w + 8
        );
    }
    println!("\n");

    for j in 0..=m {
        // --- Line A: Nodes and Horizontal Edges (Deletions) ---
        print!("{:>width$} ", "", width = col_w);
        for i in 0..=n {
            let node = if is_on_path(i, j) {
                format!("{}█{}", yellow, reset)
            } else {
                format!("{}┼{}", gray, reset)
            };
            print!("{}", node);

            if i < n {
                let on_path = is_on_path(i, j) && is_on_path(i + 1, j);
                let color = if on_path { yellow } else { gray };
                print!("{}{}{}", color, "─".repeat(col_w - 1), reset);
            }
        }
        println!();

        // --- Line B: Vertical Edges (Insertions), Diagonals (Matches), and Labels ---
        if j < m {
            print!(
                "{}{:>width$}{} ",
                blue,
                label(lexer2.token_value(&tokens2[j as usize])),
                reset,
                width = col_w
            );

            for i in 0..=n {
                let on_path_v = is_on_path(i, j) && is_on_path(i, j + 1);
                let v_color = if on_path_v { yellow } else { gray };
                print!("{}│{}", v_color, reset);

                if i < n {
                    let is_match = cmp(&tokens1[i as usize], &tokens2[j as usize]);
                    let on_diag_path = is_on_path(i, j) && is_on_path(i + 1, j + 1);

                    if is_match {
                        let color = if on_diag_path { green } else { gray };
                        let pad = (col_w - 2) / 2;
                        print!(
                            "{}{}{}{}{}",
                            " ".repeat(pad),
                            color,
                            "\\",
                            reset,
                            " ".repeat(col_w - 2 - pad)
                        );
                    } else {
                        print!("{}", " ".repeat(col_w - 1));
                    }
                }
            }
            println!();
        }
    }
}
