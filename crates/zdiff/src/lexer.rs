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
    CommentStart,
    CommentEnd,
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
    pub fn is_comment(&self) -> bool {
        matches!(
            self,
            TokenKind::Comment | TokenKind::CommentStart | TokenKind::CommentEnd
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

impl<const LEXER_MODE: u8, T: RawTokenTrait> Lexer<'_, LEXER_MODE, T> {
    pub fn read_content_span(&self, span: Range<usize>) -> &str {
        &self.source[span]
    }
}

pub const LEXER_MODE_DEFAULT: u8 = LEXER_MODE_GREEDY;
pub const LEXER_MODE_GREEDY: u8 = 1; // longest possible token lengths to reduce number of tokens
pub const LEXER_MODE_TOKENIZE: u8 = 2; // Try to tokenize as much as possible
pub const LEXER_MODE_NEWLINE: u8 = 3; // Handle newlines specifically
pub type LexerDefault<'a, T> = Lexer<'a, LEXER_MODE_DEFAULT, T>;
pub type LexerGreedy<'a, T> = Lexer<'a, LEXER_MODE_GREEDY, T>;
pub type LexerTokenize<'a, T> = Lexer<'a, LEXER_MODE_TOKENIZE, T>;
pub type LexerNewLine<'a, T> = Lexer<'a, LEXER_MODE_NEWLINE, T>;

/// What the cursor is inside, carried across tokens and lines. String literals never span a
/// line, so they are lexed in one go and need no state.
#[derive(Debug, Clone, Copy, PartialEq)]
enum LexState {
    Code,
    /// Only entered in TOKENIZE mode; the other modes consume a line comment as one token.
    LineComment,
    BlockComment,
}

#[derive(Debug, Clone)]
pub struct Lexer<'a, const LEXER_MODE: u8, T: RawTokenTrait> {
    source: &'a str,
    cursor: usize,
    state: LexState,
    phantom_data: PhantomData<T>,
}

impl<'a, const LEXER_MODE: u8, T: RawTokenTrait> Lexer<'a, LEXER_MODE, T> {
    pub fn new(source: &'a str) -> Self {
        Self {
            source,
            cursor: 0,
            state: LexState::Code,
            phantom_data: PhantomData,
        }
    }

    fn rest(&self) -> &'a str {
        &self.source[self.cursor..]
    }

    fn at_line_end(&self) -> bool {
        matches!(self.peek(), None | Some('\r' | '\n'))
    }

    fn consume_bytes(&mut self, len: usize) {
        self.cursor += len;
        debug_assert!(self.source.is_char_boundary(self.cursor));
    }

    /// Consumes a literal at the cursor if one starts there.
    fn consume_literal(&mut self) -> bool {
        match literal_len(self.rest()) {
            Some(len) => {
                self.consume_bytes(len);
                true
            }
            None => false,
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
        self.map(T::from).collect()
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

/// Byte length of the literal starting at `rest` (which starts with `"` or `'`), or None if no
/// literal starts there.
/// - `"` always starts one. Backslash escapes the next char, except a line break. A string with no
///   closing quote ends before the line break.
/// - `'` starts one only when it closes after exactly one char or one escape (`'x'`, `'\''`,
///   `'\x7f'`, `'\u{1F600}'`). Otherwise it is not a literal, so Rust lifetimes (`'a`) and
///   apostrophes in prose don't turn the rest of the line into a string.
fn literal_len(rest: &str) -> Option<usize> {
    let is_line_break = |c: char| c == '\r' || c == '\n';
    let mut chars = rest.char_indices();
    match chars.next()?.1 {
        '"' => {
            while let Some((i, c)) = chars.next() {
                match c {
                    _ if is_line_break(c) => return Some(i),
                    '"' => return Some(i + 1),
                    '\\' => {
                        if chars.clone().next().is_some_and(|(_, n)| !is_line_break(n)) {
                            chars.next();
                        }
                    }
                    _ => {}
                }
            }
            Some(rest.len())
        }
        '\'' => match chars.next()?.1 {
            '\'' => None,
            c if is_line_break(c) => None,
            '\\' => {
                if is_line_break(chars.next()?.1) {
                    return None;
                }
                // The tail of a hex, octal or unicode escape, then the closing quote.
                for (i, c) in chars {
                    match c {
                        '\'' => return Some(i + 1),
                        _ if c.is_ascii_hexdigit() || c == '{' || c == '}' => {}
                        _ => return None,
                    }
                }
                None
            }
            _ => {
                let (i, c) = chars.next()?;
                (c == '\'').then_some(i + 1)
            }
        },
        _ => None,
    }
}

impl<'a, const LEXER_MODE: u8, T: RawTokenTrait + From<RawToken>> Iterator
    for Lexer<'a, LEXER_MODE, T>
{
    type Item = RawToken;

    fn next(&mut self) -> Option<Self::Item> {
        self.peek()?;
        let start = self.cursor;
        let kind = if LEXER_MODE == LEXER_MODE_NEWLINE {
            self.next_line_segment()
        } else {
            match self.state {
                LexState::Code => self.next_code_token(),
                LexState::LineComment | LexState::BlockComment => self.next_comment_token(),
            }
        };
        Some(RawToken {
            kind,
            span: start..self.cursor,
        })
    }
}

impl<'a, const LEXER_MODE: u8, T: RawTokenTrait> Lexer<'a, LEXER_MODE, T> {
    /// NEWLINE mode: a line is split only where a comment starts or ends. Code (string literals
    /// included) is one String token, each comment part is one Comment token.
    fn next_line_segment(&mut self) -> TokenKind {
        if let Some(kind) = self.lex_newline() {
            return kind;
        }
        if self.state == LexState::Code {
            if self.rest().starts_with("//") {
                while !self.at_line_end() {
                    self.consume();
                }
                return TokenKind::Comment;
            }
            if self.rest().starts_with("/*") {
                self.consume_bytes(2);
                self.state = LexState::BlockComment;
            } else {
                while !self.at_line_end()
                    && !self.rest().starts_with("//")
                    && !self.rest().starts_with("/*")
                {
                    if !self.consume_literal() {
                        self.consume();
                    }
                }
                return TokenKind::String;
            }
        }
        debug_assert_eq!(self.state, LexState::BlockComment);
        while !self.at_line_end() {
            if self.rest().starts_with("*/") {
                self.consume_bytes(2);
                self.state = LexState::Code;
                break;
            }
            self.consume();
        }
        TokenKind::Comment
    }

    /// Inside a comment, whitespace keeps its kinds (ignore-whitespace and row building depend on
    /// them) and everything else is Comment. Quotes, `//`, `/*` and `#` are plain text here.
    fn next_comment_token(&mut self) -> TokenKind {
        if let Some(kind) = self.lex_newline() {
            if self.state == LexState::LineComment {
                self.state = LexState::Code;
            }
            return kind;
        }
        if let Some(kind) = self.lex_blank() {
            return kind;
        }
        if self.state == LexState::BlockComment && self.rest().starts_with("*/") {
            self.consume_bytes(2);
            self.state = LexState::Code;
            return TokenKind::CommentEnd;
        }
        self.lex_word_or_symbol();
        TokenKind::Comment
    }

    fn lex_newline(&mut self) -> Option<TokenKind> {
        match self.peek()? {
            '\r' => {
                self.consume();
                if self.peek() == Some('\n') {
                    self.consume();
                }
                Some(TokenKind::Newline)
            }
            '\n' => {
                self.consume();
                Some(TokenKind::Newline)
            }
            _ => None,
        }
    }

    fn lex_blank(&mut self) -> Option<TokenKind> {
        match self.peek()? {
            '\t' => {
                self.consume();
                Some(TokenKind::Tab)
            }
            c if c.is_whitespace() => {
                self.consume();
                if LEXER_MODE == LEXER_MODE_GREEDY {
                    while self.peek().map_or(false, |next| {
                        next.is_whitespace() && next != '\n' && next != '\r'
                    }) {
                        self.consume();
                    }
                }
                Some(TokenKind::Whitespace)
            }
            _ => None,
        }
    }

    fn next_code_token(&mut self) -> TokenKind {
        if let Some(kind) = self.lex_newline().or_else(|| self.lex_blank()) {
            return kind;
        }
        let c = self.peek().expect("next_code_token at end of source");
        match c {
            '/' if self.rest().starts_with("//") => {
                self.consume_bytes(2);
                if LEXER_MODE == LEXER_MODE_GREEDY {
                    while !self.at_line_end() {
                        self.consume();
                    }
                } else {
                    self.state = LexState::LineComment;
                }
                TokenKind::Comment
            }
            '/' if self.rest().starts_with("/*") => {
                self.consume_bytes(2);
                self.state = LexState::BlockComment;
                TokenKind::CommentStart
            }
            // A `*/` with no open comment: kept as CommentEnd, as before; it changes no state.
            '*' if self.rest().starts_with("*/") => {
                self.consume_bytes(2);
                TokenKind::CommentEnd
            }
            '"' | '\'' if self.consume_literal() => TokenKind::String,
            '#' => {
                self.consume();
                if LEXER_MODE == LEXER_MODE_GREEDY {
                    // The directive ends where a comment starts; literals are skipped so a `//`
                    // inside one (`#define URL "http://x"`) doesn't end it.
                    while !self.at_line_end()
                        && !self.rest().starts_with("//")
                        && !self.rest().starts_with("/*")
                    {
                        if !self.consume_literal() {
                            self.consume();
                        }
                    }
                }
                TokenKind::Preprocessor
            }
            _ => self.lex_word_or_symbol(),
        }
    }

    fn lex_word_or_symbol(&mut self) -> TokenKind {
        let c = self.peek().expect("lex_word_or_symbol at end of source");
        match c {
            _ if c.is_alphabetic()
                || c == '_'
                || (c > '\x7f' && !c.is_control() && !c.is_whitespace()) =>
            {
                let start = self.cursor;
                self.consume();

                if LEXER_MODE == LEXER_MODE_GREEDY {
                    while self.peek().map_or(false, |next| {
                        next.is_alphanumeric()
                            || next == '_'
                            || (next > '\x7f' && !next.is_control() && !next.is_whitespace())
                    }) {
                        self.consume();
                    }
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
                if LEXER_MODE == LEXER_MODE_GREEDY {
                    while self.peek().map_or(false, |next| next.is_digit(10)) {
                        self.consume();
                    }
                }
                TokenKind::Number
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
        }
    }
}

pub fn visualize_diff_grid<'a, const LEXER_MODE: u8, T: RawTokenTrait + From<RawToken>>(
    lexer1: &Lexer<'a, LEXER_MODE, T>,
    tokens1: &[T],
    lexer2: &Lexer<'a, LEXER_MODE, T>,
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

pub fn visualize_diff_grid_with_path<
    'a,
    const LEXER_MODE: u8,
    F,
    T: RawTokenTrait + From<RawToken>,
>(
    lexer1: &Lexer<'a, LEXER_MODE, T>,
    tokens1: &[T],
    lexer2: &Lexer<'a, LEXER_MODE, T>,
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

#[cfg(test)]
mod tests {
    use super::*;

    // use std::fs::{self, File};
    // use std::path::Path;
    // use tempfile::{TempDir, tempdir};

    // fn create_file(path: &Path) {
    //     File::create(path).expect("failed to create file");
    // }

    #[test]
    fn test_function_syntax() {
        let cases = [
            // Declarations
            ("C Decl", r#"void main(int argc, char *argv[])"#),
            ("Rust Decl", r#"fn main()"#),
            ("Python Decl", r#"def main(argc, argv):"#),
            // Definitions
            (
                "C Def",
                "void main(int argc, char *argv[])\n{\n    std::out << \"Hello, world!\" << std::endl;\n}",
            ),
            (
                "Rust Def",
                "fn main()\n{\n    println!(\"Hello, world!\");\n}\n",
            ),
            (
                "Python Def",
                "def main(argc, argv):\n    print(\"Hello, world!\")",
            ),
        ];

        for (name, src) in cases {
            let mut lexer = LexerDefault::<RawToken>::new(src);
            let tokens = lexer.parse();

            assert!(
                !tokens.iter().any(|t| matches!(t.kind, TokenKind::Unknown)),
                "[{}] Lexer found unknown tokens: {:?}",
                name,
                tokens
                    .iter()
                    .filter(|t| matches!(t.kind, TokenKind::Unknown))
                    .map(|t| lexer.token_value(t))
                    .collect::<Vec<_>>()
            );

            let reconstructed = lexer.reconstruct_source(&tokens);
            assert_eq!(
                reconstructed, src,
                "[{}] Reconstruction mismatch!\nExpected: {:?}\nGot:      {:?}",
                name, src, reconstructed
            );
        }
    }

    #[test]
    fn test_greedy_operators_exhaustive() {
        let input = "x / / y // comment\nx /* block */ y";
        let mut lexer = Lexer::<LEXER_MODE_DEFAULT, RawToken>::new(input);
        let tokens = lexer.parse();

        // Verification Table:
        // Index | Token Kind | Value
        // -------------------------
        // 0     | Identifier   | "x"
        // 1     | Whitespace   | " "
        // 2     | Symbol       | "/"
        // 3     | Whitespace   | " "
        // 4     | Symbol       | "/"
        // 5     | Whitespace   | " "
        // 6     | Identifier   | "y"
        // 7     | Whitespace   | " "
        // 8     | Comment      | "// comment"
        // 9     | Whitespace   | "\n"
        // 10    | Identifier   | "x"
        // 11    | Whitespace   | " "
        // 12    | CommentStart | "/*"
        // 13    | Whitespace   | " "
        // 14    | Comment      | "block"
        // 15    | Whitespace   | " "
        // 16    | CommentEnd   | "*/"
        // 17    | Whitespace   | " "
        // 18    | Identifier   | "y"

        let expected = vec![
            (TokenKind::Identifier, "x"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Symbol, "/"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Symbol, "/"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Identifier, "y"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Comment, "// comment"),
            (TokenKind::Newline, "\n"),
            (TokenKind::Identifier, "x"),
            (TokenKind::Whitespace, " "),
            (TokenKind::CommentStart, "/*"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Comment, "block"),
            (TokenKind::Whitespace, " "),
            (TokenKind::CommentEnd, "*/"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Identifier, "y"),
        ];

        assert_eq!(tokens.len(), expected.len(), "Token count mismatch.");

        println!("Tokens: {:#?}", tokens);

        for (i, (kind, value)) in expected.into_iter().enumerate() {
            assert_eq!(
                tokens[i].kind, kind,
                "Token[{}] kind mismatch. Expected {:?}, got {:?}",
                i, kind, tokens[i].kind
            );
            assert_eq!(
                lexer.token_value(&tokens[i]),
                value,
                "Token[{}] value mismatch. Expected {:?}, got {:?}",
                i,
                value,
                lexer.token_value(&tokens[i])
            );
        }
    }

    #[test]
    fn test_complex_strings() {
        let input = r#"" " "" "with symbols !@#" "unclosed"#;
        let mut lexer = LexerDefault::<RawToken>::new(input);
        let tokens = lexer.parse();

        assert!(
            tokens.iter().any(|t| matches!(t.kind, TokenKind::String)),
            "Lexer failed to identify any String tokens in input: {}",
            input
        );

        let reconstructed = lexer.reconstruct_source(&tokens);
        assert_eq!(
            reconstructed, input,
            "String reconstruction failed. Original: {}, Got: {}",
            input, reconstructed
        );
    }

    #[test]
    fn test_numeric_boundaries() {
        let input = "123.456 789";
        let mut lexer = LexerDefault::<RawToken>::new(input);
        let tokens = lexer.parse();

        assert_eq!(
            tokens[0].kind,
            TokenKind::Number,
            "Expected '123' to be Number, got {:?}",
            tokens[0].kind
        );
        assert_eq!(
            tokens[1].kind,
            TokenKind::Symbol,
            "Expected '.' to be Symbol, got {:?}",
            tokens[1].kind
        );
        assert_eq!(
            tokens[2].kind,
            TokenKind::Number,
            "Expected '456' to be Number, got {:?}",
            tokens[2].kind
        );
        assert_eq!(
            tokens[3].kind,
            TokenKind::Whitespace,
            "Expected ' ' to be Whitespace, got {:?}",
            tokens[3].kind
        );
        assert_eq!(
            tokens[4].kind,
            TokenKind::Number,
            "Expected '789' to be Number, got {:?}",
            tokens[4].kind
        );
    }

    #[test]
    fn test_unicode_and_whitespace() {
        let input = "let 🦀 = \"value\";\t\n ";
        let mut lexer = LexerDefault::<RawToken>::new(input);
        let tokens = lexer.parse();

        for token in &tokens {
            assert!(
                !matches!(token.kind, TokenKind::Unknown),
                "Unknown token found: {:?} ('{}')",
                token,
                &input[token.span.clone()]
            );
        }

        assert_eq!(
            lexer.reconstruct_source(&tokens),
            input,
            "Unicode/Whitespace reconstruction failed."
        );
    }

    #[test]
    fn test_lex_idempotency() {
        let input = "fn main() { let x = 5; } // check";
        let mut lexer1 = LexerDefault::<RawToken>::new(input);
        let tokens1 = lexer1.parse();

        let reconstructed = lexer1.reconstruct_source(&tokens1);
        let mut lexer2 = LexerDefault::<RawToken>::new(&reconstructed);
        let tokens2 = lexer2.parse();

        assert_eq!(
            tokens1.len(),
            tokens2.len(),
            "Idempotency failed: different token counts. Original: {}, New: {}",
            tokens1.len(),
            tokens2.len()
        );

        for (i, (t1, t2)) in tokens1.iter().zip(tokens2.iter()).enumerate() {
            assert_eq!(
                t1.kind, t2.kind,
                "Token kind mismatch at index {}.\nToken 1: {:?}\nToken 2: {:?}",
                i, t1, t2
            );
            assert_eq!(
                t1.span, t2.span,
                "Token spawn mismatch at index {}.\nToken 1: {:?}\nToken 2: {:?}",
                i, t1, t2
            );
        }
    }

    #[test]
    fn test_lexer_newline_formats() {
        let unix_input = "a\nb";
        let win_input = "a\r\nb";

        // Test Unix Lexing
        let lex_unix = LexerDefault::<RawToken>::new(unix_input);
        let tokens_unix: Vec<RawToken> = lex_unix.collect();

        // Expect: [Identifier("a"), Newline("\n"), Identifier("b")]
        assert_eq!(tokens_unix.len(), 3);
        assert_eq!(tokens_unix[1].kind, TokenKind::Newline);
        assert_eq!(tokens_unix[1].span.end - tokens_unix[1].span.start, 1);
        assert_eq!(&unix_input[tokens_unix[1].span.clone()], "\n");

        // Test Windows Lexing
        let lex_win = LexerDefault::<RawToken>::new(win_input);
        let tokens_win: Vec<RawToken> = lex_win.collect();

        // Expect: [Identifier("a"), Newline("\r\n"), Identifier("b")]
        assert_eq!(tokens_win.len(), 3);
        assert_eq!(tokens_win[1].kind, TokenKind::Newline);
        assert_eq!(tokens_win[1].span.end - tokens_win[1].span.start, 2);
        assert_eq!(&win_input[tokens_win[1].span.clone()], "\r\n");
    }

    #[test]
    fn test_lexer_mixed_whitespace_and_newlines() {
        let input = " \n \r\n ";
        let tokens: Vec<RawToken> = LexerDefault::<RawToken>::new(input).collect();

        dbg!(&tokens);

        // Should distinguish between Whitespace (spaces) and Newline
        assert_eq!(tokens[0].kind, TokenKind::Whitespace);
        assert_eq!(tokens[1].kind, TokenKind::Newline);
        assert_eq!(tokens[2].kind, TokenKind::Whitespace);
        assert_eq!(tokens[3].kind, TokenKind::Newline);
        assert_eq!(tokens[4].kind, TokenKind::Whitespace);
    }

    #[test]
    fn test_files_simple() {
        let cpp_file = r#"
    #include <iostream>

    // Main entry point
    int main() {
        std::cout << "Hello from C++" << std::endl;
        return 0;
    }
    "#;

        let rust_file = r#"
    fn main() {
        /* Macros use the ! symbol */
        println!("Hello from Rust");
    }
    "#;

        let python_file = r#"
    #!/usr/bin/env python3
    import os

    def main():
        print("Hello from Python")

    if __name__ == "__main__":
        main()
    "#;

        let files = [
            ("main.cpp", cpp_file),
            ("main.rs", rust_file),
            ("main.py", python_file),
        ];

        for (filename, content) in files {
            let mut lexer = LexerDefault::<RawToken>::new(content);
            let tokens = lexer.parse();

            assert!(
                !tokens.iter().any(|t| matches!(t.kind, TokenKind::Unknown)),
                "[{}] Lexer failed to categorize some characters: {:?}",
                filename,
                tokens
                    .iter()
                    .filter(|t| matches!(t.kind, TokenKind::Unknown))
                    .map(|t| lexer.token_value(t))
                    .collect::<Vec<_>>()
            );

            let reconstructed = lexer.reconstruct_source(&tokens);
            assert_eq!(
                reconstructed, content,
                "[{}] Reconstruction failed. Content was modified during lexing.",
                filename
            );

            match filename {
                "main.cpp" => {
                    assert!(
                        tokens
                            .iter()
                            .any(|t| lexer.token_value(t) == "#include <iostream>")
                    );
                }
                "main.rs" => {
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "/*"));
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "*/"));
                }
                "main.py" => {
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "__name__"));
                }
                _ => unreachable!(),
            }
        }
    }

    #[test]
    fn test_files_advanced() {
        let advanced_cpp = r#"
    #include <vector>
    #include <memory>
    #include <algorithm>

    /* * Advanced Template Meta-programming 
    * and modern C++ features.
    */
    namespace engine {
        template <typename T>
        class ResourceManager {
        private:
            std::vector<std::shared_ptr<T>> resources;
            size_t total_allocated = 0;

        public:
            ResourceManager() = default;

            auto add(T&& item) -> void {
                resources.push_back(std::make_shared<T>(std::move(item)));
                total_allocated += sizeof(T);
            }

            template <typename F>
            void for_each(F func) {
                std::for_each(resources.begin(), resources.end(), [&](auto& res) {
                    if (res != nullptr) {
                        func(*res);
                    }
                });
            }

            auto size() const { return resources.size(); }
        };
    }

    int main(int argc, char** argv) {
        engine::ResourceManager<int> manager;
        for(int i = 0; i < 100; ++i) {
            manager.add(i * 2);
        }
        // Check bits: 0xFF & 0b1010
        int bit_check = 0xAF >> 2;
        return bit_check > 0 ? 0 : 1;
    }
    "#;

        let advanced_rust = r#"
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    /// A complex trait for asynchronous processing
    pub trait Processor {
        type Output;
        fn process(&self, data: &str) -> Self::Output;
    }

    #[derive(Debug, Clone)]
    struct Node<T> where T: Processor {
        id: u64,
        inner: Arc<Mutex<T>>,
        metadata: HashMap<String, String>,
    }

    impl<T> Node<T> where T: Processor {
        pub fn new(id: u64, p: T) -> Self {
            Self {
                id,
                inner: Arc::new(Mutex::new(p)),
                metadata: HashMap::new(),
            }
        }

        pub async fn run(&self) -> Result<(), Box<dyn std::error::Error>> {
            let lock = self.inner.lock().unwrap();
            let _ = lock.process("input_data");
            println!("Node {} finished processing.", self.id);
            Ok(())
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn test_node() {
            let input = "x += 5; // increment";
            assert!(input.contains("+="));
        }
    }
    "#;

        let advanced_python = r#"
    import numpy as np
    import pandas as pd
    from datetime import datetime

    class DataPipeline:
        """
        Handles heavy data transformations
        """
        def __init__(self, name: str):
            self.name = name
            self.start_time = datetime.now()
            self._cache = {}

        @property
        def status(self) -> str:
            return f"Pipeline {self.name} started at {self.start_time}"

        def process_frame(self, df: pd.DataFrame) -> pd.DataFrame:
            # Complex filtering
            mask = (df['val'] > 0.5) & (df['category'] != 'ignore')
            df['transformed'] = df['val'].apply(lambda x: x ** 2 if x > 0 else -1)
            
            # Multiline string check
            query = """
            SELECT * FROM results
            WHERE score > 90
            AND status = 'PASS'
            """
            return df[mask]

    def run_simulation():
        data = np.random.rand(1000, 3)
        cols = ['a', 'b', 'c']
        df = pd.DataFrame(data, columns=cols)
        pipe = DataPipeline("Sim_01")
        print(pipe.status)
        return pipe.process_frame(df)

    if __name__ == "__main__":
        results = run_simulation()
        print(f"Processed {len(results)} rows.")
    "#;

        let workload = [
            ("Advanced C++", advanced_cpp),
            ("Advanced Rust", advanced_rust),
            ("Advanced Python", advanced_python),
        ];

        for (name, source) in workload {
            let mut lexer = LexerDefault::<RawToken>::new(source);
            let tokens = lexer.parse();

            let unknown_tokens: Vec<_> = tokens
                .iter()
                .filter(|t| t.kind == TokenKind::Unknown)
                .map(|t| lexer.token_value(t))
                .collect();

            assert!(
                unknown_tokens.is_empty(),
                "[{}] Lexer failed on these characters: {:?}",
                name,
                unknown_tokens
            );

            let reconstructed = lexer.reconstruct_source(&tokens);
            assert_eq!(
                reconstructed, source,
                "[{}] RECONSTRUCTION FAILURE. Loss of data detected.",
                name
            );

            match name {
                "Advanced C++" => {
                    assert!(
                        tokens
                            .iter()
                            .any(|t| lexer.token_value(t) == "ResourceManager")
                    );
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == ">>")); // Shift or nested template
                }
                "Advanced Rust" => {
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "Processor"));
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "Box"));
                }
                "Advanced Python" => {
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "lambda"));
                    assert!(tokens.iter().any(|t| lexer.token_value(t) == "status"));
                }
                _ => {}
            }
        }
    }

    #[test]
    fn test_files_simple_header() {
        let source = "\t#define hello_there
\t// Keyboard/Gamepad Navigation options
    bool        ConfigNavSwapGamepadButtons;    // = false
\tbool        ConfigNavMoveSetMousePos;       // = false
";

        let expected = [
            ("\t", TokenKind::Tab),
            ("#define hello_there", TokenKind::Preprocessor),
            ("\n", TokenKind::Newline),
            ("\t", TokenKind::Tab),
            ("// Keyboard/Gamepad Navigation options", TokenKind::Comment),
            ("\n", TokenKind::Newline),
            ("    ", TokenKind::Whitespace),
            ("bool", TokenKind::Keyword),
            ("        ", TokenKind::Whitespace),
            ("ConfigNavSwapGamepadButtons", TokenKind::Identifier),
            (";", TokenKind::Symbol),
            ("    ", TokenKind::Whitespace),
            ("// = false", TokenKind::Comment),
            ("\n", TokenKind::Newline),
            ("\t", TokenKind::Tab),
            ("bool", TokenKind::Keyword),
            ("        ", TokenKind::Whitespace),
            ("ConfigNavMoveSetMousePos", TokenKind::Identifier),
            (";", TokenKind::Symbol),
            ("       ", TokenKind::Whitespace),
            ("// = false", TokenKind::Comment),
            ("\n", TokenKind::Newline),
        ];

        let mut lexer = LexerDefault::<RawToken>::new(source);
        let tokens = lexer.parse();

        let unknown: Vec<_> = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Unknown)
            .map(|t| lexer.token_value(t))
            .collect();
        assert!(
            unknown.is_empty(),
            "Lexer produced Unknown tokens: {:?}",
            unknown
        );

        if tokens.len() != expected.len()
            || !tokens.iter().enumerate().all(|(i, t)| {
                let actual_val = lexer.token_value(t);
                i < expected.len() && (actual_val, t.kind) == expected[i]
            })
        {
            let mut report = String::new();
            report.push_str("\nTOKEN MATCH FAILURE\n");
            report.push_str(&format!(
                "{:<3} | {:<20} | {:<15} | {:<20} | {:<15}\n",
                "IDX", "ACTUAL VAL", "ACTUAL KIND", "EXPECTED VAL", "EXPECTED KIND"
            ));
            report.push_str(&"-".repeat(80));
            report.push('\n');

            let max_len = tokens.len().max(expected.len());
            for i in 0..max_len {
                let actual = tokens.get(i);
                let exp = expected.get(i);

                let a_val = actual
                    .map(|t| format!("{:?}", lexer.token_value(t)))
                    .unwrap_or_else(|| "MISSING".to_string());
                let a_kind = actual
                    .map(|t| format!("{:?}", t.kind))
                    .unwrap_or_else(|| "".to_string());

                let e_val = exp
                    .map(|(v, _)| format!("{:?}", v))
                    .unwrap_or_else(|| "EXTRA".to_string());
                let e_kind = exp
                    .map(|(_, k)| format!("{:?}", k))
                    .unwrap_or_else(|| "".to_string());

                let marker = if actual.is_some()
                    && exp.is_some()
                    && (lexer.token_value(actual.unwrap()), actual.unwrap().kind)
                        == (exp.unwrap().0, exp.unwrap().1)
                {
                    " "
                } else {
                    "!"
                };

                report.push_str(&format!(
                    "{:<3}{} | {:<20} | {:<15} | {:<20} | {:<15}\n",
                    i, marker, a_val, a_kind, e_val, e_kind
                ));
            }
            panic!("{}", report);
        }
    }

    const ALL_MODES: [u8; 3] = [LEXER_MODE_GREEDY, LEXER_MODE_TOKENIZE, LEXER_MODE_NEWLINE];

    fn lex_mode(mode: u8, src: &str) -> Vec<RawToken> {
        let tokens = match mode {
            LEXER_MODE_GREEDY => LexerGreedy::<RawToken>::new(src).parse(),
            LEXER_MODE_TOKENIZE => LexerTokenize::<RawToken>::new(src).parse(),
            LEXER_MODE_NEWLINE => LexerNewLine::<RawToken>::new(src).parse(),
            _ => unreachable!(),
        };
        assert_tiles(mode, src, &tokens);
        tokens
    }

    /// Reconstruction is exact, spans are contiguous and non-empty, and line breaks only ever
    /// appear in Newline tokens (rows, line metadata and revert's hunk_bytes rely on all three).
    fn assert_tiles(mode: u8, src: &str, tokens: &[RawToken]) {
        let mut cursor = 0;
        for t in tokens {
            assert_eq!(
                t.span.start, cursor,
                "mode {mode}: gap or overlap at {t:?} in {src:?}"
            );
            assert!(
                !t.span.is_empty(),
                "mode {mode}: empty token {t:?} in {src:?}"
            );
            let text = &src[t.span.clone()];
            if t.kind != TokenKind::Newline {
                assert!(
                    !text.contains(['\r', '\n']),
                    "mode {mode}: line break inside {:?} token {text:?} in {src:?}",
                    t.kind
                );
            }
            cursor = t.span.end;
        }
        assert_eq!(
            cursor,
            src.len(),
            "mode {mode}: tokens don't reach the end of {src:?}"
        );
    }

    fn range_of(src: &str, part: &str) -> Range<usize> {
        let start = src
            .find(part)
            .unwrap_or_else(|| panic!("{part:?} not in {src:?}"));
        assert!(
            src[start + 1..].find(part).is_none(),
            "{part:?} is not unique in {src:?}"
        );
        start..start + part.len()
    }

    /// In every mode: each non-whitespace token inside one of `comments` has a comment kind and
    /// each one outside doesn't; no token straddles a comment edge. Each of `strings` is covered
    /// by a String token, which is exactly the literal except in NEWLINE mode (where code and
    /// literals share one segment).
    fn check_comments_and_strings(src: &str, comments: &[&str], strings: &[&str]) {
        let comment_ranges: Vec<_> = comments.iter().map(|c| range_of(src, c)).collect();
        let string_ranges: Vec<_> = strings.iter().map(|s| range_of(src, s)).collect();
        for mode in ALL_MODES {
            let tokens = lex_mode(mode, src);
            let describe = |t: &RawToken| {
                format!(
                    "mode {mode}: {:?} {:?} in {src:?}",
                    t.kind,
                    &src[t.span.clone()]
                )
            };
            for t in &tokens {
                let inside = comment_ranges
                    .iter()
                    .any(|r| r.start <= t.span.start && t.span.end <= r.end);
                let overlaps = comment_ranges
                    .iter()
                    .any(|r| t.span.start < r.end && r.start < t.span.end);
                assert!(
                    inside || !overlaps,
                    "{} straddles a comment edge",
                    describe(t)
                );
                if t.kind.is_whitespace() {
                    continue;
                }
                assert_eq!(t.kind.is_comment(), inside, "{}", describe(t));
            }
            for r in &string_ranges {
                let covered = tokens.iter().any(|t| {
                    t.kind == TokenKind::String
                        && if mode == LEXER_MODE_NEWLINE {
                            t.span.start <= r.start && r.end <= t.span.end
                        } else {
                            t.span == *r
                        }
                });
                assert!(
                    covered,
                    "mode {mode}: no String token for {:?} in {src:?}: {tokens:?}",
                    &src[r.clone()]
                );
            }
        }
    }

    #[test]
    fn block_comment_spans_lines_in_every_mode() {
        check_comments_and_strings("a /* if x\n   while */ b\n", &["/* if x\n   while */"], &[]);
        check_comments_and_strings(
            "a /* if x\r\n\t \"y\r\n*/ b\r\n",
            &["/* if x\r\n\t \"y\r\n*/"],
            &[],
        );
    }

    #[test]
    fn line_comment_runs_to_the_end_of_its_line_in_every_mode() {
        check_comments_and_strings("x = 1; // return y\nz\n", &["// return y"], &[]);
        check_comments_and_strings("x = 1; // return y\r\nz", &["// return y"], &[]);
    }

    #[test]
    fn comment_markers_inside_strings_do_not_start_comments_in_every_mode() {
        check_comments_and_strings(
            "s = \"// not\" + \"/* no\";\nt = '/' ;\n",
            &[],
            &["\"// not\"", "\"/* no\"", "'/'"],
        );
        check_comments_and_strings("c = '\"'; // yes \"\nd\n", &["// yes \""], &["'\"'"]);
    }

    #[test]
    fn escaped_quotes_do_not_end_literals_in_every_mode() {
        check_comments_and_strings(
            "a = \"x\\\"// y\\\\\"; b // c\n",
            &["// c"],
            &["\"x\\\"// y\\\\\""],
        );
        check_comments_and_strings(
            "q = '\\''; // c\nr = '\\u{1F600}';\n",
            &["// c"],
            &["'\\''", "'\\u{1F600}'"],
        );
    }

    #[test]
    fn unterminated_string_ends_at_the_line_end_in_every_mode() {
        check_comments_and_strings("a = \"abc // d\nb // c\n", &["// c"], &["\"abc // d"]);
        check_comments_and_strings("a = \"abc // d\r\nb // c", &["// c"], &["\"abc // d"]);
        // A trailing backslash must not swallow the line break, LF or CRLF.
        check_comments_and_strings("a = \"abc\\\r\nb // c\n", &["// c"], &["\"abc\\"]);
        check_comments_and_strings("a = \"abc\\\nb // c\n", &["// c"], &["\"abc\\"]);
        check_comments_and_strings("a = \"abc", &[], &["\"abc"]);
    }

    #[test]
    fn unterminated_block_comment_runs_to_eof_in_every_mode() {
        check_comments_and_strings(
            "a /* x\nif y\n\"z\n// w\n",
            &["/* x\nif y\n\"z\n// w\n"],
            &[],
        );
        check_comments_and_strings("a /*", &["/*"], &[]);
    }

    #[test]
    fn keywords_inside_comments_are_not_keywords_in_every_mode() {
        let src = "if a // if while\n/* return\nstruct */ else\n";
        check_comments_and_strings(src, &["// if while", "/* return\nstruct */"], &[]);
        for mode in ALL_MODES {
            let tokens = lex_mode(mode, src);
            assert!(
                !tokens
                    .iter()
                    .any(|t| t.kind == TokenKind::Keyword && t.kind.is_comment()),
                "mode {mode}"
            );
        }
        let greedy = lex_mode(LEXER_MODE_GREEDY, src);
        let keywords: Vec<_> = greedy
            .iter()
            .filter(|t| t.kind == TokenKind::Keyword)
            .map(|t| &src[t.span.clone()])
            .collect();
        assert_eq!(keywords, ["if", "else"]);
    }

    #[test]
    fn quotes_inside_comments_do_not_start_literals_in_every_mode() {
        check_comments_and_strings(
            "// don't \"x\ny /* it's \"*/ z\n",
            &["// don't \"x", "/* it's \"*/"],
            &[],
        );
    }

    #[test]
    fn a_quote_that_does_not_close_a_char_literal_stays_a_symbol() {
        // Rust lifetimes and apostrophes in prose: no literal, so the trailing comment survives.
        let src = "fn f<'a>(x: &'a str) -> &'static str { // c\ndon't = 'x';\n";
        check_comments_and_strings(src, &["// c"], &["'x'"]);
        for mode in [LEXER_MODE_GREEDY, LEXER_MODE_TOKENIZE] {
            let tokens = lex_mode(mode, src);
            let strings: Vec<_> = tokens
                .iter()
                .filter(|t| t.kind == TokenKind::String)
                .map(|t| &src[t.span.clone()])
                .collect();
            assert_eq!(strings, ["'x'"], "mode {mode}");
        }
    }

    #[test]
    fn block_comments_do_not_nest() {
        check_comments_and_strings("/* a /* b */ c;\n", &["/* a /* b */"], &[]);
    }

    #[test]
    fn stray_comment_end_in_code_is_comment_end() {
        // Pins the pre-existing kind of a `*/` with no open comment; it doesn't change state.
        for mode in [LEXER_MODE_GREEDY, LEXER_MODE_TOKENIZE] {
            let src = "a */ b";
            let tokens = lex_mode(mode, src);
            let kinds: Vec<_> = tokens
                .iter()
                .map(|t| (t.kind, &src[t.span.clone()]))
                .collect();
            assert_eq!(
                kinds,
                [
                    (TokenKind::Identifier, "a"),
                    (TokenKind::Whitespace, " "),
                    (TokenKind::CommentEnd, "*/"),
                    (TokenKind::Whitespace, " "),
                    (TokenKind::Identifier, "b"),
                ],
                "mode {mode}"
            );
        }
        let src = "a */ b";
        let tokens = lex_mode(LEXER_MODE_NEWLINE, src);
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].kind, TokenKind::String);
    }

    #[test]
    fn comments_after_a_preprocessor_directive_are_comments() {
        let src = "#define X \"a//b\" // c\n#if 0 /* x\ny */\nz\n";
        check_comments_and_strings(src, &["// c", "/* x\ny */"], &[]);
        let greedy = lex_mode(LEXER_MODE_GREEDY, src);
        assert_eq!(greedy[0].kind, TokenKind::Preprocessor);
        assert_eq!(&src[greedy[0].span.clone()], "#define X \"a//b\" ");
    }

    #[test]
    fn newline_mode_splits_a_line_into_code_and_comment_segments() {
        let src = "int x; /* a */ y; // t\n  b */\n";
        // Not a comment-start at line 2: the stray `*/` is plain code text in NEWLINE mode.
        let tokens = lex_mode(LEXER_MODE_NEWLINE, src);
        let kinds: Vec<_> = tokens
            .iter()
            .map(|t| (t.kind, &src[t.span.clone()]))
            .collect();
        assert_eq!(
            kinds,
            [
                (TokenKind::String, "int x; "),
                (TokenKind::Comment, "/* a */"),
                (TokenKind::String, " y; "),
                (TokenKind::Comment, "// t"),
                (TokenKind::Newline, "\n"),
                (TokenKind::String, "  b */"),
                (TokenKind::Newline, "\n"),
            ]
        );
    }
}
