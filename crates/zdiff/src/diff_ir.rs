use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::ignore::IgnoreMask;

#[derive(Debug, Clone, PartialEq)]
pub enum DiffOp {
    // true: From Source 1, false: From Source 2
    Equal(bool), // From Source 1 (if not one sided diff)
    Delete,      // From Source 1
    Insert,      // From Source 2
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffResult {
    pub operation: DiffOp,
    pub token_source_idx: Option<u32>,
    pub token_target_idx: Option<u32>,
    pub hide_in_diff: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiffIR {
    pub entries: Vec<DiffResult>,
    pub distance: i32,
}
/// Hides every entry whose token, on either side, is ignored. Uses the same mask as the line
/// key, so what the key ignores is exactly what the rows hide.
pub fn diff_ir_hide_ignored(mut diff_ir: DiffIR, ignore: &IgnoreMask) -> DiffIR {
    if ignore.is_empty() {
        return diff_ir;
    }
    for entry in &mut diff_ir.entries {
        assert!(
            entry.token_source_idx.is_some() || entry.token_target_idx.is_some(),
            "entry without a token"
        );
        let source = entry
            .token_source_idx
            .is_some_and(|i| ignore.source[i as usize]);
        let target = entry
            .token_target_idx
            .is_some_and(|i| ignore.target[i as usize]);
        entry.hide_in_diff |= source || target;
    }
    diff_ir
}
impl DiffIR {
    pub fn new(
        path: &[(i32, i32)],
        equal_from_source: bool,
        cancel_flag: Arc<AtomicBool>,
    ) -> Option<Self> {
        Self::generate_ir(path, equal_from_source, cancel_flag)
    }

    // path from myers backtracking
    fn generate_ir(
        path: &[(i32, i32)],
        equal_from_source: bool,
        cancel_flag: Arc<AtomicBool>,
    ) -> Option<DiffIR> {
        let mut entries = Vec::with_capacity(path.len() * 2); // Worst case: all inserts or deletes
        let mut distance = 0;

        for (idx, window) in path.windows(2).enumerate() {
            if idx % 1000 == 0 && cancel_flag.load(Ordering::Relaxed) {
                return None;
            }

            let (x1, y1) = window[0];
            let (x2, y2) = window[1];

            let dx = x2 - x1;
            let dy = y2 - y1;

            if dx > 0 && dy > 0 {
                if dx > dy {
                    for i in 0..(dx - dy) {
                        entries.push(DiffResult {
                            operation: DiffOp::Delete,
                            token_source_idx: Some((x1 + i) as u32),
                            token_target_idx: None,
                            hide_in_diff: false,
                        });
                        distance += 1;
                    }
                    for i in 0..dy {
                        entries.push(DiffResult {
                            operation: DiffOp::Equal(equal_from_source),
                            token_source_idx: Some((x1 + (dx - dy) + i) as u32),
                            token_target_idx: Some((y1 + i) as u32),
                            hide_in_diff: false,
                        });
                    }
                } else if dy > dx {
                    for i in 0..(dy - dx) {
                        entries.push(DiffResult {
                            operation: DiffOp::Insert,
                            token_source_idx: None,
                            token_target_idx: Some((y1 + i) as u32),
                            hide_in_diff: false,
                        });
                        distance += 1;
                    }
                    for i in 0..dx {
                        entries.push(DiffResult {
                            operation: DiffOp::Equal(equal_from_source),
                            token_source_idx: Some((x1 + i) as u32),
                            token_target_idx: Some((y1 + (dy - dx) + i) as u32),
                            hide_in_diff: false,
                        });
                    }
                } else {
                    for i in 0..dx {
                        entries.push(DiffResult {
                            operation: DiffOp::Equal(equal_from_source),
                            token_source_idx: Some((x1 + i) as u32),
                            token_target_idx: Some((y1 + i) as u32),
                            hide_in_diff: false,
                        });
                    }
                }
            } else if dx > 0 {
                for i in 0..dx {
                    entries.push(DiffResult {
                        operation: DiffOp::Delete,
                        token_source_idx: Some((x1 + i) as u32),
                        token_target_idx: None,
                        hide_in_diff: false,
                    });
                    distance += 1;
                }
            } else if dy > 0 {
                for i in 0..dy {
                    entries.push(DiffResult {
                        operation: DiffOp::Insert,
                        token_source_idx: None,
                        token_target_idx: Some((y1 + i) as u32),
                        hide_in_diff: false,
                    });
                    distance += 1;
                }
            }
        }

        Some(DiffIR { entries, distance })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_ir_simple_equal() {
        let path = vec![(0, 0), (1, 1), (2, 2)];
        let ir = DiffIR::generate_ir(&path, false, Arc::new(AtomicBool::new(false))).unwrap();

        assert_eq!(ir.entries.len(), 2);
        assert_eq!(ir.distance, 0);
        assert_eq!(ir.entries[0].operation, DiffOp::Equal(false));
        assert_eq!(ir.entries[0].token_source_idx, Some(0));
        assert_eq!(ir.entries[0].token_target_idx, Some(0));
    }

    #[test]
    fn test_generate_ir_with_delete_and_insert() {
        let path = vec![(0, 0), (1, 0), (1, 1)];
        let ir = DiffIR::generate_ir(&path, false, Arc::new(AtomicBool::new(false))).unwrap();

        assert_eq!(ir.distance, 2);
        assert_eq!(ir.entries[0].operation, DiffOp::Delete);
        assert_eq!(ir.entries[1].operation, DiffOp::Insert);
    }

    #[test]
    fn test_distance_calculation() {
        let path = vec![(0, 0), (1, 0), (2, 0), (2, 1)];
        let ir = DiffIR::generate_ir(&path, false, Arc::new(AtomicBool::new(false))).unwrap();
        assert_eq!(ir.distance, 3);
    }
}
