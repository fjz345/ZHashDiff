use std::sync::Arc;

use crate::{
    clamped_cursor::ClampedCursor,
    diff_ctx::{BlockToggle, DiffRows, DiffStageTimes, MinimalDiffCtx, ScrollSpan},
    revert::{self, RevertRefusal, RevertRequest, RevertTarget},
    ui_egui::{
        active_side::{ActiveSide, ActiveSideState, outline_stroke},
        h_scroll,
        panes::ZAppPane,
    },
    viewer::{
        ViewerKind,
        hex::{self, HexViewCtx},
        image::{self, ImageDiffProcessor},
    },
};
use eframe::egui::{
    self, Layout, TextEdit, UiBuilder, Vec2, scroll_area::ScrollBarVisibility,
    text_selection::LabelSelectionState,
};
use serde::{Deserialize, Serialize};
use zdiff::{
    cached_file::CachedFile,
    diff_builder::{DIMMED, DiffBuilderOptions, DiffRow, LineContent},
    diff_ir::{DiffOp, DiffResult},
    ignore::IgnorePatterns,
    lexer::RawTokenTrait,
    myers::MyersNumAddDelete,
    row_text::build_row_text,
    universal_path::UniversalPath,
};

pub struct FileDiffPaneCtx<'a> {
    // need this because file_source can be none and we want to keep displaying
    // the text edit with the old path if even if it could not load a cachedfile
    pub file_source_path: UniversalPath,
    pub file_target_path: UniversalPath,
    pub file_source_root: Option<UniversalPath>,
    pub file_target_root: Option<UniversalPath>,
    pub file_source_root_valid: bool,
    pub file_target_root_valid: bool,
    pub file_source_path_valid: bool,
    pub file_target_path_valid: bool,
    pub file_source_loading: bool,
    pub file_target_loading: bool,
    pub diff_loading: bool,

    pub diff_ctx: Option<&'a MinimalDiffCtx>,
    pub diff_options: &'a mut DiffBuilderOptions,
    pub scroll_left: &'a mut f32,
    pub scroll_right: &'a mut f32,
    /// Both sides scroll horizontally together.
    pub h_scroll_linked: &'a mut bool,

    pub code_language: &'a str,

    pub scroll_to_row_span: &'a Option<ScrollSpan>,
    pub active_highlights: &'a Vec<usize>,
    pub conflict_cursor: &'a mut ClampedCursor,
    pub find_cursor: &'a mut ClampedCursor,
    pub active_side: &'a mut ActiveSide,
    pub load_file_1_request: &'a mut Option<UniversalPath>,
    pub load_file_2_request: &'a mut Option<UniversalPath>,
    pub set_file_1_root_request: &'a mut Option<UniversalPath>,
    pub set_file_2_root_request: &'a mut Option<UniversalPath>,
    pub pivot: &'a mut (Option<usize>, Option<usize>),
    pub revert_request: &'a mut Option<RevertRequest>,
    /// `RowBlock::key` of the block whose button was clicked, and what the button asks for.
    pub block_toggle_request: &'a mut Option<(usize, BlockToggle)>,
    /// Set when the pair resolved to the Hex viewer; the table then shows hex rows.
    pub hex_view: Option<HexViewCtx<'a>>,
    /// Set when the pair resolved to the Image viewer; the images replace the table rows.
    pub image_view: Option<&'a mut ImageDiffProcessor>,
    /// The toolbar's viewer switch for the current pair; `None` is Auto.
    pub viewer_override: &'a mut Option<ViewerKind>,
    /// Why the pair isn't in the viewer it asked for, if it isn't.
    pub viewer_fallback: Option<&'a str>,
}

#[derive(Serialize, Deserialize)]
pub struct FileDiffPane {
    pub title: Option<String>,
    #[serde(skip)]
    active_side: ActiveSideState,
    #[serde(skip)]
    content_widths: Option<ContentWidths>,
}

/// Each side's widest row, measured once per rows and font.
struct ContentWidths {
    // Kept alive so the pointer comparison can't match a new allocation at the same address.
    rows: Arc<DiffRows>,
    font_id: egui::FontId,
    pixels_per_point: f32,
    widths: [f32; 2],
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
            active_side: ActiveSideState::default(),
            content_widths: None,
        }
    }

    fn content_widths(&mut self, ui: &egui::Ui, diff_ctx: &MinimalDiffCtx) -> [f32; 2] {
        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
        let pixels_per_point = ui.ctx().pixels_per_point();
        if let Some(cached) = &self.content_widths
            && Arc::ptr_eq(&cached.rows, &diff_ctx.diff_rows)
            && cached.font_id == font_id
            && cached.pixels_per_point == pixels_per_point
        {
            return cached.widths;
        }
        let widths = ui.fonts_mut(|fonts| {
            side_content_widths(
                &diff_ctx.diff_rows,
                diff_ctx.input.file_1.as_deref(),
                diff_ctx.input.file_2.as_deref(),
                |c| fonts.glyph_width(&font_id, c),
            )
        });
        self.content_widths = Some(ContentWidths {
            rows: diff_ctx.diff_rows.clone(),
            font_id,
            pixels_per_point,
            widths,
        });
        widths
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut FileDiffPaneCtx) -> egui_tiles::UiResponse {
        // No-op once registered.
        ui.ctx().add_plugin(CopyMarkerPlugin);

        // egui turns Shift+wheel into horizontal delta. Only text rows over a side scroll; the
        // footer bars handle the wheel themselves.
        let (wheel_delta, hover_pos) =
            ui.input(|i| (i.smooth_scroll_delta.x, i.pointer.hover_pos()));
        let wheel_side = hover_pos
            .filter(|_| wheel_delta != 0.0 && ctx.hex_view.is_none() && ctx.image_view.is_none())
            .and_then(|pos| self.active_side.side_at(pos));
        let content_widths = match ctx.diff_ctx {
            Some(diff_ctx) => self.content_widths(ui, diff_ctx),
            None => [0.0; 2],
        };
        // Each side's max offset, set once the text rows are laid out.
        let mut h_scroll_max = None;

        log::trace!("=================================================");

        let available_width = ui.available_width();
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace);

        ui.horizontal(|ui| {
            ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                viewer_override_combo(ui, ctx.viewer_override, ctx.viewer_fallback);
            });
            ui.separator();

            ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let button_size = egui::vec2(24.0, 24.0);

                let toggle_btn = |ui: &mut egui::Ui,
                                  value: &mut bool,
                                  label: egui::WidgetText,
                                  tooltip: &str|
                 -> bool {
                    let btn = egui::Button::new(label).selected(*value);

                    if ui
                        .add_sized(button_size, btn)
                        .on_hover_text(tooltip)
                        .clicked()
                    {
                        *value = !*value;
                        return true;
                    }
                    false
                };
                toggle_btn(
                    ui,
                    &mut ctx.diff_options.ignore.whitespace,
                    egui::RichText::new("W").strong().into(),
                    "Ignore Whitespace",
                );
                toggle_btn(
                    ui,
                    &mut ctx.diff_options.ignore.comments,
                    egui::RichText::new("C").strong().into(),
                    "Ignore Comments",
                );
                ignore_patterns_btn(ui, button_size, &mut ctx.diff_options.ignore.patterns);
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
                toggle_btn(
                    ui,
                    ctx.h_scroll_linked,
                    "🔗".into(),
                    "Link Horizontal Scrolling",
                );

                let mut active = ctx.diff_options.diff_only_with_extra_rows.is_some();
                if toggle_btn(
                    ui,
                    &mut active,
                    egui::RichText::new("D").strong().into(),
                    "Diff Only",
                ) {
                    ctx.diff_options.diff_only_with_extra_rows = active.then_some(2);
                }
                match &mut ctx.diff_options.diff_only_with_extra_rows {
                    Some(diff_rows_num) => {
                        if ui.button("<").clicked() {
                            *diff_rows_num = diff_rows_num.saturating_sub(1);
                        }
                        if ui
                            .button(format!("{}", diff_rows_num.to_string(),))
                            .clicked()
                        {}
                        if ui.button(">").clicked() {
                            *diff_rows_num = (*diff_rows_num + 1).min(99);
                        }
                    }
                    None => {}
                }
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
            ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label("Pivot");
                let mut text = ctx
                    .pivot
                    .0
                    .and_then(|f| Some(f.to_string()))
                    .unwrap_or_default();
                if ui
                    .add(TextEdit::singleline(&mut text).desired_width(25.0))
                    .changed()
                {
                    let usize_parse = text.parse::<usize>();
                    if let Ok(as_usize) = usize_parse {
                        ctx.pivot.0 = Some(as_usize);
                    } else {
                        ctx.pivot.0 = None;
                    }
                }
                let mut text = ctx
                    .pivot
                    .1
                    .and_then(|f| Some(f.to_string()))
                    .unwrap_or_default();
                if ui
                    .add(TextEdit::singleline(&mut text).desired_width(25.0))
                    .changed()
                {
                    let usize_parse = text.parse::<usize>();
                    if let Ok(as_usize) = usize_parse {
                        ctx.pivot.1 = Some(as_usize);
                    } else {
                        ctx.pivot.1 = None;
                    }
                }
            });
            // No diff ctx while a diff is in flight, so a partial time is never shown.
            if let Some(diff_ctx) = ctx.diff_ctx {
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    let (label, tooltip) =
                        diff_status_text(diff_ctx.num_add_deletes, &diff_ctx.stage_times);
                    // Not selectable, so a drag-copy over the rows can't pick it up.
                    ui.add(egui::Label::new(label).selectable(false))
                        .on_hover_text(tooltip);
                });
            } else if let Some(hex_view) = &ctx.hex_view {
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(hex_view.status_text()).selectable(false));
                });
            } else if let Some(image_view) = &ctx.image_view {
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    let (text, size_mismatch) = image_view.status_text();
                    let mut text = egui::RichText::new(text);
                    if size_mismatch {
                        text = text.color(ui.visuals().warn_fg_color);
                    }
                    ui.add(egui::Label::new(text).selectable(false));
                });
            }
        });

        let diff_rows = ctx.diff_ctx.as_ref().and_then(|f| Some(&f.diff_rows));
        let source_path = ctx
            .diff_ctx
            .map(|f| f.input.file_1.as_ref().and_then(|f| Some(&f.path)))
            .unwrap_or_default();
        let target_path = ctx
            .diff_ctx
            .map(|f| f.input.file_2.as_ref().and_then(|f| Some(&f.path)))
            .unwrap_or_default();

        let press_pos = ui.input(|i| {
            i.pointer
                .primary_pressed()
                .then(|| i.pointer.interact_pos())
                .flatten()
        });
        let active_side =
            self.active_side
                .begin_frame(press_pos, source_path.is_some(), target_path.is_some());
        *ctx.active_side = active_side;

        let mut waiting_for_diff = false;
        let mut do_not_render_diff = match (&diff_rows, source_path, target_path) {
            (Some(_), None, None) | (None, None, None) => true,
            (None, Some(_), None) => true,
            (None, None, Some(_)) => true,
            (Some(_), Some(_), None) | (Some(_), None, Some(_)) => false,
            (None, Some(_), Some(_)) => {
                waiting_for_diff = true;
                true
            }
            (Some(_), Some(_), Some(_)) => false,
        };
        waiting_for_diff |= ctx.diff_loading;
        do_not_render_diff |= waiting_for_diff;
        // The hex and image viewers draw the loaded bytes and need no text diff.
        let other_viewer = ctx.hex_view.is_some() || ctx.image_view.is_some();
        if other_viewer {
            waiting_for_diff = false;
            do_not_render_diff = false;
        }
        let diff_rows_len = diff_rows.map(|f| f.len()).unwrap_or_else(|| 0);

        ui.add_space(4.0);
        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
        ui.spacing_mut().item_spacing.y = 0.0;

        let footer_height = 30.0;
        let table_height = (ui.available_height() - footer_height).max(0.0);

        let mut left_rect = egui::Rect::NOTHING;
        let mut right_rect = egui::Rect::NOTHING;
        let mut table_rect = egui::Rect::NOTHING;
        ui.vertical(|ui| {
            ui.set_min_width(available_width);

            if table_height > 0.0 && (diff_rows_len > 0 || other_viewer) {
                table_rect = ui.allocate_ui(egui::vec2(ui.available_width(), table_height), |ui| {
                    egui::Frame::default()
                        .fill(egui::Color32::from_gray(15))
                        .show(ui, |ui| {
                            use egui_extras::{Column, TableBuilder};

                            let mut table_builder = TableBuilder::new(ui);
                            // Images are drawn below the header (the path editors), in the
                            // table's columns, outside its scroll area so the wheel zooms.
                            let mut image_left_width = None;

                            if let Some(ScrollSpan { start, maybe_end }) = &ctx.scroll_to_row_span {
                                log::trace!("scroll_to_row_span: ({:?}, {:?})", start, maybe_end);
                                table_builder =
                                    table_builder.scroll_to_row(*start, Some(egui::Align::Min));
                            }

                            let editable_path_test =
                                |ui: &mut egui::Ui,
                                 id: egui::Id,
                                 file: &UniversalPath,
                                 is_valid: bool,
                                 spinner_active: bool|
                                 -> Option<UniversalPath> {
                                    let original_path = file.to_string();

                                    let mut text = ui.memory_mut(|mem| {
                                        mem.data
                                            .get_temp::<String>(id)
                                            .unwrap_or_else(|| original_path.clone())
                                    });

                                    let response = ui
                                        .scope(|ui| {
                                            if !is_valid {
                                                let bg = egui::Color32::from_rgb(45, 10, 10);
                                                let stroke =
                                                    egui::Stroke::new(1.0, egui::Color32::RED);

                                                {
                                                    let visuals = &mut ui.visuals_mut();
                                                    visuals.text_edit_bg_color = Some(bg);
                                                }
                                                let visuals = &mut ui.visuals_mut().widgets;

                                                visuals.inactive.bg_fill = bg;
                                                visuals.inactive.bg_stroke = stroke;

                                                visuals.hovered.bg_fill = bg;
                                                visuals.hovered.bg_stroke = stroke;

                                                visuals.active.bg_fill = bg;
                                                visuals.active.bg_stroke = stroke;
                                            }

                                            let response = ui.horizontal(|ui| {
                                                let spinner = egui::Spinner::new();
                                                let res = ui.add_sized(
                                                    [
                                                        ui.available_width(),
                                                        ui.spacing().interact_size.y,
                                                    ],
                                                    egui::TextEdit::singleline(&mut text).id(id),
                                                );
                                                if spinner_active {
                                                    let spinner_size = res.rect.size().y * 0.5;
                                                    let spinner_x = res.rect.right() - spinner_size;
                                                    let spinner_pos = egui::Pos2::new(
                                                        spinner_x,
                                                        res.rect.top() + spinner_size,
                                                    );
                                                    let spinner_rect = egui::Rect::from_center_size(
                                                        spinner_pos,
                                                        Vec2::new(spinner_size, spinner_size),
                                                    );
                                                    spinner.paint_at(ui, spinner_rect);
                                                }

                                                res
                                            });
                                            response.inner
                                        })
                                        .inner;

                                    if response.changed() {
                                        ui.memory_mut(|mem| mem.data.insert_temp(id, text.clone()));
                                    }

                                    // lost_focus not called correctly, quick fix: https://github.com/emilk/egui/issues/2142
                                    // Does not handle holding down mouse on another text field.....
                                    if response.lost_focus() || response.clicked_elsewhere() {
                                        ui.memory_mut(|mem| mem.data.remove::<String>(id));

                                        if text != original_path {
                                            return Some(UniversalPath::from(&text));
                                        }
                                    }

                                    if !is_valid {
                                        response.on_hover_text("Invalid path");
                                    }

                                    None
                                };
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
                                        ui.vertical(|ui| {
                                            *ctx.set_file_1_root_request = editable_path_test(
                                                ui,
                                                "file_1_path_editor_path".into(),
                                                &ctx.file_source_root.clone().unwrap_or_default(),
                                                ctx.file_source_root_valid,
                                                ctx.file_source_loading,
                                            );
                                            ui.separator();
                                            *ctx.load_file_1_request = editable_path_test(
                                                ui,
                                                "file_1_path_editor".into(),
                                                &ctx.file_source_path,
                                                ctx.file_source_path_valid,
                                                ctx.file_source_loading,
                                            );
                                            ui.separator();
                                        });
                                    });
                                    header.col(|_| {});
                                    header.col(|ui| {
                                        ui.vertical(|ui| {
                                            *ctx.set_file_2_root_request = editable_path_test(
                                                ui,
                                                "file_2_path_editor_path".into(),
                                                &ctx.file_target_root.clone().unwrap_or_default(),
                                                ctx.file_target_root_valid,
                                                ctx.file_target_loading,
                                            );
                                            ui.separator();
                                            *ctx.load_file_2_request = editable_path_test(
                                                ui,
                                                "file_2_path_editor".into(),
                                                &ctx.file_target_path,
                                                ctx.file_target_path_valid,
                                                ctx.file_target_loading,
                                            );
                                            ui.separator();
                                        });
                                    });
                                })
                                .body(|body| {
                                    if ctx.image_view.is_some() {
                                        image_left_width = Some(body.widths()[0]);
                                        return;
                                    }
                                    if let Some(hex_view) = &ctx.hex_view {
                                        hex::table_body(
                                            body,
                                            hex_view,
                                            row_height,
                                            &mut left_rect,
                                            &mut right_rect,
                                        );
                                        return;
                                    }
                                    if do_not_render_diff {
                                        return Default::default();
                                    }

                                    let widths = body.widths().to_vec();
                                    let max = [
                                        h_scroll::max_offset(content_widths[0], widths[0]),
                                        h_scroll::max_offset(content_widths[1], widths[2]),
                                    ];
                                    let linked = *ctx.h_scroll_linked;
                                    let mut offsets = h_scroll::clamp(
                                        [*ctx.scroll_left, *ctx.scroll_right],
                                        max,
                                        linked,
                                    );
                                    if let Some(side) = wheel_side {
                                        let offset = offsets[h_scroll::side_index(side)] - wheel_delta;
                                        offsets = h_scroll::set(offsets, side, offset, max, linked);
                                    }
                                    [*ctx.scroll_left, *ctx.scroll_right] = offsets;
                                    h_scroll_max = Some(max);
                                    let [sl, sr] = offsets;
                                    body.rows(
                                        row_height,
                                        diff_rows.map(|f| f.len()).unwrap_or_default(),
                                        |mut row| {
                                            if let Some(rows) = diff_rows {
                                                let row_index = row.index();
                                                let diff_row = &rows[row.index()];
                                                let is_highlighted =
                                                    ctx.active_highlights.contains(&row_index);

                                                log::trace!("==LEFT==");
                                                row.col(|ui| {
                                                    show_scrolled(ui, sl, |ui| {
                                                        Self::render_side_row(
                                                            ui,
                                                            ctx.diff_ctx
                                                                .map(|f| f.input.file_1.clone())
                                                                .unwrap_or_default(),
                                                            ctx.diff_ctx
                                                                .map(|f| f.input.file_2.clone())
                                                                .unwrap_or_default(),
                                                            &diff_row.left,
                                                            widths[0] + sl,
                                                            is_highlighted,
                                                            active_side == ActiveSide::Left,
                                                            ctx.code_language,
                                                        );
                                                    });
                                                    left_rect = left_rect.union(ui.max_rect());
                                                });

                                                row.col(|ui| {
                                                    let has_op =
                                                        |tokens: &[(DiffResult, _, _)], op| {
                                                            tokens.iter().any(|f| {
                                                                !f.0.hide_in_diff
                                                                    && f.0.operation == op
                                                            })
                                                        };

                                                    let symbol =
                                                |contains_delete: bool, contains_insert: bool| {
                                                    match (contains_delete, contains_insert) {
                                                        (true, true) => "≠",
                                                        (true, false) => "-",
                                                        (false, true) => "+",
                                                        (false, false) => " ",
                                                    }
                                                };

                                                    let symbol_text =
                                                        match (&diff_row.left, &diff_row.right) {
                                                            (
                                                                LineContent::Void,
                                                                LineContent::Code {
                                                                    tokens, ..
                                                                },
                                                            ) => symbol(
                                                                has_op(tokens, DiffOp::Delete),
                                                                has_op(tokens, DiffOp::Insert),
                                                            ),

                                                            (
                                                                LineContent::Code {
                                                                    tokens, ..
                                                                },
                                                                LineContent::Void,
                                                            ) => symbol(
                                                                has_op(tokens, DiffOp::Delete),
                                                                has_op(tokens, DiffOp::Insert),
                                                            ),

                                                            (
                                                                LineContent::Code {
                                                                    tokens: t1,
                                                                    ..
                                                                },
                                                                LineContent::Code {
                                                                    tokens: t2,
                                                                    ..
                                                                },
                                                            ) => {
                                                                let contains_delete =
                                                                    has_op(t1, DiffOp::Delete)
                                                                        || has_op(
                                                                            t2,
                                                                            DiffOp::Delete,
                                                                        );
                                                                let contains_insert =
                                                                    has_op(t1, DiffOp::Insert)
                                                                        || has_op(
                                                                            t2,
                                                                            DiffOp::Insert,
                                                                        );

                                                                symbol(
                                                                    contains_delete,
                                                                    contains_insert,
                                                                )
                                                            }
                                                            (
                                                                LineContent::Collapsed,
                                                                LineContent::Collapsed,
                                                            ) => "...",
                                                            _ => " ",
                                                        };
                                                    ui.centered_and_justified(|ui| {
                                                        ui.horizontal(|ui|{
                                                            let diff_ctx = ctx.diff_ctx;
                                                            let hunk = diff_ctx.and_then(|d| {
                                                                revert::hunk_starting_at(&d.precomputed_diffs, row_index)
                                                                    .map(|hunk| (d, hunk))
                                                            });
                                                            let mut revert_button = |ui: &mut egui::Ui, target: RevertTarget, label: &str, hover: &str| {
                                                                let Some((diff_ctx, hunk)) = hunk else {
                                                                    return;
                                                                };
                                                                match revert::check_revert(diff_ctx, target) {
                                                                    Ok(()) => {
                                                                        if ui.button(label).on_hover_text(hover).clicked() {
                                                                            *ctx.revert_request = Some(RevertRequest { hunk, target });
                                                                        }
                                                                    }
                                                                    // The pivot is persisted and easy to miss, so say why
                                                                    // instead of hiding the buttons.
                                                                    Err(refusal @ RevertRefusal::PivotActive) => {
                                                                        ui.add_enabled(false, egui::Button::new(label))
                                                                            .on_disabled_hover_text(format!("Can't revert: {refusal}"));
                                                                    }
                                                                    Err(_) => {}
                                                                }
                                                            };
                                                            // A collapsed row, or the first row of an expanded block.
                                                            let block = diff_ctx.and_then(|d| {
                                                                d.row_blocks
                                                                    .binary_search_by_key(&row_index, |b| b.rows.start)
                                                                    .ok()
                                                                    .map(|i| &d.row_blocks[i])
                                                            });
                                                            if let Some(block) = block {
                                                                let mut toggle_button = |label: &str, hover: &str, toggle: BlockToggle| {
                                                                    if ui.button(label).on_hover_text(hover).clicked() {
                                                                        *ctx.block_toggle_request = Some((block.key, toggle));
                                                                    }
                                                                };
                                                                if !block.expanded {
                                                                    toggle_button("+", "Expand the collapsed rows", BlockToggle::Expand);
                                                                }
                                                                // Only on a fully collapsed block: once rows are revealed, the
                                                                // scope expansion has been done or wouldn't add anything.
                                                                if !block.expanded && block.rows.len() == 1 {
                                                                    toggle_button("{", "Expand up to the line opening the scope of the change below", BlockToggle::ExpandToScope);
                                                                } else {
                                                                    toggle_button("-", "Collapse these rows again", BlockToggle::Collapse);
                                                                }
                                                            }
                                                            revert_button(ui, RevertTarget::Left, "<", "Replace this hunk in the left file with the right side");
                                                            ui.add(
                                                                egui::Label::new(
                                                                    egui::RichText::new(symbol_text)
                                                                        .color(egui::Color32::DARK_GRAY),
                                                                )
                                                                .selectable(false),
                                                            );
                                                            revert_button(ui, RevertTarget::Right, ">", "Replace this hunk in the right file with the left side");
                                                        });
                                                    });
                                                });

                                                log::trace!("==RIGHT==");
                                                row.col(|ui| {
                                                    show_scrolled(ui, sr, |ui| {
                                                        Self::render_side_row(
                                                            ui,
                                                            ctx.diff_ctx
                                                                .map(|f| f.input.file_1.clone())
                                                                .unwrap_or_default(),
                                                            ctx.diff_ctx
                                                                .map(|f| f.input.file_2.clone())
                                                                .unwrap_or_default(),
                                                            &diff_row.right,
                                                            widths[2] + sr,
                                                            is_highlighted,
                                                            active_side == ActiveSide::Right,
                                                            ctx.code_language,
                                                        );
                                                    });
                                                    right_rect = right_rect.union(ui.max_rect());
                                                });
                                            }
                                        },
                                    );
                                });
                            if let (Some(image_view), Some(left_width)) =
                                (ctx.image_view.as_deref_mut(), image_left_width)
                            {
                                image::show(ui, image_view, left_width);
                            }
                        });
                })
                .response
                .rect;
            }

            if waiting_for_diff {
                ui.vertical_centered(|ui| {
                    // Hack to simulate ui.centered_and_justified to make spinner work
                    let height = ui.available_height();
                    ui.add_space(height / 2.0 - 30.0);

                    ui.label("Waiting for diff results...");
                    ui.add_space(8.0);
                    ui.add(egui::Spinner::new().size(64.0));
                });
            } else if do_not_render_diff {
                ui.centered_and_justified(|ui| {
                    ui.label("Load Source & Target files to see diff.");
                });
            }

            // The bars scroll text rows; hex rows have a fixed width, images pan.
            if let Some(max) = h_scroll_max
                && !(waiting_for_diff || do_not_render_diff || other_viewer)
            {
                ui.add_space(4.0);
                let linked = *ctx.h_scroll_linked;
                let mut offsets = [*ctx.scroll_left, *ctx.scroll_right];
                let side_rects = [left_rect, right_rect];
                let ranges = h_scroll::ranges(max, linked);
                let footer = ui
                    .allocate_space(egui::vec2(ui.available_width(), footer_height - 4.0))
                    .1;
                for side in [ActiveSide::Left, ActiveSide::Right] {
                    let i = h_scroll::side_index(side);
                    let rect =
                        egui::Rect::from_x_y_ranges(side_rects[i].x_range(), footer.y_range());
                    if let Some(offset) = h_scroll_bar(ui, rect, i, offsets[i], ranges[i]) {
                        offsets = h_scroll::set(offsets, side, offset, max, linked);
                        ui.ctx().request_repaint();
                    }
                }
                [*ctx.scroll_left, *ctx.scroll_right] = offsets;
            }
        });

        // Rows are laid out inside the table's scroll area, so clip to it: a partially visible
        // edge row would otherwise push the outline past the table.
        let active_rect = match active_side {
            ActiveSide::Left => left_rect,
            ActiveSide::Right => right_rect,
        }
        .intersect(table_rect);
        if active_rect.is_positive() {
            ui.painter().rect_stroke(
                active_rect,
                0.0,
                outline_stroke(),
                egui::StrokeKind::Inside,
            );
        }
        self.active_side.end_frame(left_rect, right_rect);

        handle_drops(
            ui,
            &mut ctx.load_file_1_request,
            &mut ctx.load_file_2_request,
            left_rect,
            right_rect,
        );

        egui_tiles::UiResponse::None
    }

    pub(super) fn render_side_row<T: RawTokenTrait>(
        ui: &mut egui::Ui,
        file_source: Option<Arc<CachedFile<T>>>,
        file_target: Option<Arc<CachedFile<T>>>,
        content: &LineContent,
        width: f32,
        is_highlighted: bool,
        selectable: bool,
        code_language: &str,
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
                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;

                        let line_num_str = if *line_num > 0 {
                            line_num.to_string()
                        } else {
                            String::new()
                        };

                        ui.add_sized(
                            [GUTTER_WIDTH, row_h],
                            egui::Label::new(
                                egui::RichText::new(&line_num_str)
                                    .color(egui::Color32::DARK_GRAY)
                                    .size(10.0),
                            )
                            .selectable(false),
                        );

                        ui.add_space(GUTTER_GAP);

                        let row_text = build_row_text(
                            tokens,
                            file_source.as_deref(),
                            file_target.as_deref(),
                        );

                        let theme = egui_extras::syntax_highlighting::CodeTheme::from_memory(
                            ui.ctx(),
                            ui.style(),
                        );
                        let mut layout_job = egui_extras::syntax_highlighting::highlight(
                            ui.ctx(),
                            ui.style(),
                            &theme,
                            &row_text.text,
                            code_language,
                        );
                        // Text matched by an ignore pattern is dimmed over the syntax colors.
                        let dimmed: Vec<_> = row_text
                            .color_overrides
                            .iter()
                            .filter(|(_, color)| *color == DIMMED)
                            .map(|(range, _)| range.clone())
                            .collect();
                        let [r, g, b, a] = DIMMED.0;
                        let dim = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
                        recolor_ranges(&mut layout_job, &dimmed, dim);

                        let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                        let ghost_galleys: Vec<(usize, Arc<egui::Galley>)> = row_text
                            .ghosts
                            .iter()
                            .map(|ghost| {
                                let [r, g, b, a] = ghost.color.0;
                                let galley = ui.fonts_mut(|fonts| {
                                    fonts.layout_no_wrap(
                                        ghost.text.clone(),
                                        font_id.clone(),
                                        egui::Color32::from_rgba_unmultiplied(r, g, b, a),
                                    )
                                });
                                (ghost.byte_offset, galley)
                            })
                            .collect();
                        let ghost_widths: Vec<(usize, f32)> = ghost_galleys
                            .iter()
                            .map(|(offset, galley)| (*offset, galley.size().x))
                            .collect();
                        insert_ghost_gaps(&mut layout_job, &ghost_widths);

                        layout_job.wrap.max_width = f32::INFINITY;
                        let galley = ui.fonts_mut(|fonts| fonts.layout_job(layout_job));
                        let ghost_xs = ghost_x_offsets(&galley, &row_text.text, &ghost_widths);
                        // The hit area spans the rest of the row, not just the text, so a press
                        // anywhere in the block starts a selection. Drag sense on both sides:
                        // egui hit-tests against the previous frame, so the side that becomes
                        // selectable on a press needs it already, or that press is lost.
                        let hit_size = egui::vec2(
                            galley.size().x.max(ui.available_width()),
                            galley.size().y.max(row_h),
                        );
                        let (hit_rect, response) =
                            ui.allocate_exact_size(hit_size, egui::Sense::click_and_drag());
                        let text_color = ui.style().visuals.text_color();
                        if selectable {
                            let marker = if tokens.iter().all(|(_, _, is_ghost)| *is_ghost) {
                                Some(COPY_MARKER_NO_LINE)
                            } else if row_text.text.is_empty() {
                                Some(COPY_MARKER_BLANK_LINE)
                            } else {
                                None
                            };
                            let galley = match marker {
                                Some(marker) => copy_marker_galley(ui, marker),
                                None => galley,
                            };
                            LabelSelectionState::label_text_selection(
                                ui,
                                &response,
                                hit_rect.left_top(),
                                galley,
                                text_color,
                                egui::Stroke::NONE,
                            );
                        } else {
                            ui.painter().add(egui::epaint::TextShape::new(
                                hit_rect.left_top(),
                                galley,
                                text_color,
                            ));
                        }
                        for ((_, ghost_galley), x) in ghost_galleys.into_iter().zip(ghost_xs) {
                            ui.painter().add(egui::epaint::TextShape::new(
                                hit_rect.left_top() + egui::vec2(x, 0.0),
                                ghost_galley,
                                text_color,
                            ));
                        }

                        ui.painter().rect_filled(
                            extended_rect,
                            0.0,
                            egui::Color32::from_rgba_unmultiplied(
                                bg.0[0], bg.0[1], bg.0[2], bg.0[3],
                            ),
                        );
                        if is_highlighted {
                            ui.painter().rect_filled(
                                extended_rect,
                                0.0,
                                egui::Color32::from_rgba_unmultiplied(255, 255, 0, 40), // Faint yellow
                            );
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

                if selectable {
                    // Full-row hit area like Code rows, so a press here starts a selection too.
                    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                        let (hit_rect, response) =
                            ui.allocate_exact_size(rect.size(), egui::Sense::click_and_drag());
                        LabelSelectionState::label_text_selection(
                            ui,
                            &response,
                            hit_rect.left_top(),
                            copy_marker_galley(ui, COPY_MARKER_NO_LINE),
                            egui::Color32::TRANSPARENT,
                            egui::Stroke::NONE,
                        );
                    });
                }
            }
            LineContent::Collapsed => {
                let fill = if is_highlighted {
                    egui::Color32::from_rgba_unmultiplied(38, 79, 120, 100) // Clear blue highlight
                } else {
                    egui::Color32::from_rgb(37, 43, 54) // Subdued slate/steel background
                };
                ui.painter().rect_filled(extended_rect, 0.0, fill);

                ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;

                        ui.add_sized(
                            [GUTTER_WIDTH, row_h],
                            egui::Label::new(
                            egui::RichText::new("|") // Mid-line ellipsis fits a gutter better than "-"
                            .color(egui::Color32::from_gray(100))
                            .size(12.0))
                            .selectable(false)
                        );

                        ui.add_space(GUTTER_GAP);

                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("Collapsed context")
                                    .color(egui::Color32::from_gray(130))
                                    .size(11.0),
                            )
                            .selectable(false),
                        );

                        if selectable {
                            let galley = copy_marker_galley(ui, COPY_MARKER_NO_LINE);
                            let (hit_rect, response) = ui
                                .allocate_exact_size(galley.size(), egui::Sense::click_and_drag());
                            LabelSelectionState::label_text_selection(
                                ui,
                                &response,
                                hit_rect.left_top(),
                                galley,
                                egui::Color32::TRANSPARENT,
                                egui::Stroke::NONE,
                            );
                        }
                    });
                });
            }
        }
    }
}

/// Line number column of a code row, and the gap between it and the row text.
const GUTTER_WIDTH: f32 = 35.0;
const GUTTER_GAP: f32 = 4.0;

/// Width of each side's widest row: gutter, text and ghost text. Measured over all rows, since
/// the table only lays out the visible ones.
pub(super) fn side_content_widths<T: RawTokenTrait>(
    rows: &[DiffRow],
    file_source: Option<&CachedFile<T>>,
    file_target: Option<&CachedFile<T>>,
    mut glyph_width: impl FnMut(char) -> f32,
) -> [f32; 2] {
    let mut row_width = |content: &LineContent| match content {
        LineContent::Code { tokens, .. } => {
            let row_text = build_row_text(tokens, file_source, file_target);
            let ghosts: f32 = row_text
                .ghosts
                .iter()
                .map(|ghost| h_scroll::text_width(&ghost.text, &mut glyph_width))
                .sum();
            GUTTER_WIDTH
                + GUTTER_GAP
                + h_scroll::text_width(&row_text.text, &mut glyph_width)
                + ghosts
        }
        LineContent::Void | LineContent::Collapsed => 0.0,
    };
    rows.iter().fold([0.0; 2], |[left, right], row| {
        [
            left.max(row_width(&row.left)),
            right.max(row_width(&row.right)),
        ]
    })
}

/// Draws `add_contents` scrolled left by `offset` and clipped to the cell. The child isn't
/// allocated in the cell, so the unclipped column doesn't grow to the row's text width.
fn show_scrolled(ui: &mut egui::Ui, offset: f32, add_contents: impl FnOnce(&mut egui::Ui)) {
    let cell = ui.max_rect();
    let shifted = egui::Rect::from_min_size(
        cell.min - egui::vec2(offset, 0.0),
        cell.size() + egui::vec2(offset, 0.0),
    );
    let mut child = ui.new_child(UiBuilder::new().max_rect(shifted));
    // Clipping also limits hit-testing, so shifted text can't take clicks from the middle column.
    child.set_clip_rect(cell.intersect(ui.clip_rect()));
    add_contents(&mut child);
}

/// One side's horizontal scrollbar in `rect`, scrolling `range` points. Returns the offset when
/// the bar moved it.
fn h_scroll_bar(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    side: usize,
    offset: f32,
    range: f32,
) -> Option<f32> {
    if !rect.is_positive() {
        return None;
    }
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect));
    // Floating bars take no space and only show on hover.
    child.spacing_mut().scroll = egui::style::ScrollStyle::solid();
    let output = egui::ScrollArea::horizontal()
        .id_salt(("h_scroll_bar", side))
        .auto_shrink([false, true])
        .scroll_bar_visibility(ScrollBarVisibility::AlwaysVisible)
        .scroll_offset(egui::vec2(offset, 0.0))
        .show(&mut child, |ui| {
            ui.allocate_space(egui::vec2(rect.width() + range, 0.0));
        });
    let moved = output.state.offset.x;
    (moved != offset).then_some(moved)
}

/// Toolbar button with a popup editing the ignore patterns. Patterns are recompiled only when the
/// text changes; invalid ones are listed under the field and skipped by the diff.
fn ignore_patterns_btn(ui: &mut egui::Ui, button_size: Vec2, patterns: &mut IgnorePatterns) {
    let btn = egui::Button::new(egui::RichText::new("R").strong()).selected(!patterns.is_empty());
    let response = ui
        .add_sized(button_size, btn)
        .on_hover_text("Ignore Regex: text matching these patterns is ignored and dimmed");
    egui::Popup::from_toggle_button_response(&response)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.label("One regex per line, matched per line. Ignores tokens a match fully covers.");
            let mut text = patterns.text().to_owned();
            let edit = TextEdit::multiline(&mut text)
                .code_editor()
                .desired_rows(4)
                .hint_text(r"\d\d:\d\d:\d\d");
            if ui.add(edit).changed() {
                *patterns = IgnorePatterns::new(text);
            }
            for error in patterns.errors() {
                let message = format!("Line {}: {}", error.line + 1, error.message);
                let message = egui::RichText::new(message).monospace();
                ui.colored_label(ui.visuals().error_fg_color, message);
            }
        });
}

/// Auto / Text / Hex for the current pair. A warning sign carries the fallback reason when the
/// pair is shown in another viewer than it asked for.
fn viewer_override_combo(
    ui: &mut egui::Ui,
    viewer_override: &mut Option<ViewerKind>,
    fallback: Option<&str>,
) {
    let name = |kind: Option<ViewerKind>| kind.map_or("Auto", ViewerKind::name);
    egui::ComboBox::from_id_salt("viewer_override")
        .selected_text(name(*viewer_override))
        .width(50.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(viewer_override, None, name(None));
            for kind in ViewerKind::ALL {
                ui.selectable_value(viewer_override, Some(kind), name(Some(kind)));
            }
        })
        .response
        .on_hover_text("Viewer for this pair; resets when another pair is opened");
    if let Some(reason) = fallback {
        ui.add(egui::Label::new("⚠").selectable(false))
            .on_hover_text(reason);
    }
}

/// Label and per-stage tooltip of a completed diff. Plain text so the segment can move into a
/// shared status line.
fn diff_status_text(
    num_add_deletes: MyersNumAddDelete,
    times: &DiffStageTimes,
) -> (String, String) {
    let (adds, deletes) = num_add_deletes;
    (
        format!("+{adds}/-{deletes}  {:.1?}", times.total()),
        format!(
            "Line diff: {:.1?}\nToken diff: {:.1?}\nIR: {:.1?}\nRows: {:.1?}",
            times.line_diff, times.token_diff, times.diff_ir, times.diff_rows
        ),
    )
}

/// Rows that are not file text get a one-character selectable label holding one of these
/// noncharacters. egui's cross-label copy adds a blank line for any vertical gap between copied
/// labels (and skips empty ones), so without them a Void or ghost-only row would copy as a blank
/// line and consecutive real blank lines would collapse into one. `CopyMarkerPlugin` removes them.
const COPY_MARKER_NO_LINE: char = '\u{FDD0}';
const COPY_MARKER_BLANK_LINE: char = '\u{FDD1}';

fn strip_copy_markers(copied: &str) -> String {
    let (no_line, blank) = (COPY_MARKER_NO_LINE.to_string(), COPY_MARKER_BLANK_LINE.to_string());
    copied
        .split('\n')
        .filter(|line| *line != no_line)
        .map(|line| if line == blank { "" } else { line })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) struct CopyMarkerPlugin;

impl egui::Plugin for CopyMarkerPlugin {
    fn debug_name(&self) -> &'static str {
        "CopyMarkerPlugin"
    }

    fn output_hook(&mut self, output: &mut egui::FullOutput) {
        output.platform_output.commands.retain_mut(|command| match command {
            egui::OutputCommand::CopyText(text)
                if text.contains([COPY_MARKER_NO_LINE, COPY_MARKER_BLANK_LINE]) =>
            {
                *text = strip_copy_markers(text);
                !text.is_empty()
            }
            _ => true,
        });
    }
}

/// Invisible selectable stand-in for a row that holds no file text; see `COPY_MARKER_NO_LINE`.
fn copy_marker_galley(ui: &egui::Ui, marker: char) -> Arc<egui::Galley> {
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    ui.fonts_mut(|fonts| {
        fonts.layout_no_wrap(marker.to_string(), font_id, egui::Color32::TRANSPARENT)
    })
}

/// Ghost text is visual-only: it is kept out of the label text so egui can neither select nor
/// copy it, and each ghost is a blank gap in the layout job instead. `ghosts` is (byte offset
/// into the real text, ghost width) in row order. Ghosts at the end of the text need no gap
/// because nothing follows them.
fn insert_ghost_gaps(job: &mut egui::text::LayoutJob, ghosts: &[(usize, f32)]) {
    for &(offset, width) in ghosts {
        if offset >= job.text.len() {
            continue;
        }
        let mut i = job
            .sections
            .iter()
            .position(|s| s.byte_range.contains(&offset))
            .expect("layout sections cover the whole text");
        let section = &mut job.sections[i];
        if section.byte_range.start < offset {
            let mut tail = section.clone();
            tail.leading_space = 0.0;
            tail.byte_range.start = offset;
            section.byte_range.end = offset;
            job.sections.insert(i + 1, tail);
            i += 1;
        }
        job.sections[i].leading_space += width;
    }
}

/// Sets the color of `ranges` (byte ranges into the job's text), splitting sections at their ends.
fn recolor_ranges(
    job: &mut egui::text::LayoutJob,
    ranges: &[std::ops::Range<usize>],
    color: egui::Color32,
) {
    let mut split_at = |offset: usize| {
        if let Some(i) = job
            .sections
            .iter()
            .position(|s| s.byte_range.start < offset && offset < s.byte_range.end)
        {
            let section = &mut job.sections[i];
            let mut tail = section.clone();
            tail.leading_space = 0.0;
            tail.byte_range.start = offset;
            section.byte_range.end = offset;
            job.sections.insert(i + 1, tail);
        }
    };
    for range in ranges {
        split_at(range.start);
        split_at(range.end);
    }
    for section in &mut job.sections {
        let r = &section.byte_range;
        if ranges.iter().any(|range| range.start <= r.start && r.end <= range.end) {
            section.format.color = color;
        }
    }
}

/// X of each ghost relative to the galley origin: inside its gap, or past the end of the text for
/// trailing ghosts. Ghosts sharing an offset sit side by side in one gap.
fn ghost_x_offsets(galley: &egui::Galley, text: &str, ghosts: &[(usize, f32)]) -> Vec<f32> {
    let glyphs = galley.rows.first().map_or(&[][..], |row| &row.glyphs[..]);
    ghosts
        .iter()
        .enumerate()
        .map(|(i, &(offset, _))| {
            let same_offset = |(o, _): &&(usize, f32)| *o == offset;
            let gap_width: f32 = ghosts.iter().filter(same_offset).map(|g| g.1).sum();
            let before_in_gap: f32 = ghosts[..i].iter().filter(same_offset).map(|g| g.1).sum();
            let gap_start = match glyphs.get(text[..offset].chars().count()) {
                Some(glyph) => glyph.pos.x - gap_width,
                None => glyphs.last().map_or(0.0, |glyph| glyph.max_x()),
            };
            gap_start + before_in_gap
        })
        .collect()
}

fn handle_drops(
    ui: &egui::Ui,
    load_file_1_request: &mut Option<UniversalPath>,
    load_file_2_request: &mut Option<UniversalPath>,
    rect1: egui::Rect,
    rect2: egui::Rect,
) -> bool {
    // assert!(
    //     load_file_1_request.is_none() && load_file_2_request.is_none(),
    //     "File load requests should be None when handling drops"
    // );

    // let mut should_draw = false;
    let did_drop = ui.input(|i| {
        if let Some(drop_pos) = i.pointer.hover_pos() {
            // should_draw = true;
            for dropped_file in &i.raw.dropped_files {
                if let Some(path) = &dropped_file.path {
                    if rect1.contains(drop_pos) {
                        *load_file_1_request = Some(UniversalPath::from(path.clone()));
                        log::info!("File dropped on left pane: {:?}", path);
                        return true;
                    } else if rect2.contains(drop_pos) {
                        *load_file_2_request = Some(UniversalPath::from(path.clone()));
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use zdiff::{
        diff_builder::{DiffBuilderOptions, DiffRow, LineContent},
        ignore::{IgnoreOptions, IgnorePatterns},
    };

    use crate::{
        diff_ctx::DiffStageTimes,
        ui_egui::copy_harness::{CopyHarness, Side},
    };

    use super::{
        COPY_MARKER_BLANK_LINE, COPY_MARKER_NO_LINE, GUTTER_GAP, GUTTER_WIDTH, diff_status_text,
        insert_ghost_gaps, recolor_ranges, strip_copy_markers,
    };

    #[cfg(feature = "serde")]
    #[test]
    fn ignore_comments_is_persisted_with_the_diff_options() {
        let options = DiffBuilderOptions {
            ignore: IgnoreOptions {
                comments: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_value(&options).unwrap();
        assert_eq!(json["ignore_comments"], true);
        let loaded: DiffBuilderOptions = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(loaded, options);

        // State saved before the option existed still loads, with the option off.
        let mut old = json;
        old.as_object_mut().unwrap().remove("ignore_comments");
        let loaded: DiffBuilderOptions = serde_json::from_value(old).unwrap();
        assert_eq!(loaded, DiffBuilderOptions::default());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn ignore_patterns_are_persisted_with_the_diff_options() {
        // The invalid line is kept, so a typo isn't lost on restart.
        let text = "\\d\\d:\\d\\d\n(";
        let options = DiffBuilderOptions {
            ignore: IgnoreOptions {
                patterns: IgnorePatterns::new(text),
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_value(&options).unwrap();
        assert_eq!(json["ignore_regex"], text);
        let loaded: DiffBuilderOptions = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(loaded, options);
        // Loading compiles the patterns again.
        assert!(!loaded.ignore.patterns.is_empty());
        assert_eq!(loaded.ignore.patterns.errors().len(), 1);

        // State saved before the option existed still loads, with no patterns.
        let mut old = json;
        old.as_object_mut().unwrap().remove("ignore_regex");
        let loaded: DiffBuilderOptions = serde_json::from_value(old).unwrap();
        assert_eq!(loaded, DiffBuilderOptions::default());
    }

    #[test]
    fn copy_markers_drop_non_file_rows_and_keep_blank_lines() {
        let (no_line, blank) = (COPY_MARKER_NO_LINE.to_string(), COPY_MARKER_BLANK_LINE.to_string());
        let copied = [no_line.as_str(), "a", &no_line, &blank, &blank, "b", &no_line].join("\n");

        assert_eq!(strip_copy_markers(&copied), "a\n\n\nb");
    }

    fn job_with_one_section(text: &str) -> eframe::egui::text::LayoutJob {
        eframe::egui::text::LayoutJob::simple_singleline(
            text.to_owned(),
            eframe::egui::FontId::monospace(12.0),
            eframe::egui::Color32::WHITE,
        )
    }

    #[test]
    fn ghost_gap_splits_the_section_without_changing_the_text() {
        let mut job = job_with_one_section("abcd");

        insert_ghost_gaps(&mut job, &[(2, 7.0)]);

        assert_eq!(job.text, "abcd");
        let sections: Vec<_> = job
            .sections
            .iter()
            .map(|s| (s.byte_range.clone(), s.leading_space))
            .collect();
        assert_eq!(sections, [(0..2, 0.0), (2..4, 7.0)]);
    }

    #[test]
    fn recolor_splits_sections_at_the_range_ends_without_changing_the_text() {
        use eframe::egui::Color32;
        let dim = Color32::from_rgba_unmultiplied(1, 2, 3, 4);
        let colors = |job: &eframe::egui::text::LayoutJob| -> Vec<_> {
            job.sections
                .iter()
                .map(|s| (s.byte_range.clone(), s.format.color))
                .collect()
        };

        let mut job = job_with_one_section("abcdef");
        recolor_ranges(&mut job, &[1..3, 5..6], dim);
        assert_eq!(job.text, "abcdef");
        assert_eq!(
            colors(&job),
            [
                (0..1, Color32::WHITE),
                (1..3, dim),
                (3..5, Color32::WHITE),
                (5..6, dim)
            ]
        );

        // A range across a section boundary recolors both parts.
        let mut job = job_with_one_section("abc");
        job.append("def", 0.0, job.sections[0].format.clone());
        recolor_ranges(&mut job, &[2..4], dim);
        assert_eq!(
            colors(&job),
            [
                (0..2, Color32::WHITE),
                (2..3, dim),
                (3..4, dim),
                (4..6, Color32::WHITE)
            ]
        );
    }

    #[test]
    fn ghost_gaps_at_the_same_offset_add_up_and_trailing_ghosts_add_no_gap() {
        let mut job = job_with_one_section("abcd");

        insert_ghost_gaps(&mut job, &[(0, 1.0), (0, 2.0), (4, 5.0)]);

        let sections: Vec<_> = job
            .sections
            .iter()
            .map(|s| (s.byte_range.clone(), s.leading_space))
            .collect();
        assert_eq!(sections, [(0..4, 3.0)]);
    }

    #[test]
    fn diff_status_shows_counts_and_total_time_with_each_stage_in_the_tooltip() {
        let times = DiffStageTimes {
            line_diff: Duration::from_micros(12_340),
            token_diff: Duration::from_micros(1_000),
            diff_ir: Duration::from_micros(500),
            diff_rows: Duration::from_millis(2),
        };

        let (label, tooltip) = diff_status_text((7, 3), &times);

        assert_eq!(label, "+7/-3  15.8ms");
        assert_eq!(
            tooltip,
            "Line diff: 12.3ms\nToken diff: 1.0ms\nIR: 500.0µs\nRows: 2.0ms"
        );
    }

    const SOURCE: &str = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
    const TARGET: &str = "fn main() {\n    let x = 1;\n    let z = 3 + 4;\n}\n";

    #[test]
    fn single_row_selection_copies_that_rows_text() {
        let mut harness = CopyHarness::new(SOURCE, TARGET, &DiffBuilderOptions::default());

        let copied = harness.drag_and_copy(Side::Left, (1, 4), (1, 9));
        assert_eq!(copied.as_deref(), Some("let x"));

        let copied = harness.drag_and_copy(Side::Right, (2, 4), (2, 9));
        assert_eq!(copied.as_deref(), Some("let z"));
    }


    // Same on both sides except the last line, so rows 0..=2 carry no ghosts and are identical
    // on both sides.
    const SHARED_SOURCE: &str = "fn main() {\n    foo(bar, baz);\n    let x = 1;\n}\nold\n";
    const SHARED_TARGET: &str = "fn main() {\n    foo(bar, baz);\n    let x = 1;\n}\nnew\n";

    fn shared_harness() -> CopyHarness {
        CopyHarness::new(SHARED_SOURCE, SHARED_TARGET, &DiffBuilderOptions::default())
    }

    #[test]
    fn multi_row_selection_copies_exact_file_text() {
        let mut harness = shared_harness();

        let expected = Some("fn main() {\n    foo(bar, baz);\n    let x = 1;\n}");
        let copied = harness.drag_and_copy(Side::Left, (0, 0), (3, 1));
        assert_eq!(copied.as_deref(), expected);
        let copied = harness.drag_and_copy(Side::Right, (0, 0), (3, 1));
        assert_eq!(copied.as_deref(), expected);
    }

    #[test]
    fn multi_row_selection_keeps_punctuation_spacing() {
        let mut harness = shared_harness();

        let copied = harness.drag_and_copy(Side::Left, (1, 4), (1, 18));
        assert_eq!(copied.as_deref(), Some("foo(bar, baz);"));
    }

    #[test]
    fn partial_first_and_last_row() {
        let mut harness = shared_harness();

        let copied = harness.drag_and_copy(Side::Left, (0, 3), (2, 9));
        assert_eq!(
            copied.as_deref(),
            Some("main() {\n    foo(bar, baz);\n    let x")
        );
    }

    #[test]
    fn upward_drag_copies_same_text_as_downward() {
        let mut harness = shared_harness();

        let down = harness.drag_and_copy(Side::Left, (0, 3), (2, 9));
        let up = harness.drag_and_copy(Side::Left, (2, 9), (0, 3));
        assert_eq!(up, down);
    }

    #[test]
    fn trailing_whitespace_is_kept() {
        let mut harness = CopyHarness::new("a  \nb\n", "a  \nb\n", &DiffBuilderOptions::default());

        let copied = harness.drag_and_copy(Side::Left, (0, 0), (1, 1));
        assert_eq!(copied.as_deref(), Some("a  \nb"));
    }

    #[test]
    fn crlf_rows_are_joined_with_lf() {
        let mut harness = CopyHarness::new("a\r\nb\r\n", "a\r\nb\r\n", &DiffBuilderOptions::default());

        let copied = harness.drag_and_copy(Side::Left, (0, 0), (1, 1));
        assert_eq!(copied.as_deref(), Some("a\nb"));
    }

    fn collapsed_row_between(row: &DiffRow) -> DiffRow {
        DiffRow {
            left: LineContent::Collapsed,
            right: LineContent::Collapsed,
            ..row.clone()
        }
    }

    /// Phantom blank lines from rows without a label are a known gap, handled with the ghost
    /// fidelity work; this only pins that no placeholder text reaches the clipboard.
    fn assert_only_real_text(copied: Option<String>) {
        let copied = copied.expect("copy command emitted");
        let without_newlines: String = copied.chars().filter(|c| *c != '\n').collect();
        assert_eq!(without_newlines, "fn main() {    let x = 1;");
    }

    #[test]
    fn collapsed_row_contributes_nothing() {
        let mut harness = shared_harness();
        let collapsed = collapsed_row_between(&harness.rows()[1]);
        harness.set_row(1, collapsed);

        assert_only_real_text(harness.drag_and_copy(Side::Left, (0, 0), (2, 14)));
    }

    #[test]
    fn copy_skips_a_collapsed_block_and_includes_it_once_expanded() {
        use std::{
            collections::BTreeMap,
            sync::{Arc, atomic::AtomicBool},
        };

        use crate::diff_ctx::{expand_rows, finalize_diff_rows};

        // Lines 1 and 12 differ, so one context row each leaves lines 3..=10 collapsed.
        let source: String = (1..=12).map(|n| format!("line {n}\n")).collect();
        let target = source
            .replace("line 1\n", "edit 1\n")
            .replace("line 12\n", "edit 12\n");
        let options = DiffBuilderOptions {
            diff_only_with_extra_rows: Some(1),
            ..Default::default()
        };
        let mut harness = CopyHarness::new(&source, &target, &options);
        let (collapsed, _, blocks) = finalize_diff_rows(
            harness.rows().to_vec(),
            &options,
            source.lines().count(),
            target.lines().count(),
            &Arc::new(AtomicBool::new(false)),
        )
        .expect("not cancelled");
        let [ref block] = blocks[..] else {
            panic!("expected one collapsed block: {collapsed:#?}");
        };

        harness.set_rows(collapsed.clone());
        let copied = harness
            .drag_and_copy(Side::Left, (1, 0), (3, 7))
            .expect("copy command emitted");
        let without_newlines: String = copied.chars().filter(|c| *c != '\n').collect();
        assert_eq!(without_newlines, "line 2line 11");

        let (expanded, _) = expand_rows(&collapsed, &blocks, &BTreeMap::from([(block.row, 0)]));
        harness.set_rows(expanded);
        let expected: Vec<String> = (2..=11).map(|n| format!("line {n}")).collect();
        let copied = harness.drag_and_copy(Side::Left, (1, 0), (10, 7));
        assert_eq!(copied, Some(expected.join("\n")));
    }

    #[test]
    fn void_row_contributes_nothing() {
        let mut harness = shared_harness();
        let void = DiffRow {
            left: LineContent::Void,
            right: LineContent::Void,
            ..harness.rows()[1].clone()
        };
        harness.set_row(1, void);

        assert_only_real_text(harness.drag_and_copy(Side::Left, (0, 0), (2, 14)));
    }


    fn options_variants() -> [(&'static str, DiffBuilderOptions); 2] {
        [
            ("default", DiffBuilderOptions::default()),
            (
                "ignore_whitespace",
                DiffBuilderOptions {
                    ignore: IgnoreOptions {
                        whitespace: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
            ),
        ]
    }

    #[test]
    fn inline_ghost_tokens_add_no_characters() {
        for (name, options) in options_variants() {
            let mut harness = CopyHarness::new("let x = old(1);\nkeep\n", "let x = new(1);\nkeep\n", &options);

            let copied = harness.drag_and_copy(Side::Left, (0, 0), (1, 4));
            assert_eq!(copied.as_deref(), Some("let x = old(1);\nkeep"), "left, {name}");
            let copied = harness.drag_and_copy(Side::Right, (0, 0), (1, 4));
            assert_eq!(copied.as_deref(), Some("let x = new(1);\nkeep"), "right, {name}");
        }
    }

    #[test]
    fn inline_ghost_row_copies_partial_real_text_on_either_side_of_the_ghost() {
        let mut harness = CopyHarness::new("let x = old(1);\nkeep\n", "let x = new(1);\nkeep\n", &DiffBuilderOptions::default());

        let copied = harness.drag_and_copy(Side::Left, (0, 4), (0, 12));
        assert_eq!(copied.as_deref(), Some("x = old("));
        let copied = harness.drag_and_copy(Side::Left, (0, 9), (1, 2));
        assert_eq!(copied.as_deref(), Some("ld(1);\nke"));
    }

    #[test]
    fn selection_spanning_a_ghost_only_row_joins_the_surrounding_real_lines() {
        for (name, options) in options_variants() {
            let mut harness = CopyHarness::new("a\nb\n", "a\nx\nb\n", &options);

            let copied = harness.drag_and_copy(Side::Left, (0, 0), (2, 1));
            assert_eq!(copied.as_deref(), Some("a\nb"), "left, {name}");
            let copied = harness.drag_and_copy(Side::Right, (0, 0), (2, 1));
            assert_eq!(copied.as_deref(), Some("a\nx\nb"), "right, {name}");
        }
    }

    #[test]
    fn real_blank_line_inside_a_selection_is_preserved() {
        for (name, options) in options_variants() {
            let mut harness = CopyHarness::new("a\n\nb\n", "a\n\nb\n", &options);

            let copied = harness.drag_and_copy(Side::Left, (0, 0), (2, 1));
            assert_eq!(copied.as_deref(), Some("a\n\nb"), "{name}");
        }
    }

    #[test]
    fn hidden_whitespace_tokens_are_copied_in_ignore_whitespace_mode() {
        let options = DiffBuilderOptions {
            ignore: IgnoreOptions {
                whitespace: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut harness = CopyHarness::new("a  b\nc\n", "a b\nc\n", &options);

        let copied = harness.drag_and_copy(Side::Left, (0, 0), (1, 1));
        assert_eq!(copied.as_deref(), Some("a  b\nc"));
        let copied = harness.drag_and_copy(Side::Right, (0, 0), (1, 1));
        assert_eq!(copied.as_deref(), Some("a b\nc"));
    }

    const REAL_SOURCE: &str = "trait Processor {\n    fn run(&self);\n}\n\nstruct Item {\n    id: u64,\n    inner: u8,\n}\n\nfn go() {\n    println!(\"hello\");\n    old_body();\n}\n";
    const REAL_TARGET: &str = "trait NewProcessor {\n    fn run(&self);\n}\n\nstruct Item {\n    id: usize,\n}\n\nfn go() {\n    println!(\"hello world\");\n    new_body();\n    more();\n}\n";

    #[test]
    fn whole_diff_copies_each_files_exact_text() {
        for (name, options) in options_variants() {
            let mut harness = CopyHarness::new(REAL_SOURCE, REAL_TARGET, &options);
            let last = harness.rows().len() - 1;

            let copied = harness.drag_and_copy(Side::Left, (0, 0), (last, 1));
            assert_eq!(copied.as_deref(), Some(REAL_SOURCE.trim_end()), "left, {name}");
            let copied = harness.drag_and_copy(Side::Right, (0, 0), (last, 1));
            assert_eq!(copied.as_deref(), Some(REAL_TARGET.trim_end()), "right, {name}");
        }
    }

    // Each side has one line the other lacks. Ghost rows are off so the missing line is a Void
    // row, not ghost text, and a side's copy therefore holds only its own marker.
    const SIDES_SOURCE: &str = "top\nLEFTONLY\nm1\nm2\nm3\nbottom\n";
    const SIDES_TARGET: &str = "top\nm1\nm2\nm3\nRIGHTONLY\nbottom\n";

    fn sides_harness() -> CopyHarness {
        let options = DiffBuilderOptions {
            ghost_rows: false,
            ..Default::default()
        };
        CopyHarness::new(SIDES_SOURCE, SIDES_TARGET, &options)
    }

    fn code_rows(harness: &CopyHarness, side: Side) -> (usize, usize) {
        let is_code = |row: &DiffRow| {
            matches!(
                match side {
                    Side::Left => &row.left,
                    Side::Right => &row.right,
                },
                LineContent::Code { .. }
            )
        };
        let rows = harness.rows();
        let first = rows.iter().position(is_code).expect("side has code rows");
        let last = rows.iter().rposition(is_code).expect("side has code rows");
        (first, last)
    }

    fn whole_side(harness: &mut CopyHarness, side: Side) -> String {
        let (first, last) = code_rows(harness, side);
        harness
            .drag_and_copy(side, (first, 0), (last, usize::MAX))
            .expect("selection exists")
    }

    fn assert_only(copied: &str, own: &str, other: &str) {
        assert!(copied.contains(own), "missing {own}: {copied:?}");
        assert!(!copied.contains(other), "contains {other}: {copied:?}");
    }

    #[test]
    fn first_press_on_the_inactive_side_starts_a_selection_there() {
        let mut harness = sides_harness();

        // Left is active at startup, so this press is the first one on the inactive side.
        let copied = whole_side(&mut harness, Side::Right);
        assert_only(&copied, "RIGHTONLY", "LEFTONLY");
    }

    #[test]
    fn switching_sides_leaves_only_the_new_sides_selection() {
        let mut harness = sides_harness();

        assert_only(&whole_side(&mut harness, Side::Left), "LEFTONLY", "RIGHTONLY");
        assert_only(&whole_side(&mut harness, Side::Right), "RIGHTONLY", "LEFTONLY");
        assert_only(&whole_side(&mut harness, Side::Left), "LEFTONLY", "RIGHTONLY");
    }

    #[test]
    fn drag_ending_on_the_opposite_side_copies_only_the_active_side() {
        let mut harness = sides_harness();
        let (left_first, left_last) = code_rows(&harness, Side::Left);
        let (right_first, right_last) = code_rows(&harness, Side::Right);

        let copied = harness
            .drag_across_and_copy(
                (Side::Left, (left_first, 0)),
                (Side::Right, (right_last, 3)),
            )
            .expect("selection exists");
        assert!(!copied.contains("RIGHTONLY"), "{copied:?}");

        let copied = harness
            .drag_across_and_copy(
                (Side::Right, (right_first, 0)),
                (Side::Left, (left_last, 3)),
            )
            .expect("selection exists");
        assert!(!copied.contains("LEFTONLY"), "{copied:?}");
    }

    #[test]
    fn press_in_the_blank_part_of_a_row_starts_a_selection() {
        let mut harness = shared_harness();

        let copied = harness.drag_from_blank_and_copy(Side::Left, 0, (2, 5));
        assert_eq!(
            copied.as_deref(),
            Some("fn main() {\n    foo(bar, baz);\n    l")
        );
    }

    #[test]
    fn escape_clears_the_selection() {
        let mut harness = sides_harness();

        whole_side(&mut harness, Side::Left);
        harness.press_escape();
        assert_eq!(harness.copy(), None);
    }

    #[test]
    fn recompute_leaves_a_valid_or_cleared_selection() {
        let mut harness = sides_harness();
        whole_side(&mut harness, Side::Left);

        harness.set_row(
            2,
            DiffRow {
                left: LineContent::Void,
                right: LineContent::Void,
            },
        );
        if let Some(copied) = harness.copy() {
            for line in copied.lines().filter(|l| !l.is_empty()) {
                assert!(SIDES_SOURCE.lines().any(|s| s == line), "garbled line {line:?}");
            }
        }
    }

    // One point per char, so a width is the gutter plus a char count.
    fn char_count_widths(source: &str, target: &str) -> [f32; 2] {
        CopyHarness::new(source, target, &DiffBuilderOptions::default()).content_widths(|_| 1.0)
    }

    #[test]
    fn content_width_is_the_widest_rows_gutter_and_text_on_each_side() {
        let text_x = GUTTER_WIDTH + GUTTER_GAP;
        let text = "short\nthe longest line\nend\n";
        assert_eq!(char_count_widths(text, text), [text_x + 16.0; 2]);
        // An inserted line is a ghost on the other side.
        assert_eq!(
            char_count_widths("short\n", "short\nadded line\n"),
            [text_x + 10.0; 2]
        );
        // No rows, no extent.
        assert_eq!(char_count_widths("", ""), [0.0; 2]);
    }

    #[test]
    fn content_width_includes_ghost_text() {
        let text_x = GUTTER_WIDTH + GUTTER_GAP;
        // Each side shows its own text plus the other side's changed token as a ghost.
        let widths = char_count_widths("let x = old(1);\n", "let x = brand_new(1);\n");
        assert_eq!(widths, [text_x + 15.0 + 9.0, text_x + 21.0 + 3.0]);
    }

    #[test]
    fn harness_builds_rows_with_ghost_tokens() {
        let harness = CopyHarness::new(SOURCE, TARGET, &DiffBuilderOptions::default());

        let has_ghost = harness.rows().iter().any(|row| {
            [&row.left, &row.right].into_iter().any(|side| match side {
                LineContent::Code { tokens, .. } => tokens.iter().any(|(_, _, is_ghost)| *is_ghost),
                _ => false,
            })
        });
        assert!(has_ghost, "expected at least one ghost token in {:?}", harness.rows());
    }
}
