use eframe::egui::{self, UiBuilder};
use serde::{Deserialize, Serialize};
use zdiff::lexer::{Lexer, RawToken, TokenKind};
use crate::ui_egui::panes::ZAppPane;

pub struct FileDiffPaneCtx<'a> {
    pub file_1_name: Option<&'a String>,
    pub file_2_name: Option<&'a String>,
    pub file_1: Option<&'a String>,
    pub file_2: Option<&'a String>,
    pub diff_rows: Option<&'a Vec<DiffRow>>,
    pub tokens_1: Option<&'a Vec<RawToken>>,
    pub tokens_2: Option<&'a Vec<RawToken>>,
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
        Self {
            title,
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut FileDiffPaneCtx) -> egui_tiles::UiResponse {
        let (Some(_f1), Some(_f2), Some(rows), Some(_t1), Some(_t2)) = 
            (ctx.file_1, ctx.file_2, ctx.diff_rows, ctx.tokens_1, ctx.tokens_2) else {
            ui.centered_and_justified(|ui| { ui.label("Load files to see diff."); });
            return egui_tiles::UiResponse::None;
        };

        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);
        let available_width = ui.available_width();

        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.spacing_mut().item_spacing.y = 0.0;

        egui::Frame::default()
            .fill(egui::Color32::from_gray(15))
            .show(ui, |ui| {
                use egui_extras::{TableBuilder, Column};

                TableBuilder::new(ui)
                    .id_salt("file_diff_table")
                    .striped(false) 
                    .resizable(true)
                    .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                    .column(Column::initial(available_width * 0.48).at_least(100.0).clip(true))
                    .column(Column::exact(12.0)) // "≠"
                    .column(Column::remainder().clip(true))
                    .header(20.0, |mut header| {
                        header.col(|ui| { ui.strong("Source"); });
                        header.col(|_| {});
                        header.col(|ui| { ui.strong("Target"); });
                    })
                    .body(|body| {
                        let widths = body.widths().to_vec();
                        
                        body.rows(row_height, rows.len(), |mut row| {
                            let diff_row = &rows[row.index()];
                            
                            let left_w = widths[0];
                            let right_w = widths[2];

                            row.col(|ui| {
                                Self::render_side(ui, &diff_row.left, left_w);
                            });

                            row.col(|ui| {
                                let text = match (&diff_row.left, &diff_row.right) {
                                    (LineContent::Void, _) => "+",
                                    (_, LineContent::Void) => "-",
                                    (LineContent::Code { .. }, LineContent::Code { bg, .. }) 
                                        if *bg != egui::Color32::TRANSPARENT => "≠",
                                    _ => " ",
                                };
                                ui.centered_and_justified(|ui| {
                                    ui.label(egui::RichText::new(text).color(egui::Color32::DARK_GRAY));
                                });
                            });

                            row.col(|ui| {
                                Self::render_side(ui, &diff_row.right, right_w);
                            });
                        });
                    });
            });

        egui_tiles::UiResponse::None
    }

    fn render_side(ui: &mut egui::Ui, content: &LineContent, width: f32) {
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
        
        let (rect, _) = ui.allocate_at_least(egui::vec2(width, row_h), egui::Sense::hover());

        match content {
            LineContent::Code { tokens, line_num, bg } => {
                ui.painter().rect_filled(rect, 0.0, *bg);

                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0; // Keep tokens tight

                        // Line number gutter (fixed width)
                        let gutter_width = 35.0;
                        let line_num_str = if *line_num > 0 { line_num.to_string() } else { String::new() };
                        
                        ui.add_sized(
                            [gutter_width, row_h], 
                            egui::Label::new(
                                egui::RichText::new(line_num_str)
                                    .color(egui::Color32::DARK_GRAY)
                                    .size(10.0)
                            )
                        );

                        ui.add_space(4.0);
                        
                        for (text, color) in tokens {
                            if text == "\n" || text == "\r\n" { continue; }
                            ui.label(egui::RichText::new(text).color(*color));
                        }
                    });
                });
            }
            LineContent::Void => {
                ui.painter().rect_filled(rect, 0.0, egui::Color32::from_gray(15));
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiffRow {
    pub left: LineContent,
    pub right: LineContent,
}

#[derive(Debug, Clone)]
pub enum LineContent {
    Code {
        tokens: Vec<(String, egui::Color32)>,
        line_num: i32,
        bg: egui::Color32,
    },
    Void,
}
pub fn build_diff_rows(
    path: &[(i32, i32)],
    t1: &[RawToken],
    t2: &[RawToken],
    lex1: &Lexer,
    lex2: &Lexer,
) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    let mut left_buf = Vec::new();
    let mut right_buf = Vec::new();
    
    let (mut l_num, mut r_num) = (1, 1);
    let (mut is_del, mut is_ins) = (false, false);

    for window in path.windows(2) {
        let (x1, y1) = window[0];
        let (x2, y2) = window[1];

        if x2 > x1 && y2 > y1 { // MATCH
            let val = lex1.token_value(&t1[x1 as usize]);
            left_buf.push((val.to_string(), egui::Color32::GRAY));
            right_buf.push((val.to_string(), egui::Color32::GRAY));
            
            if t1[x1 as usize].kind == TokenKind::Newline {
                rows.push(flush_row(&mut left_buf, &mut right_buf, l_num, r_num, is_del, is_ins));
                l_num += 1; r_num += 1;
                is_del = false; is_ins = false;
            }
        } else if x2 > x1 { // DELETE
            is_del = true;
            let val = lex1.token_value(&t1[x1 as usize]);
            left_buf.push((val.to_string(), egui::Color32::from_rgb(255, 100, 100)));
            if t1[x1 as usize].kind == TokenKind::Newline {
                rows.push(flush_row(&mut left_buf, &mut Vec::new(), l_num, 0, true, false));
                l_num += 1;
            }
        } else if y2 > y1 { // INSERT
            is_ins = true;
            let val = lex2.token_value(&t2[y1 as usize]);
            right_buf.push((val.to_string(), egui::Color32::from_rgb(100, 255, 100)));
            if t2[y1 as usize].kind == TokenKind::Newline {
                rows.push(flush_row(&mut Vec::new(), &mut right_buf, 0, r_num, false, true));
                r_num += 1;
            }
        }
    }

    if !left_buf.is_empty() || !right_buf.is_empty() {
        rows.push(flush_row(&mut left_buf, &mut right_buf, l_num, r_num, is_del, is_ins));
    }

    rows
}

fn flush_row(l: &mut Vec<(String, egui::Color32)>, r: &mut Vec<(String, egui::Color32)>, ln: i32, rn: i32, is_del: bool, is_ins: bool) -> DiffRow {
    let left = if l.is_empty() { LineContent::Void } else {
        LineContent::Code { 
            tokens: l.drain(..).collect(), 
            line_num: ln, 
            bg: if is_del { egui::Color32::from_rgba_unmultiplied(255, 0, 0, 20) } else { egui::Color32::TRANSPARENT }
        }
    };
    let right = if r.is_empty() { LineContent::Void } else {
        LineContent::Code { 
            tokens: r.drain(..).collect(), 
            line_num: rn, 
            bg: if is_ins { egui::Color32::from_rgba_unmultiplied(0, 255, 0, 20) } else { egui::Color32::TRANSPARENT }
        }
    };
    DiffRow { left, right }
}