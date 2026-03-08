#[derive(Debug, Clone, PartialEq)]
pub enum DiffOp {
    Equal,  // From Source 1
    Delete, // From Source 1
    Insert, // From Source 2
}

#[derive(Debug, Clone)]
pub struct DiffResult<'a, T> {
    pub operation: DiffOp,
    pub token: &'a T,
}

#[derive(Debug, Clone)]
pub struct DiffIR<'a, T> {
    pub entries: Vec<DiffResult<'a, T>>,
    pub distance: i32,
}

// path from myers backtracking, plus original source/target slices, to generate a diff IR
pub fn generate_ir<'a, T>(
    source: &'a [T],
    target: &'a [T],
    path: &[(i32, i32)],
) -> DiffIR<'a, T> {
    let mut entries = Vec::new();
    let mut distance = 0;

    for window in path.windows(2) {
        let (x1, y1) = window[0];
        let (x2, y2) = window[1];

        if x2 > x1 && y2 > y1 {
            entries.push(DiffResult {
                operation: DiffOp::Equal,
                token: &source[x1 as usize],
            });
        } else if x2 > x1 {
            distance += 1;
            entries.push(DiffResult {
                operation: DiffOp::Delete,
                token: &source[x1 as usize],
            });
        } else if y2 > y1 {
            distance += 1;
            entries.push(DiffResult {
                operation: DiffOp::Insert,
                token: &target[y1 as usize],
            });
        }
    }

    DiffIR { entries, distance }
}