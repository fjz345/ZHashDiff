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
}

impl IgnoreOptions {
    /// Per-token flags for both sides. Empty when the options ignore nothing.
    pub fn mask<T: RawTokenTrait>(&self, source: &[T], target: &[T]) -> IgnoreMask {
        if !self.whitespace && !self.comments {
            return IgnoreMask::default();
        }
        let flags = |tokens: &[T]| {
            let mut flags = if self.comments {
                comment_flags(tokens)
            } else {
                vec![false; tokens.len()]
            };
            if self.whitespace {
                for (flag, t) in flags.iter_mut().zip(tokens) {
                    *flag |= t.as_ref().kind.is_whitespace();
                }
            }
            flags
        };
        IgnoreMask {
            source: flags(source),
            target: flags(target),
        }
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
}

impl IgnoreMask {
    pub fn is_empty(&self) -> bool {
        self.source.is_empty() && self.target.is_empty()
    }
}
