use crate::lexer::RawTokenTrait;

#[derive(Debug, Clone, PartialEq)]
pub enum DiffOp {
    Equal,  // From Source 1
    Delete, // From Source 1
    Insert, // From Source 2
}

#[derive(Debug, Clone)]
pub struct DiffResult {
    pub operation: DiffOp,
    pub token_source_idx: Option<u32>,
    pub token_target_idx: Option<u32>,
    pub hide_in_diff: bool,
}

#[derive(Debug, Clone)]
pub struct DiffIR {
    pub entries: Vec<DiffResult>,
    pub distance: i32,
}
type DiffIRNoWs = DiffIR;
pub fn diff_ir_to_no_ws<T: RawTokenTrait>(
    mut diff_ir: DiffIR,
    tokens_source: Option<&'_ [T]>,
    tokens_target: Option<&'_ [T]>,
) -> DiffIRNoWs {
    for entry in &mut diff_ir.entries {
        let should_hide = |token: &T| -> bool {
            if token.as_ref().kind.is_whitespace() {
                true
            } else {
                false
            }
        };
        match (entry.token_source_idx, entry.token_target_idx) {
            (None, None) => panic!("Unreacahble"),
            (Some(t_src), None) => {
                if let Some(tokens) = tokens_source {
                    if should_hide(&tokens[t_src as usize]) {
                        entry.hide_in_diff = true;
                    }
                }
            }
            (None, Some(t_tgt)) => {
                if let Some(tokens) = tokens_target {
                    if should_hide(&tokens[t_tgt as usize]) {
                        entry.hide_in_diff = true;
                    }
                }
            }
            (Some(t_src), Some(t_tgt)) => {
                if let Some(tokens) = tokens_source {
                    if should_hide(&tokens[t_src as usize]) {
                        entry.hide_in_diff = true;
                    }
                }
                if let Some(tokens) = tokens_target {
                    if should_hide(&tokens[t_tgt as usize]) {
                        entry.hide_in_diff = true;
                    }
                }
            }
        }
    }

    DiffIRNoWs {
        entries: diff_ir.entries,
        distance: diff_ir.distance,
    }
}
impl DiffIR {
    pub fn new(path: &[(i32, i32)]) -> Self {
        Self::generate_ir(path)
    }

    // path from myers backtracking
    fn generate_ir(path: &[(i32, i32)]) -> DiffIR {
        let mut entries = Vec::with_capacity(path.len() * 2); // Worst case: all inserts or deletes
        let mut distance = 0;

        for window in path.windows(2) {
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
                            operation: DiffOp::Equal,
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
                            operation: DiffOp::Equal,
                            token_source_idx: Some((x1 + i) as u32),
                            token_target_idx: Some((y1 + (dy - dx) + i) as u32),
                            hide_in_diff: false,
                        });
                    }
                } else {
                    for i in 0..dx {
                        entries.push(DiffResult {
                            operation: DiffOp::Equal,
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

        DiffIR { entries, distance }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_ir_simple_equal() {
        let path = vec![(0, 0), (1, 1), (2, 2)];
        let ir = DiffIR::generate_ir(&path);

        assert_eq!(ir.entries.len(), 2);
        assert_eq!(ir.distance, 0);
        assert_eq!(ir.entries[0].operation, DiffOp::Equal);
        assert_eq!(ir.entries[0].token_source_idx, Some(0));
        assert_eq!(ir.entries[0].token_target_idx, Some(0));
    }

    #[test]
    fn test_generate_ir_with_delete_and_insert() {
        let path = vec![(0, 0), (1, 0), (1, 1)];
        let ir = DiffIR::generate_ir(&path);

        assert_eq!(ir.distance, 2);
        assert_eq!(ir.entries[0].operation, DiffOp::Delete);
        assert_eq!(ir.entries[1].operation, DiffOp::Insert);
    }

    #[test]
    fn test_distance_calculation() {
        let path = vec![(0, 0), (1, 0), (2, 0), (2, 1)];
        let ir = DiffIR::generate_ir(&path);
        assert_eq!(ir.distance, 3);
    }
}
