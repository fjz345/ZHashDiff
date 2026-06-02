use std::sync::Arc;

#[cfg(debug_assertions)]
use zdiff::universal_path::UniversalPath;
use zdiff::{
    cached_file::CachedFile,
    diff_builder::{DiffBuilderOptions, DiffRow},
    lexer::RawToken,
    myers::MyersDiffAlgorithm,
};

#[derive(Debug, Default, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct UpdateDiffRowsInput {
    #[cfg_attr(feature = "serde", serde(skip))]
    pub file_1: Option<Arc<CachedFile<RawToken>>>,
    #[cfg_attr(feature = "serde", serde(skip))]
    pub file_2: Option<Arc<CachedFile<RawToken>>>,
    pub options: DiffBuilderOptions,
    pub myers_diff_algorithm: MyersDiffAlgorithm,
}
impl PartialEq for UpdateDiffRowsInput {
    fn eq(&self, other: &Self) -> bool {
        self.file_1.as_ref().map_or(None, |f| Some(f.hash.clone()))
            == other.file_1.as_ref().map_or(None, |f| Some(f.hash.clone()))
            && self.file_2.as_ref().map_or(None, |f| Some(f.hash.clone()))
                == other.file_2.as_ref().map_or(None, |f| Some(f.hash.clone()))
            && self.options == other.options
            && self.myers_diff_algorithm == other.myers_diff_algorithm
            && self.file_1.as_ref().map_or(None, |f| Some(f.lexer_mode))
                == other.file_1.as_ref().map_or(None, |f| Some(f.lexer_mode))
            && self.file_2.as_ref().map_or(None, |f| Some(f.lexer_mode))
                == other.file_2.as_ref().map_or(None, |f| Some(f.lexer_mode))
    }
}

#[derive(Debug, Default)]
pub struct DiffCtx {
    pub file_1_hash: String,
    pub file_2_hash: String,
    #[cfg(debug_assertions)]
    pub debug_file_1_path: UniversalPath,
    #[cfg(debug_assertions)]
    pub debug_file_2_path: UniversalPath,

    pub one_sided_diff_is_left: Option<bool>,
    pub diff_option: DiffBuilderOptions,
    pub precomputed_diffs: Vec<(usize, usize)>, // list indicies of diff_rows of DiffOp != Equal from diff_rows
    pub precomputed_file_rows: (Vec<usize>, Vec<usize>), // line mapping from DiffRow index to DiffRow line number
    // Myers
    pub diff_rows: Vec<DiffRow>,
    pub num_add_deletes: (u32, u32),

    pub update_diff_rows_input: UpdateDiffRowsInput,
}
