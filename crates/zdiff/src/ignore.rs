use std::sync::Arc;

use regex::Regex;

use crate::lexer::{RawTokenTrait, TokenKind};

/// Diff options that decide line equality: an ignored token is dropped from the line key and
/// hidden in the rows. Part of the diff stage's input, so changing one recomputes the diff.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
// Missing fields load as default, so persisted state survives new options.
#[cfg_attr(feature = "serde", serde(default))]
pub struct IgnoreOptions {
    // Flattened into DiffBuilderOptions; the name is the persisted key from before the split.
    #[cfg_attr(feature = "serde", serde(rename = "ignore_whitespace"))]
    pub whitespace: bool,
    #[cfg_attr(feature = "serde", serde(rename = "ignore_comments"))]
    pub comments: bool,
    #[cfg_attr(feature = "serde", serde(rename = "ignore_regex"))]
    pub patterns: IgnorePatterns,
}

impl IgnoreOptions {
    /// Per-token flags for both sides. Empty when the options ignore nothing. Each text is the one
    /// its tokens' spans index into.
    pub fn mask<T: RawTokenTrait>(
        &self,
        source: &[T],
        source_text: &str,
        target: &[T],
        target_text: &str,
    ) -> IgnoreMask {
        if !self.whitespace && !self.comments && self.patterns.is_empty() {
            return IgnoreMask::default();
        }
        let matched_source = self.patterns.flags(source, source_text);
        let matched_target = self.patterns.flags(target, target_text);
        let flags = |tokens: &[T], matched: &[bool]| {
            let mut flags = if self.comments {
                comment_flags(tokens)
            } else {
                vec![false; tokens.len()]
            };
            for (i, (flag, t)) in flags.iter_mut().zip(tokens).enumerate() {
                *flag |= self.whitespace && t.as_ref().kind.is_whitespace();
                *flag |= matched.get(i).copied().unwrap_or(false);
            }
            flags
        };
        IgnoreMask {
            source: flags(source, &matched_source),
            target: flags(target, &matched_target),
            matched_source,
            matched_target,
        }
    }
}

/// User regexes, one per line of `text`, whose matches are ignored. Compiled once on
/// construction; invalid lines are skipped and reported in `errors`. Equality and persistence go
/// by `text` alone, so a typo survives a restart and the compiled set never needs comparing.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(from = "String", into = "String"))]
pub struct IgnorePatterns {
    text: String,
    // Arc: the options are cloned into every stage input.
    compiled: Arc<[Regex]>,
    errors: Vec<PatternError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternError {
    /// 0-based line of `IgnorePatterns::text`.
    pub line: usize,
    pub message: String,
}

impl IgnorePatterns {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let mut compiled = Vec::new();
        let mut errors = Vec::new();
        for (line, pattern) in text.lines().enumerate() {
            if pattern.is_empty() {
                continue;
            }
            match Regex::new(pattern) {
                Ok(regex) => compiled.push(regex),
                Err(e) => errors.push(PatternError {
                    line,
                    message: e.to_string(),
                }),
            }
        }
        Self {
            text,
            compiled: compiled.into(),
            errors,
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn errors(&self) -> &[PatternError] {
        &self.errors
    }

    /// True when no valid pattern exists, so nothing is ignored.
    pub fn is_empty(&self) -> bool {
        self.compiled.is_empty()
    }

    /// Per token, true when it lies entirely inside one match of one pattern. Patterns run per
    /// line, on the line's text without its terminator: lines split after each Newline token, as
    /// in the line diff, so no match spans lines and line breaks are never matched. Empty when
    /// there are no patterns.
    pub fn flags<T: RawTokenTrait>(&self, tokens: &[T], text: &str) -> Vec<bool> {
        if self.is_empty() {
            return Vec::new();
        }
        let mut flags = vec![false; tokens.len()];
        let mut line_start = 0;
        while line_start < tokens.len() {
            let newline = tokens[line_start..]
                .iter()
                .position(|t| t.as_ref().kind == TokenKind::Newline)
                .map(|i| line_start + i);
            let line_end = newline.unwrap_or(tokens.len());
            let line = &tokens[line_start..line_end];
            if let (Some(first), Some(last)) = (line.first(), line.last()) {
                let offset = first.as_ref().span.start;
                let haystack = &text[offset..last.as_ref().span.end];
                for regex in self.compiled.iter() {
                    for m in regex.find_iter(haystack).filter(|m| !m.is_empty()) {
                        let (start, end) = (offset + m.start(), offset + m.end());
                        // Tokens are contiguous and ordered, so the covered ones are a run.
                        let first_inside = line.partition_point(|t| t.as_ref().span.start < start);
                        for (i, t) in line.iter().enumerate().skip(first_inside) {
                            if t.as_ref().span.end > end {
                                break;
                            }
                            flags[line_start + i] = true;
                        }
                    }
                }
            }
            line_start = line_end + 1;
        }
        flags
    }
}

impl PartialEq for IgnorePatterns {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}
impl Eq for IgnorePatterns {}

impl From<String> for IgnorePatterns {
    fn from(text: String) -> Self {
        Self::new(text)
    }
}

impl From<IgnorePatterns> for String {
    fn from(patterns: IgnorePatterns) -> Self {
        patterns.text
    }
}

/// Per token, true when it belongs to a comment:
/// - comment tokens
/// - blanks inside a comment. The lexer keeps their whitespace kinds, so without this an extra
///   word in a block comment changes the key.
/// - the blanks directly before a comment, so `x();` and `x(); // note` are equal lines
///
/// Line breaks are never comment: they shape the lines, and ignoring them is ignore-whitespace's.
fn comment_flags<T: RawTokenTrait>(tokens: &[T]) -> Vec<bool> {
    let is_blank = |t: &T| matches!(t.as_ref().kind, TokenKind::Whitespace | TokenKind::Tab);
    let mut flags = vec![false; tokens.len()];
    let (mut in_block, mut in_line) = (false, false);
    for (i, token) in tokens.iter().enumerate() {
        let kind = token.as_ref().kind;
        if kind.is_comment() {
            if !in_block && !in_line {
                let blanks_before = tokens[..i].iter().rev().take_while(|t| is_blank(t)).count();
                flags[i - blanks_before..i].fill(true);
            }
            flags[i] = true;
            match kind {
                TokenKind::CommentStart => in_block = true,
                TokenKind::CommentEnd => in_block = false,
                // A Comment outside a block starts a line comment (TOKENIZE splits it into
                // tokens; GREEDY makes it one token to the line end, NEWLINE one segment).
                _ => in_line |= !in_block,
            }
        } else if is_blank(token) {
            flags[i] = in_block || in_line;
        } else if kind == TokenKind::Newline {
            in_line = false;
        }
    }
    flags
}

/// Per token, true when it is dropped from the line key. Both sides empty means nothing is
/// ignored; otherwise each side has one flag per token.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IgnoreMask {
    pub source: Vec<bool>,
    pub target: Vec<bool>,
    /// The regex-matched subset of `source`/`target`, shown dimmed. Empty without patterns.
    pub matched_source: Vec<bool>,
    pub matched_target: Vec<bool>,
}

impl IgnoreMask {
    pub fn is_empty(&self) -> bool {
        self.source.is_empty() && self.target.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::{LexerDefault, RawToken};

    fn lex(text: &str) -> Vec<RawToken> {
        LexerDefault::<RawToken>::new(text).parse()
    }

    fn with_patterns(patterns: &str) -> IgnoreOptions {
        IgnoreOptions {
            patterns: IgnorePatterns::new(patterns),
            ..Default::default()
        }
    }

    /// Text of each token the options ignore in `text`, and of each token only the patterns match.
    fn ignored<'a>(options: &IgnoreOptions, text: &'a str) -> (Vec<&'a str>, Vec<&'a str>) {
        let tokens = lex(text);
        let mask = options.mask(&tokens, text, &[], "");
        let pick = |flags: &[bool]| -> Vec<&'a str> {
            assert!(flags.is_empty() || flags.len() == tokens.len());
            tokens
                .iter()
                .zip(flags)
                .filter(|(_, f)| **f)
                .map(|(t, _)| &text[t.span.clone()])
                .collect()
        };
        (pick(&mask.source), pick(&mask.matched_source))
    }

    fn matched<'a>(patterns: &str, text: &'a str) -> Vec<&'a str> {
        let (ignored, matched) = ignored(&with_patterns(patterns), text);
        assert_eq!(
            ignored, matched,
            "patterns alone ignore exactly what they match"
        );
        matched
    }

    #[test]
    fn a_token_fully_inside_a_match_is_ignored() {
        assert_eq!(matched(r"\d+", "id = 1234;\n"), ["1234"]);
        // One match covering several tokens ignores all of them.
        assert_eq!(matched(r"\d+:\d+", "at 12:30 go\n"), ["12", ":", "30"]);
    }

    #[test]
    fn a_partially_matched_token_still_counts() {
        assert_eq!(matched("foo", "foobar foo\n"), ["foo"]);
        assert_eq!(matched(r"\d\d", "12345\n"), Vec::<&str>::new());
        // The match covers the blank but only part of each word.
        assert_eq!(matched("ar f", "bar foo\n"), [" "]);
    }

    #[test]
    fn several_patterns_combine() {
        assert_eq!(matched("a1\nb2", "a1 b2 c3\n"), ["a1", "b2"]);
        // Blank lines between patterns are skipped, CRLF between them is a line break.
        assert_eq!(matched("a1\r\n\r\nc3\r\n", "a1 b2 c3\n"), ["a1", "c3"]);
    }

    #[test]
    fn an_invalid_pattern_is_reported_and_the_others_still_apply() {
        let text = "(\n\\d+\n[z-a]";
        let patterns = IgnorePatterns::new(text);
        let lines: Vec<usize> = patterns.errors().iter().map(|e| e.line).collect();
        assert_eq!(lines, [0, 2]);
        assert!(patterns.errors().iter().all(|e| !e.message.is_empty()));
        assert!(!patterns.is_empty());
        assert_eq!(patterns.text(), text, "the text keeps the invalid lines");
        assert_eq!(matched(text, "x = 42;\n"), ["42"]);

        let only_invalid = IgnorePatterns::new("(");
        assert!(only_invalid.is_empty());
        assert_eq!(matched("(", "x = 42;\n"), Vec::<&str>::new());
    }

    #[test]
    fn patterns_are_evaluated_per_line() {
        assert_eq!(matched(r"a\nb", "a\nb\n"), Vec::<&str>::new());
        assert_eq!(matched(r"a\r\nb", "a\r\nb\r\n"), Vec::<&str>::new());
        // Line breaks are never matched, and anchors apply to each line.
        assert_eq!(matched(r"\s+", "x  \ny\r\n"), ["  "]);
        assert_eq!(matched("^b$", "a\nb\r\nc\n"), ["b"]);
        assert_eq!(
            matched("c$", "a b c"),
            ["c"],
            "last line without a line break"
        );
    }

    #[test]
    fn empty_matches_ignore_nothing() {
        assert_eq!(matched("x*", "a b\n"), Vec::<&str>::new());
    }

    #[test]
    fn patterns_combine_with_whitespace_and_comments() {
        let text = "x = 12; // c 34\n";
        let options = IgnoreOptions {
            whitespace: true,
            comments: true,
            patterns: IgnorePatterns::new(r"\d+"),
        };
        let (all, matched_all) = ignored(&options, text);
        let kept: Vec<&str> = lex(text)
            .iter()
            .map(|t| &text[t.span.clone()])
            .filter(|t| !all.contains(t))
            .collect();
        assert_eq!(kept, ["x", "=", ";"]);
        // Only the code number: the comment is one token, which the pattern covers in part.
        assert_eq!(matched_all, ["12"]);

        let whitespace = IgnoreOptions {
            whitespace: true,
            ..Default::default()
        };
        assert_eq!(ignored(&whitespace, text).1, Vec::<&str>::new());
        let mut no_comments = with_patterns(r"\d+");
        no_comments.whitespace = true;
        let (some, _) = ignored(&no_comments, text);
        assert!(some.contains(&"12") && some.contains(&"\n"));
        assert!(!some.contains(&"// c 34"));
    }

    #[test]
    fn no_patterns_leave_the_mask_empty() {
        let tokens = lex("a 1\n");
        assert!(
            IgnoreOptions::default()
                .mask(&tokens, "a 1\n", &tokens, "a 1\n")
                .is_empty()
        );
        let whitespace = IgnoreOptions {
            whitespace: true,
            ..Default::default()
        };
        let mask = whitespace.mask(&tokens, "a 1\n", &tokens, "a 1\n");
        assert!(mask.matched_source.is_empty() && mask.matched_target.is_empty());
    }
}
