use eframe::egui;
use zdiff::{
    diff_builder::{DiffBuilderOptions, DiffRow, LineContent, build_diff_rows},
    diff_ir::generate_ir,
    lexer::{Lexer, RawToken},
};

#[test]
fn test_build_diff_rows_header_edit() {
    let s1 = "\t#define hello_there\n\t// Comment\n";
    let s2 = "\t#define world_here\n\t// Comment\n";

    let mut lex1 = Lexer::<RawToken>::new(s1);
    let mut lex2 = Lexer::<RawToken>::new(s2);
    let t1 = lex1.parse();
    let t2 = lex2.parse();

    // path: (x, y)
    let path = vec![
        (0, 0),
        (1, 1), // \t match
        (2, 1), // Del hello_there
        (2, 2), // Ins world_here
        (3, 3),
        (4, 4),
        (5, 5),
        (6, 6),
    ];

    let options = DiffBuilderOptions {
        keyword_highlight: true,
        highlight_rows: true,
        ghost_rows: false,
        ignore_whitespace: false,
    };

    let diff_ir = generate_ir(&path);
    let rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options);

    println!("\n--- BUILT DIFF ROWS VISUALIZATION ---");
    println!(
        "{:<3} | {:<5} | {:<30} | {:<5} | {:<30}",
        "IDX", "L#", "LEFT", "R#", "RIGHT"
    );

    for (i, row) in rows.iter().enumerate() {
        let l_disp = match &row.left {
            LineContent::Code { tokens, .. } => {
                let collected_tokens: Vec<_> = tokens.iter().map(|(s, _)| s.clone()).collect(); // Collect into a Vec first
                format!("{:?}", collected_tokens)
            }
            _ => "VOID".into(),
        };

        let r_disp = match &row.right {
            LineContent::Code { tokens, .. } => {
                let collected_tokens: Vec<_> = tokens.iter().map(|(s, _)| s.clone()).collect();
                format!("{:?}", collected_tokens)
            }
            _ => "VOID".into(),
        };

        println!(
            "{:<3} | {:<5} | {:<30} | {:<5} | {:<30}",
            i,
            i + 1,
            l_disp,
            i + 1,
            r_disp
        );
    }

    assert_row_content(
        0,
        &rows[0],
        1,
        1,
        "\t#define hello_there\n",
        "\t#define world_here\n",
        &t1,
        &t2,
        &s1,
        &s2,
    );
    assert_row_content(
        1,
        &rows[1],
        2,
        2,
        "\t// Comment\n",
        "\t// Comment\n",
        &t1,
        &t2,
        &s1,
        &s2,
    );
}

#[test]
fn test_build_diff_rows_ghost_enabled() {
    let s1 = "deleted_line\nmatch\n";
    let s2 = "match\n";

    let mut lex1 = Lexer::<RawToken>::new(s1);
    let mut lex2 = Lexer::<RawToken>::new(s2);
    let t1 = lex1.parse();
    let t2 = lex2.parse();

    // path: (x, y)
    // 0,0 -> 2,0 : Delete "deleted_line" and "\n" from left
    // 2,0 -> 4,2 : Match "match" and "\n"
    let path = vec![
        (0, 0),
        (1, 0),
        (2, 0), // Delete "deleted_line", "\n"
        (3, 1),
        (4, 2), // Match "match", "\n"
    ];

    let options = DiffBuilderOptions {
        keyword_highlight: true,
        highlight_rows: true,
        ghost_rows: true,
        ignore_whitespace: false,
    };

    let diff_ir = generate_ir(&path);
    let rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options);

    println!("\n--- GHOST ROWS VISUALIZATION ---");
    println!(
        "{:<3} | {:<5} | {:<30} | {:<5} | {:<30}",
        "IDX", "L#", "LEFT (REAL/GHOST)", "R#", "RIGHT (GHOST/REAL)"
    );
    for (i, row) in rows.iter().enumerate() {
        let (l_text, l_num) = match &row.left {
            LineContent::Code {
                tokens, line_num, ..
            } => (
                // Collect into a Vec to satisfy Debug {:?} formatting
                tokens.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>(),
                *line_num,
            ),
            _ => (vec![], -1), // Match type with a Vec
        };

        let (r_text, r_num) = match &row.right {
            LineContent::Code {
                tokens, line_num, ..
            } => (
                tokens.iter().map(|(s, _)| s.clone()).collect::<Vec<_>>(),
                *line_num,
            ),
            _ => (vec![], -1),
        };

        println!(
            "{:<3} | {:<5} | {:<30?} | {:<5} | {:<30?}",
            i, l_num, l_text, r_num, r_text
        );
    }

    assert_row_content(
        0,
        &rows[0],
        1,
        1,
        "deleted_line\n",
        "deleted_line\n",
        &t1,
        &t2,
        &s1,
        &s2,
    );

    if let LineContent::Code { tokens, .. } = &rows[0].right {
        let ghost_color = egui::Color32::from_rgba_unmultiplied(150, 150, 150, 80);
        assert_eq!(
            egui::Color32::from_rgba_unmultiplied(
                tokens[0].1.0[0],
                tokens[0].1.0[1],
                tokens[0].1.0[2],
                tokens[0].1.0[3],
            ),
            ghost_color,
            "Right side token should have ghost color"
        );
    }

    assert_row_content(1, &rows[1], 2, 2, "match\n", "match\n", &t1, &t2, &s1, &s2);
}

#[test]
fn test_build_diff_rows_ignore_whitespace() {
    let s1 = "ImGuiChildFlags_Border\n";
    let s2 = "ImGuiChildFlags_Borders,  // Renamed in 1.91.1\n";

    let mut lex1 = Lexer::<RawToken>::new(s1);
    let mut lex2 = Lexer::<RawToken>::new(s2);
    let t1 = lex1.parse();
    let t2 = lex2.parse();

    // path: (x, y)
    let path = vec![
        (0, 0),
        (1, 0), // Delete "ImGuiChildFlags_Border"
        (1, 1), // Insert "ImGuiChildFlags_Borders"
        (1, 2), // Insert ","
        (1, 3), // Insert "  " (Whitespace)
        (1, 4), // Insert "// Renamed..." (Comment)
        (2, 5), // Match "\n"
    ];

    let options = DiffBuilderOptions {
        keyword_highlight: true,
        highlight_rows: true,
        ghost_rows: true,
        ignore_whitespace: true,
    };

    let diff_ir = generate_ir(&path);
    let rows = build_diff_rows(diff_ir, Some(&t1), Some(&t2), &options);

    println!("\n--- WHITESPACE IGNORE VISUALIZATION ---");
    for (i, row) in rows.iter().enumerate() {
        let l_text = match &row.left {
            LineContent::Code { tokens, .. } => {
                tokens
                    .iter()
                    .map(|(s, _)| {
                        // Replace 'token' with the actual field name in DiffResult
                        // that contains your RawToken/string data
                        format!("{:?}", s)
                    })
                    .collect::<String>()
            }
            _ => "VOID".into(),
        };

        let r_text = match &row.right {
            LineContent::Code { tokens, .. } => {
                tokens
                    .iter()
                    .map(|(s, _)| {
                        // Replace 'token' with the actual field name in DiffResult
                        // that contains your RawToken/string data
                        format!("{:?}", s)
                    })
                    .collect::<String>()
            }
            _ => "VOID".into(),
        };

        // ... repeat for r_text ...
    }

    assert_eq!(
        rows.len(),
        1,
        "Should have collapsed the diff into a single row"
    );

    assert_row_content(
        0,
        &rows[0],
        1,
        1,
        "ImGuiChildFlags_Border\n",
        "ImGuiChildFlags_Borders,  // Renamed in 1.91.1\n",
        &t1,
        &t2,
        &s1,
        &s2,
    );
}

fn assert_row_content(
    idx: usize,
    row: &DiffRow,
    l_line: i32,
    r_line: i32,
    l_text: &str,
    r_text: &str,
    l_tokens: &[RawToken],
    r_tokens: &[RawToken],
    s1: &str,
    s2: &str,
) {
    let get_data = |content: &LineContent,
                    source_text: &str,
                    target_text: &str,
                    source_tokens: &[RawToken],
                    target_tokens: &[RawToken]| match content {
        LineContent::Code {
            tokens, line_num, ..
        } => {
            let text = tokens
                .iter()
                .map(|(res, _)| {
                    let text = match res.operation {
                        zdiff::diff_ir::DiffOp::Equal | zdiff::diff_ir::DiffOp::Delete => {
                            let token = &source_tokens[res.token_idx as usize];
                            &source_text[token.as_ref().span.clone()]
                        }
                        zdiff::diff_ir::DiffOp::Insert => {
                            let token = &target_tokens[res.token_idx as usize];
                            &target_text[token.as_ref().span.clone()]
                        }
                    };
                    text
                })
                .collect::<String>();
            (text, *line_num)
        }
        LineContent::Void => ("VOID".to_string(), -1),
    };

    let (act_l_text, act_l_num) = get_data(&row.left, s1, s2, l_tokens, r_tokens);
    let (act_r_text, act_r_num) = get_data(&row.right, s1, s2, l_tokens, r_tokens);

    if act_l_text != l_text || act_r_text != r_text || act_l_num != l_line || act_r_num != r_line {
        let mut report = String::new();
        report.push_str(&format!("\nFAIL: Row Index {}\n", idx));
        report.push_str(&format!(
            "{:<5} | {:<5} | {:<40} | {:<5} | {:<40}\n",
            "SIDE", "L-NUM", "LEFT TEXT", "R-NUM", "RIGHT TEXT"
        ));
        report.push_str(&"-".repeat(105));
        report.push('\n');

        report.push_str(&format!(
            "{:<5} | {:<5} | {:<40?} | {:<5} | {:<40?}\n",
            "EXP", l_line, l_text, r_line, r_text
        ));
        report.push_str(&format!(
            "{:<5} | {:<5} | {:<40?} | {:<5} | {:<40?}\n",
            "ACT", act_l_num, act_l_text, act_r_num, act_r_text
        ));

        panic!("{}", report);
    }
}
