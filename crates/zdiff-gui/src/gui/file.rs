use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use tempfile::NamedTempFile;
use zdiff::cached_file::CachedFile;
use zdiff::lexer::{LEXER_MODE_DEFAULT, RawToken};
use zdiff::universal_path::UniversalPath;

use crate::p4::get_p4_file_content;

fn default_channel() -> (mpsc::Sender<UniversalPath>, mpsc::Receiver<UniversalPath>) {
    mpsc::channel()
}

fn default_file_path() -> Option<UniversalPath> {
    None
}

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct FileProcessor {
    #[cfg_attr(feature = "serde", serde(skip, default = "default_channel"))]
    channel: (mpsc::Sender<UniversalPath>, mpsc::Receiver<UniversalPath>),
    #[cfg_attr(feature = "serde", serde(skip, default = "default_file_path"))]
    file_path: Option<UniversalPath>,

    #[cfg_attr(feature = "serde", serde(skip))]
    cached_file: Option<Arc<CachedFile<RawToken>>>,
    diff_lexer_mode: u8,
}

impl Default for FileProcessor {
    fn default() -> Self {
        Self {
            channel: default_channel(),
            file_path: default_file_path(),
            cached_file: None,
            diff_lexer_mode: LEXER_MODE_DEFAULT,
        }
    }
}

#[allow(dead_code)]
impl FileProcessor {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn get_tx(&self) -> mpsc::Sender<UniversalPath> {
        self.channel.0.clone()
    }
    pub fn get_rx(&self) -> &mpsc::Receiver<UniversalPath> {
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

    pub fn get_path(&mut self) -> Option<UniversalPath> {
        self.poll_path_channel();
        self.file_path.clone()
    }

    pub fn get_path_as_string(&self) -> String {
        self.file_path
            .as_ref()
            .map(|p| p.to_p4_string())
            .unwrap_or_else(|| "N/A".to_string())
    }

    pub fn set_path(&mut self, path: impl Into<Option<UniversalPath>>) {
        self.file_path = path.into();
        self.invalidate_cache_file();
    }

    pub fn invalidate_cache_file(&mut self) {
        log::debug!("Invalidating cache file for path: {:?}", self.file_path);
        self.cached_file = None;
    }

    pub fn get_cached_file(&mut self) -> Option<Arc<CachedFile<RawToken>>> {
        if let Some(path) = self.get_path() {
            if self.cached_file.is_none() {
                log::debug!(
                    "Constructing CachedFile: {:?} with lexer mode {:?}",
                    path,
                    self.diff_lexer_mode
                );

                let target_path = match &path {
                    UniversalPath::Local(p) => p.clone(),
                    UniversalPath::Depot(depot_str) => {
                        let sanitized = depot_str.trim_start_matches('/');
                        let target_path = std::env::temp_dir().join(sanitized);

                        if let Some(parent) = target_path.parent() {
                            if let Err(e) = std::fs::create_dir_all(parent) {
                                log::error!(
                                    "Failed to create directories for {}: {}",
                                    depot_str,
                                    e
                                );
                                return None;
                            }
                        }

                        let content = match get_p4_file_content(depot_str) {
                            Ok(c) => c,
                            Err(e) => {
                                log::error!("P4 command failed for {}: {}", depot_str, e);
                                return None;
                            }
                        };

                        if let Err(e) = std::fs::write(&target_path, content.as_bytes()) {
                            log::error!("Failed to write P4 content to temp file: {}", e);
                            return None;
                        }

                        target_path
                    }
                };

                match CachedFile::new(&target_path, self.diff_lexer_mode) {
                    Ok(r) => {
                        self.cached_file = Some(Arc::new(r));
                    }
                    Err(e) => {
                        log::error!("Cannot find file {}, Error: {e}", target_path.display());
                        self.cached_file = None;
                    }
                }

                if path.is_depot() {
                    let _ = std::fs::remove_file(&target_path);
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
