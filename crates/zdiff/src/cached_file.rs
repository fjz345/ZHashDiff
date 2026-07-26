use std::{
    fs::File,
    io::{self, Write},
    ops::Range,
    path::Path,
};

use zcommon::hash::hash_file_mmap;

use crate::{
    diff_ir::DiffResult,
    lexer::{
        LEXER_MODE_GREEDY, LEXER_MODE_NEWLINE, LEXER_MODE_TOKENIZE, LexerDefault, LexerGreedy,
        LexerNewLine, LexerTokenize, RawTokenTrait,
    },
    read_file_contents,
    universal_path::UniversalPath,
};

#[derive(Debug, Default, PartialEq)]
pub struct FileMetadata {
    pub line_starts: Vec<usize>,
}

impl FileMetadata {
    pub fn new(contents: &str) -> Self {
        let line_starts = std::iter::once(0)
            .chain(contents.match_indices('\n').map(|(i, _)| i))
            .collect();
        Self { line_starts }
    }

    pub fn get_line_index(&self, byte_offset: usize) -> usize {
        log::trace!(
            "get_line_index(byte_offset: {})\n{:?}",
            byte_offset,
            &self.line_starts
        );
        match self.line_starts.binary_search(&byte_offset) {
            Ok(line) => line,
            Err(line) => line - 1,
        }
    }

    pub fn num_lines(&self) -> usize {
        self.line_starts.len()
    }
}

#[derive(Debug)]
pub struct CachedFile<T: RawTokenTrait> {
    pub path: UniversalPath,
    pub hash: String,
    pub contents: String,
    pub tokens: Vec<T>,
    pub metadata: FileMetadata,
    pub lexer_mode: u8,
}

impl<T: RawTokenTrait> PartialEq for CachedFile<T> {
    fn eq(&self, other: &Self) -> bool {
        self.path == other.path
            && self.lexer_mode == other.lexer_mode
            && self.hash == other.hash
            && self.metadata == other.metadata
    }
}

impl<T: RawTokenTrait> CachedFile<T> {
    pub fn read_content_span(&self, span: Range<usize>) -> &str {
        &self.contents[span]
    }

    pub fn content_search(&self, query: &str) -> Vec<usize> {
        log::trace!("content_search: {}", query);
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
    pub fn new(
        display_path: UniversalPath,
        physical_path: impl AsRef<Path>,
        lexer_mode: u8,
    ) -> io::Result<Self> {
        let lexer_parse_fn = match lexer_mode {
            LEXER_MODE_GREEDY => |contents: &str| LexerGreedy::new(contents).parse(),
            LEXER_MODE_TOKENIZE => |contents: &str| LexerTokenize::new(contents).parse(),
            LEXER_MODE_NEWLINE => |contents: &str| LexerNewLine::new(contents).parse(),
            _ => |contents: &str| LexerDefault::new(contents).parse(),
        };

        let contents = read_file_contents(&physical_path)?;
        let hash = hash_file_mmap(&physical_path)?;
        let tokens = lexer_parse_fn(&contents);
        let metadata = FileMetadata::new(&contents);

        Ok(Self {
            path: display_path,
            hash,
            contents,
            tokens,
            metadata,
            lexer_mode,
        })
    }

    pub fn revert_one_diff(
        &self,
        diff: &DiffResult,
        other: &Self,
        self_is_source: bool,
    ) -> io::Result<()> {
        let remove_from_self = match diff.operation {
            crate::diff_ir::DiffOp::Equal(_) => return Ok(()),
            crate::diff_ir::DiffOp::Delete => self_is_source,
            crate::diff_ir::DiffOp::Insert => !self_is_source,
        };

        let (self_idx, other_idx) = if self_is_source {
            (diff.token_source_idx, diff.token_target_idx)
        } else {
            (diff.token_target_idx, diff.token_source_idx)
        };

        let span_of = |file: &Self, idx: u32| file.tokens[idx as usize].as_ref().span.clone();
        let mut new_contents = self.contents.clone();

        if remove_from_self {
            if let Some(idx) = self_idx {
                new_contents.replace_range(span_of(self, idx), "");
            }
        } else if let Some(o_idx) = other_idx {
            let other_span = span_of(other, o_idx);
            let text = other.read_content_span(other_span.clone());
            let insert_at = self_idx
                .map(|idx| span_of(self, idx).start)
                .unwrap_or_else(|| other_span.start.min(self.contents.len()));

            new_contents.insert_str(insert_at, text);
        }

        let mut file = File::create(&self.path.to_p4_string())?;
        file.write_all(new_contents.as_bytes())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        diff_builder::DiffBuilderOptions,
        lexer::{LEXER_MODE_GREEDY, RawToken},
        test_harness::DiffTestHarness,
    };

    #[test]
    fn test_cached_file_revert() {
        let temp_file_1 = tempfile::NamedTempFile::new().unwrap();
        let temp_file_2 = tempfile::NamedTempFile::new().unwrap();

        let s1 = "\t#define hello_there\n\t// Comment\n";
        let s2 = "\t#define world_here\n\t// Comment\n";
        std::fs::write(temp_file_1.path(), s1).unwrap();
        std::fs::write(temp_file_2.path(), s2).unwrap();

        let display_path_1 = UniversalPath::from(temp_file_1.path().to_path_buf());
        let display_path_2 = UniversalPath::from(temp_file_2.path().to_path_buf());
        let physical_path_1 = temp_file_1.path().to_path_buf();
        let physical_path_2 = temp_file_2.path().to_path_buf();
        let cached_file_1 = CachedFile::<RawToken>::new(
            display_path_1.clone(),
            physical_path_1.clone(),
            LEXER_MODE_GREEDY,
        )
        .expect("failed to create CachedFile");
        let cached_file_1_clone = CachedFile::<RawToken>::new(
            display_path_1.clone(),
            physical_path_1.clone(),
            LEXER_MODE_GREEDY,
        )
        .expect("failed to create CachedFile");
        let cached_file_2 =
            CachedFile::<RawToken>::new(display_path_2, physical_path_2, LEXER_MODE_GREEDY)
                .expect("failed to create CachedFile");

        let path = vec![
            (0, 0),
            (1, 1),
            (2, 1),
            (2, 2),
            (3, 3),
            (4, 4),
            (5, 5),
            (6, 6),
        ];

        let harness = DiffTestHarness::new(
            cached_file_1.contents.as_str(),
            cached_file_2.contents.as_str(),
            path,
            DiffBuilderOptions {
                ghost_rows: false,
                ..Default::default()
            },
            4,
        );
        let diff_ir = harness.diff_ir().clone();

        for a in diff_ir.entries.iter() {
            println!("{:?}", a);
        }

        let diff_to_revert_source = &diff_ir.entries[1];
        cached_file_1
            .revert_one_diff(diff_to_revert_source, &cached_file_2, true)
            .expect("Failed to revert");

        let diff_to_revert_target = &diff_ir.entries[2];
        cached_file_2
            .revert_one_diff(diff_to_revert_target, &cached_file_1_clone, false)
            .expect("Failed to revert");

        let expected_reverted = "\t\n\t// Comment\n";
        let s1_reverted = std::fs::read_to_string(temp_file_1.path()).unwrap();
        let s2_reverted = std::fs::read_to_string(temp_file_2.path()).unwrap();
        assert_eq!(s1_reverted, expected_reverted);
        assert_eq!(s2_reverted, expected_reverted);
    }

    #[test]
    fn test_cached_file_reconstruct_token_modification() {
        let temp_file_1 = tempfile::NamedTempFile::new().unwrap();

        let s1 = "\t#define hello_there\n\t// Comment\n";
        std::fs::write(temp_file_1.path(), s1).unwrap();

        let display_path_1 = UniversalPath::from(temp_file_1.path().to_path_buf());
        let physical_path_1 = temp_file_1.path().to_path_buf();
        let cached_file_greedy = CachedFile::<RawToken>::new(
            display_path_1.clone(),
            physical_path_1.clone(),
            LEXER_MODE_GREEDY,
        )
        .expect("failed to create CachedFile");
        let cached_file_newline = CachedFile::<RawToken>::new(
            display_path_1.clone(),
            physical_path_1.clone(),
            LEXER_MODE_NEWLINE,
        )
        .expect("failed to create CachedFile");
        let cached_file_tokenize = CachedFile::<RawToken>::new(
            display_path_1.clone(),
            physical_path_1.clone(),
            LEXER_MODE_TOKENIZE,
        )
        .expect("failed to create CachedFile");

        let tokens_unmodified = cached_file_greedy.tokens.clone();
        let mut tokens_greedy = cached_file_greedy.tokens.clone();
        let mut tokens_newline = cached_file_newline.tokens.clone();
        let mut tokens_tokenize = cached_file_tokenize.tokens.clone();

        tokens_greedy.remove(0);
        tokens_newline.remove(0);
        tokens_tokenize.remove(0);

        let reconstructed_unmodified = tokens_unmodified
            .iter()
            .map(|t| cached_file_greedy.read_content_span(t.span.clone()))
            .collect::<String>();
        let reconstructed_greedy = tokens_greedy
            .iter()
            .map(|t| cached_file_greedy.read_content_span(t.span.clone()))
            .collect::<String>();
        let reconstructed_newline = tokens_newline
            .iter()
            .map(|t| cached_file_newline.read_content_span(t.span.clone()))
            .collect::<String>();
        let reconstructed_tokenize = tokens_tokenize
            .iter()
            .map(|t| cached_file_tokenize.read_content_span(t.span.clone()))
            .collect::<String>();

        assert_eq!(reconstructed_unmodified, s1);
        assert_eq!(reconstructed_greedy, "#define hello_there\n\t// Comment\n");
        assert_eq!(reconstructed_newline, "\n\t// Comment\n");
        assert_eq!(
            reconstructed_tokenize,
            "#define hello_there\n\t// Comment\n"
        );
    }
}
