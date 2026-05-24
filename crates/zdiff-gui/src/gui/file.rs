use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use zdiff::cached_file::CachedFile;
use zdiff::lexer::{
    LEXER_MODE_DEFAULT, LexerDefault, LexerGreedy, LexerNewLine, LexerTokenize, RawToken,
};

fn default_channel() -> (mpsc::Sender<PathBuf>, mpsc::Receiver<PathBuf>) {
    mpsc::channel()
}

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileProcessor {
    #[cfg_attr(feature = "serde", serde(skip, default = "default_channel"))]
    channel: (mpsc::Sender<PathBuf>, mpsc::Receiver<PathBuf>),

    file_path: Option<PathBuf>,
    #[cfg_attr(feature = "serde", serde(skip))]
    cached_file: Option<Arc<CachedFile<RawToken>>>,

    diff_lexer_mode: u8,
}

impl Default for FileProcessor {
    fn default() -> Self {
        Self {
            channel: default_channel(),
            file_path: None,
            cached_file: None,
            diff_lexer_mode: LEXER_MODE_DEFAULT,
        }
    }
}

impl FileProcessor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_tx(&self) -> mpsc::Sender<PathBuf> {
        self.channel.0.clone()
    }
    pub fn get_rx(&self) -> &mpsc::Receiver<PathBuf> {
        &self.channel.1
    }

    pub fn poll_path_channel(&mut self) {
        while let Ok(path) = self.channel.1.try_recv() {
            self.set_path(path);
        }
    }

    pub fn set_lexer_mode(&mut self, mode: u8) {
        if self.diff_lexer_mode != mode {
            log::debug!(
                "Setting lexer mode {:?} for path: {:?}",
                mode,
                self.file_path
            );
            self.diff_lexer_mode = mode;
            self.invalidate_cache_file();
        }
    }

    pub fn get_path(&mut self) -> Option<PathBuf> {
        self.poll_path_channel();
        self.file_path.clone()
    }

    pub fn get_path_as_string(&self) -> String {
        self.file_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "N/A".to_string())
    }

    pub fn set_path(&mut self, path: impl Into<Option<PathBuf>>) {
        self.file_path = path.into();
        self.invalidate_cache_file();
    }

    pub fn invalidate_cache_file(&mut self) {
        log::debug!("Invalidating cache file for path: {:?}", self.file_path);
        self.cached_file = None;
    }

    pub fn get_cached_file(&mut self) -> Option<Arc<CachedFile<RawToken>>> {
        if let Some(path) = self.get_path()
            && self.cached_file.is_none()
        {
            log::debug!(
                "Constructing CachedFile: {:?} with lexer mode {:?}",
                path,
                self.diff_lexer_mode
            );
            match CachedFile::new(&path, self.diff_lexer_mode) {
                Ok(r) => {
                    self.cached_file = Some(Arc::new(r));
                }
                Err(e) => {
                    log::error!("Cannot find file {}, Error: {e}", path.display());
                    self.cached_file = None;
                }
            }
        }

        self.cached_file.clone()
    }

    pub fn get_cached_file_hash(&mut self) -> Option<String> {
        self.get_cached_file()
            .as_ref()
            .and_then(|f| Some(f.hash.clone()))
    }
}
