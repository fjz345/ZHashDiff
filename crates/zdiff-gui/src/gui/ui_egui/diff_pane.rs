use std::{fmt::Debug, path::PathBuf, sync::Arc};

use crate::{app::DiffCtx, ui_egui::panes::ZAppPane};
use eframe::egui::{self, UiBuilder, scroll_area::ScrollBarVisibility};
use serde::{Deserialize, Serialize};
use zdiff::{
    diff_builder::{CachedFile, DiffBuilderOptions, DiffRow, LineContent, build_diff_rows},
    diff_ir::{DiffResult, generate_ir},
    lexer::{Lexer, RawToken, RawTokenTrait},
};

pub struct FileDiffPaneCtx<'a, T: RawTokenTrait> {
    pub file_source: Option<Arc<CachedFile<T>>>,
    pub file_target: Option<Arc<CachedFile<T>>>,

    pub diff_ctx: Option<&'a DiffCtx>,
    pub diff_options: &'a mut DiffBuilderOptions,
    pub scroll_left: &'a mut f32,
    pub scroll_right: &'a mut f32,
    pub scroll_to_goto_row: &'a mut Option<usize>,
    pub scroll_to_find_row: &'a mut Option<usize>,
}

#[derive(Serialize, Deserialize)]
pub struct FileDiffPane {
    pub title: Option<String>,
}

impl ZAppPane for FileDiffPane {
    fn title(&self) -> String {
        self.title.clone().unwrap_or(format!("Pane"))
    }
}

impl FileDiffPane {
    pub fn new(title: Option<String>) -> Self {
        Self { title }
    }

    pub fn ui<T: RawTokenTrait>(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &mut FileDiffPaneCtx<T>,
    ) -> egui_tiles::UiResponse {
        let scroll_delta = ui.input(|i| i.smooth_scroll_delta.x + i.raw_scroll_delta.x);
        if scroll_delta != 0.0 {
            *ctx.scroll_left = (*ctx.scroll_left - scroll_delta).max(0.0);
            *ctx.scroll_right = (*ctx.scroll_right - scroll_delta).max(0.0);
        }

        let sl = *ctx.scroll_left;
        let sr = *ctx.scroll_right;
        let available_width = ui.available_width();
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let button_size = egui::vec2(24.0, 24.0);

            let ws_btn = egui::Button::new(egui::RichText::new("W").strong())
                .selected(ctx.diff_options.ignore_whitespace);
            if ui
                .add_sized(button_size, ws_btn)
                .on_hover_text("Ignore Whitespace")
                .clicked()
            {
                ctx.diff_options.ignore_whitespace = !ctx.diff_options.ignore_whitespace;
            }
            let hl_btn = egui::Button::new(egui::RichText::new("H").strong())
                .selected(ctx.diff_options.highlight_rows);
            if ui
                .add_sized(button_size, hl_btn)
                .on_hover_text("Highlight Rows")
                .clicked()
            {
                ctx.diff_options.highlight_rows = !ctx.diff_options.highlight_rows;
            }
            let gst_btn = egui::Button::new("👻") // Emoji works because of egui's emoji font
                .selected(ctx.diff_options.ghost_rows);
            if ui
                .add_sized(button_size, gst_btn)
                .on_hover_text("Ghost Rows")
                .clicked()
            {
                ctx.diff_options.ghost_rows = !ctx.diff_options.ghost_rows;
            }
            let kw_btn = egui::Button::new(egui::RichText::new("K").strong())
                .selected(ctx.diff_options.keyword_highlight);
            if ui
                .add_sized(button_size, kw_btn)
                .on_hover_text("Keyword Highlight")
                .clicked()
            {
                ctx.diff_options.keyword_highlight = !ctx.diff_options.keyword_highlight;
            }
        });

        // Currently tokens are only computed when diff_rows is calcualted.
        // This means that match only matches on diff_rows
        let diff_rows = ctx.diff_ctx.as_ref().and_then(|f| Some(&f.diff_rows));
        let dummy_1: Option<bool> = Some(false);
        let dummy_2: Option<bool> = Some(false);
        match (&diff_rows, dummy_1, dummy_2) {
            (Some(_), None, None) | (None, None, None) => {
                ui.centered_and_justified(|ui| {
                    ui.label("Load Source & Target files to see diff.");
                });
                return egui_tiles::UiResponse::None;
            }
            (None, Some(_), None) => {
                ui.centered_and_justified(|ui| {
                    ui.label("Target tokens were not set");
                });
                return egui_tiles::UiResponse::None;
            }
            (None, None, Some(_)) => {
                ui.centered_and_justified(|ui| {
                    ui.label("Source tokens were not set");
                });
                return egui_tiles::UiResponse::None;
            }
            (Some(_), Some(_), None) | (Some(_), None, Some(_)) => {}
            (None, Some(_), Some(_)) => {
                ui.centered_and_justified(|ui| {
                    ui.label("Waiting for diff results...");
                });
                return egui_tiles::UiResponse::None;
            }
            (Some(_), Some(_), Some(_)) => {}
        }

        let rows = diff_rows.unwrap();

        ui.add_space(4.0);
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.spacing_mut().item_spacing.y = 0.0;

        let footer_height = 30.0;
        let table_height = (ui.available_height() - footer_height).max(0.0);

        ui.vertical(|ui| {
            ui.set_min_width(available_width);
            ui.allocate_ui(egui::vec2(ui.available_width(), table_height), |ui| {
                egui::Frame::default()
                    .fill(egui::Color32::from_gray(15))
                    .show(ui, |ui| {
                        use egui_extras::{Column, TableBuilder};

                        let mut table_builder = TableBuilder::new(ui);
                        if let Some(scroll_to_row) = ctx.scroll_to_goto_row {
                            table_builder =
                                table_builder.scroll_to_row(*scroll_to_row, Some(egui::Align::Min));
                            *ctx.scroll_to_goto_row = None;
                        }
                        table_builder
                            .id_salt("file_diff_table")
                            .striped(false)
                            .resizable(true)
                            .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                            .column(
                                Column::initial(available_width * 0.48)
                                    .at_least(100.0)
                                    .clip(false),
                            )
                            .column(Column::exact(12.0)) // "≠"
                            .column(Column::remainder().clip(false))
                            .header(20.0, |mut header| {
                                header.col(|ui| {
                                    ui.strong(
                                        ctx.file_source
                                            .as_ref()
                                            .and_then(|f| Some(f.path.display().to_string()))
                                            .unwrap_or_default(),
                                    );
                                });
                                header.col(|_| {});
                                header.col(|ui| {
                                    ui.strong(
                                        ctx.file_target
                                            .as_ref()
                                            .and_then(|f| Some(f.path.display().to_string()))
                                            .unwrap_or_default(),
                                    );
                                });
                            })
                            .body(|body| {
                                let widths = body.widths().to_vec();
                                body.rows(row_height, rows.len(), |mut row| {
                                    let row_index = row.index();
                                    let diff_row = &rows[row.index()];

                                    row.col(|ui| {
                                        egui::ScrollArea::horizontal()
                                            .id_salt((format!("l{}", row_index)))
                                            .scroll_bar_visibility(
                                                ScrollBarVisibility::AlwaysHidden,
                                            )
                                            .scroll_offset(egui::vec2(sl, 0.0))
                                            .show(ui, |ui| {
                                                Self::render_side(
                                                    ui,
                                                    ctx.file_source.clone(),
                                                    ctx.file_target.clone(),
                                                    &diff_row.left,
                                                    widths[0],
                                                );
                                            });
                                    });

                                    row.col(|ui| {
                                        let text = match (&diff_row.left, &diff_row.right) {
                                            (LineContent::Void, _) => "+",
                                            (_, LineContent::Void) => "-",
                                            (
                                                LineContent::Code { .. },
                                                LineContent::Code { bg, .. },
                                            ) if egui::Color32::from_rgba_unmultiplied(
                                                bg.0[0], bg.0[1], bg.0[2], bg.0[3],
                                            ) != egui::Color32::TRANSPARENT =>
                                            {
                                                "≠"
                                            }
                                            _ => " ",
                                        };
                                        ui.centered_and_justified(|ui| {
                                            ui.label(
                                                egui::RichText::new(text)
                                                    .color(egui::Color32::DARK_GRAY),
                                            );
                                        });
                                    });

                                    row.col(|ui| {
                                        egui::ScrollArea::horizontal()
                                            .id_salt(format!("r{}", row_index))
                                            .scroll_bar_visibility(
                                                ScrollBarVisibility::AlwaysHidden,
                                            )
                                            .scroll_offset(egui::vec2(sr, 0.0))
                                            .show(ui, |ui| {
                                                Self::render_side(
                                                    ui,
                                                    ctx.file_source.clone(),
                                                    ctx.file_target.clone(),
                                                    &diff_row.right,
                                                    widths[2],
                                                );
                                            });
                                    });
                                });
                            });
                    });
            });

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let left_w = available_width * 0.48;
                ui.allocate_ui(egui::vec2(left_w, 20.0), |ui| {
                    ui.add(egui::Slider::new(ctx.scroll_left, 0.0..=2000.0).show_value(false));
                });
                ui.add_space(12.0);
                ui.allocate_ui(egui::vec2(ui.available_width(), 20.0), |ui| {
                    ui.add(egui::Slider::new(ctx.scroll_right, 0.0..=2000.0).show_value(false));
                });
            });
        });

        egui_tiles::UiResponse::None
    }

    fn render_side<T: RawTokenTrait>(
        ui: &mut egui::Ui,
        file_source: Option<Arc<CachedFile<T>>>,
        file_target: Option<Arc<CachedFile<T>>>,
        content: &LineContent,
        width: f32,
    ) {
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);

        let (rect, _) = ui.allocate_at_least(egui::vec2(width, row_h), egui::Sense::hover());

        match content {
            LineContent::Code {
                tokens,
                line_num,
                bg,
            } => {
                ui.painter().rect_filled(
                    rect,
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(bg.0[0], bg.0[1], bg.0[2], bg.0[3]),
                );

                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0; // Keep tokens tight

                        // Line number gutter (fixed width)
                        let gutter_width = 35.0;
                        let line_num_str = if *line_num > 0 {
                            line_num.to_string()
                        } else {
                            String::new()
                        };

                        ui.add_sized(
                            [gutter_width, row_h],
                            egui::Label::new(
                                egui::RichText::new(line_num_str)
                                    .color(egui::Color32::DARK_GRAY)
                                    .size(10.0),
                            ),
                        );

                        ui.add_space(4.0);

                        let read_string = |diff_result: &DiffResult| -> &str {
                            let str = match diff_result.operation {
                                zdiff::diff_ir::DiffOp::Equal | zdiff::diff_ir::DiffOp::Delete => {
                                    let token =
                                        &file_source.clone().expect("Source was None").tokens
                                            [diff_result.token_idx as usize];
                                    file_source
                                        .as_ref()
                                        .unwrap()
                                        .read_content_span(token.as_ref().span.clone())
                                }
                                zdiff::diff_ir::DiffOp::Insert => {
                                    let token =
                                        &file_target.clone().expect("Source was None").tokens
                                            [diff_result.token_idx as usize];
                                    file_target
                                        .as_ref()
                                        .unwrap()
                                        .read_content_span(token.as_ref().span.clone())
                                }
                            };
                            return str;
                        };
                        for (diff_result, color) in tokens {
                            let str = read_string(diff_result);
                            if str == "\n" || str == "\r\n" {
                                continue;
                            }
                            ui.label(egui::RichText::new(str).color(
                                egui::Color32::from_rgba_unmultiplied(
                                    color.0[0], color.0[1], color.0[2], color.0[3],
                                ),
                            ));
                        }
                    });
                });
            }
            LineContent::Void => {
                ui.painter()
                    .rect_filled(rect, 0.0, egui::Color32::from_gray(15));
            }
        }
    }
}

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
