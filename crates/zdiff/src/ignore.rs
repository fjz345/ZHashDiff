use crate::lexer::RawTokenTrait;

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
}

impl IgnoreOptions {
    /// Per-token flags for both sides. Empty when the options ignore nothing.
    pub fn mask<T: RawTokenTrait>(&self, source: &[T], target: &[T]) -> IgnoreMask {
        if !self.whitespace {
            return IgnoreMask::default();
        }
        let flags = |tokens: &[T]| {
            tokens
                .iter()
                .map(|t| t.as_ref().kind.is_whitespace())
                .collect()
        };
        IgnoreMask {
            source: flags(source),
            target: flags(target),
        }
    }
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
