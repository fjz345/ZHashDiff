use std::{path::PathBuf, sync::Arc};

use crate::{app::DiffCtx, clamped_cursor::ClampedCursor, ui_egui::panes::ZAppPane};
use eframe::egui::{self, Layout, UiBuilder, scroll_area::ScrollBarVisibility};
use serde::{Deserialize, Serialize};
use zdiff::{
    cached_file::CachedFile,
    diff_builder::{DiffBuilderOptions, LineContent},
    diff_ir::{DiffOp, DiffResult},
    lexer::RawTokenTrait,
};

pub struct FileDiffPaneCtx<'a, T: RawTokenTrait> {
    pub file_source: Option<Arc<CachedFile<T>>>,
    pub file_target: Option<Arc<CachedFile<T>>>,

    pub diff_ctx: Option<&'a DiffCtx>,
    pub diff_options: &'a mut DiffBuilderOptions,
    pub scroll_left: &'a mut f32,
    pub scroll_right: &'a mut f32,

    pub scroll_to_row_span: &'a Option<(usize, Option<usize>)>,
    pub active_highlights: &'a Vec<usize>,
    pub conflict_cursor: &'a mut ClampedCursor,
    pub find_cursor: &'a mut ClampedCursor,
    pub load_file_1_request: &'a mut Option<PathBuf>,
    pub load_file_2_request: &'a mut Option<PathBuf>,
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

        log::trace!("=================================================");

        let sl = *ctx.scroll_left;
        let sr = *ctx.scroll_right;
        let available_width = ui.available_width();
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);

        ui.horizontal(|ui| {
            ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let button_size = egui::vec2(24.0, 24.0);

                let toggle_btn = |ui: &mut egui::Ui,
                                  value: &mut bool,
                                  label: egui::WidgetText,
                                  tooltip: &str| {
                    let btn = egui::Button::new(label).selected(*value);

                    if ui
                        .add_sized(button_size, btn)
                        .on_hover_text(tooltip)
                        .clicked()
                    {
                        *value = !*value;
                    }
                };
                toggle_btn(
                    ui,
                    &mut ctx.diff_options.ignore_whitespace,
                    egui::RichText::new("W").strong().into(),
                    "Ignore Whitespace",
                );
                toggle_btn(
                    ui,
                    &mut ctx.diff_options.highlight_rows,
                    egui::RichText::new("H").strong().into(),
                    "Highlight Rows",
                );
                toggle_btn(
                    ui,
                    &mut ctx.diff_options.ghost_rows,
                    "👻".into(),
                    "Ghost Rows",
                );
                toggle_btn(
                    ui,
                    &mut ctx.diff_options.keyword_highlight,
                    egui::RichText::new("K").strong().into(),
                    "Keyword Highlight",
                );
            });
            ui.separator();

            ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if ui.button("<").clicked() {
                    ctx.conflict_cursor.dec();
                }
                if ui
                    .button(format!(
                        "{}/{}",
                        ctx.conflict_cursor.get().to_string(),
                        ctx.conflict_cursor.get_max().to_string()
                    ))
                    .clicked()
                {}
                if ui.button(">").clicked() {
                    ctx.conflict_cursor.inc();
                }
            });
            ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if ui.button("<").clicked() {
                    ctx.find_cursor.dec();
                }
                if ui
                    .button(format!(
                        "{}/{}",
                        ctx.find_cursor.get().to_string(),
                        ctx.find_cursor.get_max().to_string()
                    ))
                    .clicked()
                {}
                if ui.button(">").clicked() {
                    ctx.find_cursor.inc();
                }
            });
        });

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

        let mut left_rect = egui::Rect::NOTHING;
        let mut right_rect = egui::Rect::NOTHING;
        ui.vertical(|ui| {
            ui.set_min_width(available_width);
            ui.allocate_ui(egui::vec2(ui.available_width(), table_height), |ui| {
                egui::Frame::default()
                    .fill(egui::Color32::from_gray(15))
                    .show(ui, |ui| {
                        use egui_extras::{Column, TableBuilder};

                        let mut table_builder = TableBuilder::new(ui);

                        if let Some((start, maybe_end)) = ctx.scroll_to_row_span {
                            log::info!("scroll_to_row_span: ({:?}, {:?})", start, maybe_end);
                            table_builder =
                                table_builder.scroll_to_row(*start, Some(egui::Align::Min));
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
                                    let is_highlighted = ctx.active_highlights.contains(&row_index);

                                    log::trace!("==LEFT==");
                                    row.col(|ui| {
                                        egui::ScrollArea::horizontal()
                                            .id_salt(format!("l{}", row_index))
                                            .scroll_bar_visibility(
                                                ScrollBarVisibility::AlwaysHidden,
                                            )
                                            .scroll_offset(egui::vec2(sl, 0.0))
                                            .show(ui, |ui| {
                                                Self::render_side_row(
                                                    ui,
                                                    ctx.file_source.clone(),
                                                    ctx.file_target.clone(),
                                                    &diff_row.left,
                                                    widths[0],
                                                    is_highlighted,
                                                );
                                            });
                                        left_rect = left_rect.union(ui.max_rect());
                                    });

                                    row.col(|ui| {
                                        let has_op = |tokens: &[(DiffResult, _)], op| {
                                            tokens
                                                .iter()
                                                .any(|f| !f.0.hide_in_diff && f.0.operation == op)
                                        };

                                        let symbol =
                                            |contains_delete: bool, contains_insert: bool| match (
                                                contains_delete,
                                                contains_insert,
                                            ) {
                                                (true, true) => "≠",
                                                (true, false) => "-",
                                                (false, true) => "+",
                                                (false, false) => " ",
                                            };

                                        let text = match (&diff_row.left, &diff_row.right) {
                                            (
                                                LineContent::Void,
                                                LineContent::Code { tokens, .. },
                                            ) => symbol(
                                                has_op(tokens, DiffOp::Delete),
                                                has_op(tokens, DiffOp::Insert),
                                            ),

                                            (
                                                LineContent::Code { tokens, .. },
                                                LineContent::Void,
                                            ) => symbol(
                                                has_op(tokens, DiffOp::Delete),
                                                has_op(tokens, DiffOp::Insert),
                                            ),

                                            (
                                                LineContent::Code { tokens: t1, .. },
                                                LineContent::Code { tokens: t2, .. },
                                            ) => {
                                                let contains_delete = has_op(t1, DiffOp::Delete)
                                                    || has_op(t2, DiffOp::Delete);
                                                let contains_insert = has_op(t1, DiffOp::Insert)
                                                    || has_op(t2, DiffOp::Insert);

                                                symbol(contains_delete, contains_insert)
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

                                    log::trace!("==RIGHT==");
                                    row.col(|ui| {
                                        egui::ScrollArea::horizontal()
                                            .id_salt(format!("r{}", row_index))
                                            .scroll_bar_visibility(
                                                ScrollBarVisibility::AlwaysHidden,
                                            )
                                            .scroll_offset(egui::vec2(sr, 0.0))
                                            .show(ui, |ui| {
                                                Self::render_side_row(
                                                    ui,
                                                    ctx.file_source.clone(),
                                                    ctx.file_target.clone(),
                                                    &diff_row.right,
                                                    widths[2],
                                                    is_highlighted,
                                                );
                                            });
                                        right_rect = right_rect.union(ui.max_rect());
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

        handle_drops(
            ui,
            &mut ctx.load_file_1_request,
            &mut ctx.load_file_2_request,
            left_rect,
            right_rect,
        );

        egui_tiles::UiResponse::None
    }

    fn render_side_row<T: RawTokenTrait>(
        ui: &mut egui::Ui,
        file_source: Option<Arc<CachedFile<T>>>,
        file_target: Option<Arc<CachedFile<T>>>,
        content: &LineContent,
        width: f32,
        is_highlighted: bool,
    ) {
        let row_h = ui.text_style_height(&egui::TextStyle::Monospace);

        let (rect, _) = ui.allocate_at_least(egui::vec2(width, row_h), egui::Sense::hover());
        let mut extended_rect = rect.clone();
        extended_rect.extend_with_x(9999999.0);

        match content {
            LineContent::Code {
                tokens,
                line_num,
                bg,
            } => {
                ui.painter().rect_filled(
                    extended_rect,
                    0.0,
                    egui::Color32::from_rgba_unmultiplied(bg.0[0], bg.0[1], bg.0[2], bg.0[3]),
                );

                if is_highlighted {
                    ui.painter().rect_filled(
                        extended_rect,
                        0.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 0, 40), // Faint yellow
                    );
                }

                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;

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
                                            [diff_result.token_source_idx.unwrap() as usize];
                                    file_source
                                        .as_ref()
                                        .unwrap()
                                        .read_content_span(token.as_ref().span.clone())
                                }
                                zdiff::diff_ir::DiffOp::Insert => {
                                    let token =
                                        &file_target.clone().expect("Source was None").tokens
                                            [diff_result.token_target_idx.unwrap() as usize];
                                    file_target
                                        .as_ref()
                                        .unwrap()
                                        .read_content_span(token.as_ref().span.clone())
                                }
                            };
                            return str;
                        };
                        log::trace!("-----------------------------------------------");
                        for (diff_result, color) in tokens {
                            let str = read_string(diff_result);
                            log::trace!(
                                "Op: {:?}, Str: {:?}, L: {:?}, R: {:?}",
                                diff_result.operation,
                                str,
                                diff_result.token_source_idx,
                                diff_result.token_target_idx
                            );

                            if str.is_empty() || str == "\n" || str == "\r\n" {
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
                let fill = if is_highlighted {
                    egui::Color32::from_rgba_unmultiplied(255, 255, 0, 40)
                } else {
                    egui::Color32::from_gray(30)
                };
                ui.painter().rect_filled(extended_rect, 0.0, fill);
            }
        }
    }
}

fn handle_drops(
    ui: &egui::Ui,
    load_file_1_request: &mut Option<PathBuf>,
    load_file_2_request: &mut Option<PathBuf>,
    rect1: egui::Rect,
    rect2: egui::Rect,
) -> bool {
    assert!(
        load_file_1_request.is_none() && load_file_2_request.is_none(),
        "File load requests should be None when handling drops"
    );

    // let mut should_draw = false;
    let did_drop = ui.input(|i| {
        if let Some(drop_pos) = i.pointer.hover_pos() {
            // should_draw = true;
            for dropped_file in &i.raw.dropped_files {
                if let Some(path) = &dropped_file.path {
                    if rect1.contains(drop_pos) {
                        *load_file_1_request = Some(path.clone());
                        log::info!("File dropped on left pane: {:?}", path);
                        return true;
                    } else if rect2.contains(drop_pos) {
                        *load_file_2_request = Some(path.clone());
                        log::info!("File dropped on right pane: {:?}", path);
                        return true;
                    }
                    break;
                }
            }
        }
        return false;
    });
    // if should_draw {
    //     ui.painter()
    //         .debug_rect(rect1, egui::Color32::RED, "Left Drop Zone");
    //     ui.painter()
    //         .debug_rect(rect2, egui::Color32::BLUE, "Right Drop Zone");
    // }

    return did_drop;
}
