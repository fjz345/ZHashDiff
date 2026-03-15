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

        if x2 > x1 && y2 > y1 {
            entries.push(DiffResult {
                operation: DiffOp::Equal,
                token_idx: x1 as u32,
            });
        } else if x2 > x1 {
            distance += 1;
            entries.push(DiffResult {
                operation: DiffOp::Delete,
                token_idx: x1 as u32,
            });
        } else if y2 > y1 {
            distance += 1;
            entries.push(DiffResult {
                operation: DiffOp::Insert,
                token_idx: y1 as u32,
            });
        }
    }

    DiffIR { entries, distance }
}
