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

impl DiffIR {
    pub fn new(path: &[(i32, i32)]) -> Self {
        Self::generate_ir(path)
    }

    // path from myers backtracking, plus original source/target slices, to generate a diff IR
    fn generate_ir(path: &[(i32, i32)]) -> DiffIR {
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
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::*;
    use crate::lexer::{Lexer, RawToken, TokenKind};

    fn mock_token(kind: TokenKind, start: usize, end: usize) -> RawToken {
        RawToken {
            kind,
            span: Range { start, end },
        }
    }

    // Lexer requires a lifetime 'a to match the string slice it wraps
    fn setup_lexers(
        content1: &'static str,
        content2: &'static str,
    ) -> (Lexer<'static, RawToken>, Lexer<'static, RawToken>) {
        (Lexer::new(content1), Lexer::new(content2))
    }

    #[test]
    fn test_generate_ir_simple_equal() {
        let path = vec![(0, 0), (1, 1), (2, 2)];
        let ir = DiffIR::generate_ir(&path);

        assert_eq!(ir.entries.len(), 2);
        assert_eq!(ir.distance, 0);
        assert_eq!(ir.entries[0].operation, DiffOp::Equal);
        assert_eq!(ir.entries[0].token_idx, 0);
        // assert_eq!(ir.entries[0].left_idx, Some(0));
        // assert_eq!(ir.entries[0].right_idx, Some(0));
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

    // #[test]
    // fn test_trait_whitespace_neutralization() {
    //     let content1 = "pub trait NewProcessor {";
    //     let content2 = "pub trait \nProcessor {";
    //     let (l1, l2) = setup_lexers(content1, content2);

    //     let tokens1 = vec![
    //         mock_token(TokenKind::Keyword, 0, 3),      // 0: pub
    //         mock_token(TokenKind::Whitespace, 3, 4),   // 1:
    //         mock_token(TokenKind::Keyword, 4, 9),      // 2: trait
    //         mock_token(TokenKind::Whitespace, 9, 10),  // 3:
    //         mock_token(TokenKind::Identifier, 10, 22), // 4: NewProcessor
    //         mock_token(TokenKind::Whitespace, 22, 23), // 5:
    //         mock_token(TokenKind::Symbol, 23, 24),     // 6: {
    //     ];

    //     let tokens2 = vec![
    //         mock_token(TokenKind::Keyword, 0, 3),      // 0: pub
    //         mock_token(TokenKind::Whitespace, 3, 4),   // 1:
    //         mock_token(TokenKind::Keyword, 4, 9),      // 2: trait
    //         mock_token(TokenKind::Whitespace, 9, 10),  // 3:
    //         mock_token(TokenKind::Newline, 10, 11),    // 4: \n
    //         mock_token(TokenKind::Identifier, 11, 20), // 5: Processor
    //         mock_token(TokenKind::Whitespace, 20, 21), // 6:
    //         mock_token(TokenKind::Symbol, 21, 22),     // 7: {
    //     ];

    //     // Forces NewProcessor, space, and { to be Deletes
    //     // Forces \n, Processor, space, and { to be Inserts
    //     let path = vec![
    //         (0, 0),
    //         (4, 4), // "pub trait " is Equal
    //         (7, 4), // DELETE: "NewProcessor", " ", "{"
    //         (7, 8), // INSERT: "\n", "Processor", " ", "{"
    //     ];

    //     let ir = DiffIR::new(&path, true, &tokens1, &tokens2, &l1, &l2);

    //     println!("Generated IR: {:#?}", ir);

    //     // 1. Ensure the change block is preserved
    //     assert_eq!(ir.entries[3].operation, DiffOp::Equal); //
    //     assert_eq!(ir.entries[4].operation, DiffOp::Delete); // NewProcessor
    //     assert_eq!(ir.entries[5].operation, DiffOp::Insert); // \n
    //     assert_eq!(ir.entries[6].operation, DiffOp::Insert); // Processor

    //     // 2. The CRITICAL Check:
    //     // After the "Processor" identifier, there should be exactly ONE whitespace token
    //     // that is marked Equal before the opening brace.
    //     let whitespace_after_processor = &ir.entries[7];

    //     assert_eq!(whitespace_after_processor.operation, DiffOp::Equal);
    //     assert_eq!(whitespace_after_processor.left_idx, Some(5));
    //     assert_eq!(whitespace_after_processor.right_idx, Some(6));

    //     // Verify no "hallucinated" extra equals exist between index 6 and index 7
    //     assert_eq!(ir.entries[8].operation, DiffOp::Equal); // {
    //     assert_eq!(ir.entries[8].left_idx, Some(6));
    //     assert_eq!(ir.entries.len(), 9);
    // }
}
