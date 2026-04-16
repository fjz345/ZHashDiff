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
            let mut lexer = Lexer::<RawToken>::new(src);
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
        let mut lexer = Lexer::<RawToken>::new(input);
        let tokens = lexer.parse();

        // Verification Table:
        // Index | Token Kind | Value
        // -------------------------
        // 0     | Identifier | "x"
        // 1     | Whitespace | " "
        // 2     | Symbol     | "/"
        // 3     | Whitespace | " "
        // 4     | Symbol     | "/"
        // 5     | Whitespace | " "
        // 6     | Identifier | "y"
        // 7     | Whitespace | " "
        // 8     | Comment    | "// comment"
        // 9     | Whitespace | "\n"
        // 10    | Identifier | "x"
        // 11    | Whitespace | " "
        // 12    | Comment    | "/* block */"
        // 13    | Whitespace | " "
        // 14    | Identifier | "y"

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
            (TokenKind::Comment, "/* block */"),
            (TokenKind::Whitespace, " "),
            (TokenKind::Identifier, "y"),
        ];

        assert_eq!(tokens.len(), expected.len(), "Token count mismatch.");

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
        let mut lexer = Lexer::<RawToken>::new(input);
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
        let mut lexer = Lexer::<RawToken>::new(input);
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
        let mut lexer = Lexer::<RawToken>::new(input);
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
        let mut lexer1 = Lexer::<RawToken>::new(input);
        let tokens1 = lexer1.parse();

        let reconstructed = lexer1.reconstruct_source(&tokens1);
        let mut lexer2 = Lexer::<RawToken>::new(&reconstructed);
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
        let lex_unix = Lexer::<RawToken>::new(unix_input);
        let tokens_unix: Vec<RawToken> = lex_unix.collect();

        // Expect: [Identifier("a"), Newline("\n"), Identifier("b")]
        assert_eq!(tokens_unix.len(), 3);
        assert_eq!(tokens_unix[1].kind, TokenKind::Newline);
        assert_eq!(tokens_unix[1].span.end - tokens_unix[1].span.start, 1);
        assert_eq!(&unix_input[tokens_unix[1].span.clone()], "\n");

        // Test Windows Lexing
        let lex_win = Lexer::<RawToken>::new(win_input);
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
        let tokens: Vec<RawToken> = Lexer::<RawToken>::new(input).collect();

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
            let mut lexer = Lexer::<RawToken>::new(content);
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
                    assert!(
                        tokens
                            .iter()
                            .any(|t| lexer.token_value(t) == "/* Macros use the ! symbol */")
                    );
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
            let mut lexer = Lexer::<RawToken>::new(source);
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

        let mut lexer = Lexer::<RawToken>::new(source);
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
}
