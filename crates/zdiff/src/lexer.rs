use std::ops::Range;

#[derive(Debug, PartialEq)]
pub enum TokenKind {
    Identifier,
    Number,
    String,
    Symbol,
    Whitespace,
    Comment,
    Unknown,
}

#[derive(Debug)]
pub struct RawToken {
    pub kind: TokenKind,
    pub span: Range<usize>,
}

#[derive(Debug)]
pub struct Lexer<'a> {
    source: &'a str,
    cursor: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { source, cursor: 0 }
    }

    fn peek(&self) -> Option<char> {
        self.source[self.cursor..].chars().next()
    }

    fn consume(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.cursor += c.len_utf8();
        Some(c)
    }

    pub fn parse(&mut self) -> Vec<RawToken> {
        self.collect()
    }

    pub fn token_value(&self, token: &RawToken) -> &str {
        &self.source[token.span.clone()]
    }

    pub fn reconstruct_source(&self, tokens: &[RawToken]) -> String {
        tokens.iter().map(|t| self.token_value(t)).collect()
    }
}

impl<'a> Iterator for Lexer<'a> {
    type Item = RawToken;

    fn next(&mut self) -> Option<Self::Item> {
        let c = self.peek()?; 
        let start = self.cursor;

        let kind = match c {
            _ if c.is_whitespace() => {
                self.consume();
                while self.peek().map_or(false, |next| next.is_whitespace()) {
                    self.consume();
                }
                TokenKind::Whitespace
            }
            '/' if self.source[self.cursor..].starts_with("//") => {
                self.consume(); // /
                self.consume(); // /
                while let Some(next_c) = self.peek() {
                    if next_c == '\n' { break; }
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
            _ if c.is_alphabetic() || c == '_' || (c > '\x7f' && !c.is_control() && !c.is_whitespace()) => {
                self.consume();
                while self.peek().map_or(false, |next| {
                    next.is_alphanumeric() || next == '_' || (next > '\x7f' && !next.is_control() && !next.is_whitespace())
                }) {
                    self.consume();
                }
                TokenKind::Identifier
            }
            _ if c.is_digit(10) => {
                self.consume();
                while self.peek().map_or(false, |next| next.is_digit(10)) {
                    self.consume();
                }
                TokenKind::Number
            }
            _ if "!@#$%^&*()-=+[]{}|;:'<>,.?/".contains(c) => {
                let start_index = self.cursor;
                
                let operators = [
                    ">>", "<<", "==", "!=", ">=", "<=", "&&", "||", "->", "::", "+=", "-=", "*=", "/="
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

        Some(RawToken { kind, span: start..self.cursor })
    }
}