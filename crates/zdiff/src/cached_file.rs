use std::{io, ops::Range, path::Path};

use zcommon::hash::hash_file_mmap;

use crate::{
    lexer::{
        LEXER_MODE_GREEDY, LEXER_MODE_NEWLINE, LEXER_MODE_TOKENIZE, LexerDefault, LexerGreedy,
        LexerNewLine, LexerTokenize, RawTokenTrait,
    },
    read_file_contents,
    universal_path::UniversalPath,
};

#[derive(Debug, Default)]
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
}
