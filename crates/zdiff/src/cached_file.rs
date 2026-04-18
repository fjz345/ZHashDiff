use std::{
    io,
    ops::Range,
    path::{Path, PathBuf},
};

use zcommon::hash::hash_file;

use crate::{lexer::RawTokenTrait, read_file_contents};

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
}

#[derive(Debug, Default)]
pub struct CachedFile<T: RawTokenTrait> {
    pub path: PathBuf,
    pub hash: String,
    pub contents: String,
    pub tokens: Vec<T>,
    pub metadata: FileMetadata,
}

impl<T: RawTokenTrait> CachedFile<T> {
    pub fn read_content_span(&self, span: Range<usize>) -> &str {
        &self.contents[span]
    }

    // returns vec of lines that match
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
        path: impl AsRef<Path>,
        lexer_parse_fn: impl FnOnce(&str) -> Vec<T>,
    ) -> io::Result<Self> {
        let contents = read_file_contents(&path)?;
        let hash = hash_file(&path)?;
        let tokens = lexer_parse_fn(&contents);
        let path = path.as_ref().to_path_buf();
        let metadata = FileMetadata::new(&contents);
        Ok(Self {
            path,
            hash,
            contents,
            tokens,
            metadata,
        })
    }
}
