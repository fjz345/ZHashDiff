#[derive(Debug, Clone, PartialEq)]
pub enum DiffOp {
    Equal,  // From Source 1
    Delete, // From Source 1
    Insert, // From Source 2
}

#[derive(Debug, Clone)]
pub struct DiffResult {
    pub operation: DiffOp,
    pub token_idx: u32,
}

#[derive(Debug, Clone)]
pub struct DiffIR {
    pub entries: Vec<DiffResult>,
    pub distance: i32,
}

// path from myers backtracking, plus original source/target slices, to generate a diff IR
pub fn generate_ir(path: &[(i32, i32)]) -> DiffIR {
    let mut entries = Vec::new();
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
                        token_idx: (x1 + i) as u32,
                    });
                    distance += 1;
                }
                for i in 0..dy {
                    entries.push(DiffResult {
                        operation: DiffOp::Equal,
                        token_idx: (x1 + (dx - dy) + i) as u32,
                    });
                }
            } else if dy > dx {
                for i in 0..(dy - dx) {
                    entries.push(DiffResult {
                        operation: DiffOp::Insert,
                        token_idx: (y1 + i) as u32,
                    });
                    distance += 1;
                }
                for i in 0..dx {
                    entries.push(DiffResult {
                        operation: DiffOp::Equal,
                        token_idx: (x1 + i) as u32,
                    });
                }
            } else {
                for i in 0..dx {
                    entries.push(DiffResult {
                        operation: DiffOp::Equal,
                        token_idx: (x1 + i) as u32,
                    });
                }
            }
        } else if dx > 0 {
            for i in 0..dx {
                entries.push(DiffResult {
                    operation: DiffOp::Delete,
                    token_idx: (x1 + i) as u32,
                });
                distance += 1;
            }
        } else if dy > 0 {
            for i in 0..dy {
                entries.push(DiffResult {
                    operation: DiffOp::Insert,
                    token_idx: (y1 + i) as u32,
                });
                distance += 1;
            }
        }
    }

    DiffIR { entries, distance }
}
