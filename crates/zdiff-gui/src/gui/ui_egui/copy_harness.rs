//! Headless harness for the diff pane's text selection and copy behavior.
//!
//! Renders both sides of a list of diff rows through `FileDiffPane::render_side_row` in a
//! plain egui `Context` (no window, no GPU), drives synthetic pointer events and returns the
//! text of the resulting copy command.

use std::sync::{Arc, atomic::AtomicBool};

use eframe::egui::{
    self, Event, Galley, Modifiers, OutputCommand, PointerButton, Pos2, Rect, Shape,
};
use zdiff::{
    cached_file::{CachedFile, FileMetadata},
    diff_builder::{DiffBuilderOptions, DiffRow, LineContent, build_diff_rows},
    diff_ir::DiffIR,
    lexer::{LEXER_MODE_DEFAULT, LexerDefault, RawToken},
    myers::{MyersDiffAlgorithm, myers_diff_path},
    row_text::{RowText, build_row_text},
    universal_path::UniversalPath,
};

use crate::ui_egui::{
    active_side::{ActiveSide, ActiveSideState},
    diff_pane::{
        CopyMarkerPlugin, FileDiffPane, measure_wrapped_rows, show_scrolled, side_content_widths,
        wrap_width, wrapped_row_heights,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

/// A (row index, character column) position inside one side's text.
pub type RowCol = (usize, usize);

const SIDE_WIDTH: f32 = 600.0;
// Used only for rows whose renderer paints no text shape (e.g. empty rows): gutter + spacing.
const FALLBACK_TEXT_OFFSET: f32 = 39.0;

pub struct CopyHarness {
    ctx: egui::Context,
    file_source: Arc<CachedFile<RawToken>>,
    file_target: Arc<CachedFile<RawToken>>,
    rows: Vec<DiffRow>,
    time: f64,
    active_side: ActiveSideState,
    /// Horizontal scroll offset of both sides.
    h_offset: f32,
    side_widths: [f32; 2],
    wrap: bool,
}

struct FrameLayout {
    row_rects: [Vec<Rect>; 2],
    /// Per side and row: position of each real-text column plus the end of the text, read from
    /// the painted galley so gaps left for ghost text and wrapped rows are accounted for. Empty
    /// if the row paints no real text.
    col_pos: [Vec<Vec<Pos2>>; 2],
    char_width: f32,
}

fn cached_file(path: &str, contents: &str) -> CachedFile<RawToken> {
    CachedFile {
        path: UniversalPath::from(path),
        hash: String::new(),
        contents: contents.to_string(),
        tokens: LexerDefault::<RawToken>::new(contents).parse(),
        metadata: FileMetadata::new(contents),
        lexer_mode: LEXER_MODE_DEFAULT,
    }
}

fn active_side_of(side: Side) -> ActiveSide {
    match side {
        Side::Left => ActiveSide::Left,
        Side::Right => ActiveSide::Right,
    }
}

fn side_index(side: Side) -> usize {
    match side {
        Side::Left => 0,
        Side::Right => 1,
    }
}

fn pos_of(layout: &FrameLayout, (side, (row, col)): (Side, RowCol)) -> Pos2 {
    let i = side_index(side);
    let rect = layout.row_rects[i][row];
    let cols = &layout.col_pos[i][row];
    match cols.last() {
        Some(end) => cols.get(col).copied().unwrap_or(*end),
        None => Pos2::new(
            rect.left() + FALLBACK_TEXT_OFFSET + col as f32 * layout.char_width,
            rect.center().y,
        ),
    }
}

fn collect_text_shapes(shape: &Shape, out: &mut Vec<(Pos2, Arc<Galley>)>) {
    match shape {
        Shape::Text(text) => out.push((text.pos, text.galley.clone())),
        Shape::Vec(shapes) => shapes.iter().for_each(|s| collect_text_shapes(s, out)),
        _ => {}
    }
}

impl CopyHarness {
    pub fn new(source: &str, target: &str, options: &DiffBuilderOptions) -> Self {
        let file_source = cached_file("source", source);
        let file_target = cached_file("target", target);

        let cmp = |a: &RawToken, b: &RawToken| {
            a.kind == b.kind && source[a.span.clone()] == target[b.span.clone()]
        };
        let path = myers_diff_path(
            MyersDiffAlgorithm::Linear,
            &file_source.tokens,
            &file_target.tokens,
            cmp,
            &options
                .ignore
                .mask(&file_source.tokens, source, &file_target.tokens, target),
            Arc::new(AtomicBool::new(false)),
        )
        .expect("myers was not cancelled");
        let diff_ir = DiffIR::new(&path, true, Arc::new(AtomicBool::new(false)))
            .expect("diff ir was not cancelled");
        let rows = build_diff_rows(
            diff_ir,
            Some(&file_source.tokens),
            Some(&file_target.tokens),
            source,
            target,
            options,
            file_source
                .metadata
                .num_lines()
                .max(file_target.metadata.num_lines()),
        );

        // Mirrors FileDiffPane::ui, which registers it before rendering rows.
        let ctx = egui::Context::default();
        ctx.add_plugin(CopyMarkerPlugin);

        Self {
            ctx,
            file_source: Arc::new(file_source),
            file_target: Arc::new(file_target),
            rows,
            time: 0.0,
            active_side: ActiveSideState::default(),
            h_offset: 0.0,
            side_widths: [SIDE_WIDTH; 2],
            wrap: false,
        }
    }

    pub fn set_h_offset(&mut self, offset: f32) {
        self.h_offset = offset;
    }

    /// Turns wrap on with each side's column `side_widths` wide. Rows then take the pane's
    /// wrapped heights.
    pub fn set_wrap(&mut self, side_widths: [f32; 2]) {
        self.wrap = true;
        self.side_widths = side_widths;
    }

    /// The pane's horizontal extent of each side, measured with `glyph_width`.
    pub fn content_widths(&self, glyph_width: impl FnMut(char) -> f32) -> [f32; 2] {
        side_content_widths(
            &self.rows,
            Some(&*self.file_source),
            Some(&*self.file_target),
            glyph_width,
        )
    }

    /// The pane's row heights with wrap on, each side measured with `side_height`.
    pub fn wrapped_row_heights(
        &self,
        line_height: f32,
        side_height: impl FnMut(usize, &RowText) -> f32,
    ) -> Vec<f32> {
        wrapped_row_heights(
            &self.rows,
            Some(&*self.file_source),
            Some(&*self.file_target),
            line_height,
            side_height,
        )
    }

    /// The pane's row heights with wrap on, measured with egui's monospace layout at
    /// `text_widths`. Returns them with the line height.
    pub fn measured_row_heights(&self, text_widths: [f32; 2]) -> (Vec<f32>, f32) {
        measure_in_fresh_context(|fonts, font_id| {
            let heights = measure_wrapped_rows(
                fonts,
                font_id,
                &self.rows,
                Some(&*self.file_source),
                Some(&*self.file_target),
                text_widths,
            );
            (heights, fonts.row_height(font_id))
        })
    }

    pub fn rows(&self) -> &[DiffRow] {
        &self.rows
    }

    /// Replaces a row, e.g. to inject Collapsed rows that are created outside the zdiff pipeline.
    pub fn set_row(&mut self, index: usize, row: DiffRow) {
        self.rows[index] = row;
    }

    /// Replaces all rows, e.g. with the GUI's collapsed or expanded rows.
    pub fn set_rows(&mut self, rows: Vec<DiffRow>) {
        self.rows = rows;
    }

    /// Drags from `from` to `to` on `side`, copies, and returns the copied text
    /// (`None` if no copy command was emitted).
    pub fn drag_and_copy(&mut self, side: Side, from: RowCol, to: RowCol) -> Option<String> {
        self.drag_across_and_copy((side, from), (side, to))
    }

    /// Like `drag_and_copy`, but the release point may be on the other side.
    pub fn drag_across_and_copy(
        &mut self,
        from: (Side, RowCol),
        to: (Side, RowCol),
    ) -> Option<String> {
        let layout = self.run_frame(vec![]).0;
        let (p_from, p_to) = (pos_of(&layout, from), pos_of(&layout, to));
        self.drag_positions_and_copy(p_from, p_to)
    }

    /// Presses in the blank part of `side`'s row `from_row`, right of its text, drags to `to` on
    /// the same side, copies, and returns the copied text.
    pub fn drag_from_blank_and_copy(
        &mut self,
        side: Side,
        from_row: usize,
        to: RowCol,
    ) -> Option<String> {
        let layout = self.run_frame(vec![]).0;
        let rect = layout.row_rects[side_index(side)][from_row];
        let p_from = Pos2::new(rect.right() - 20.0, rect.center().y);
        let p_to = pos_of(&layout, (side, to));
        self.drag_positions_and_copy(p_from, p_to)
    }

    fn drag_positions_and_copy(&mut self, p_from: Pos2, p_to: Pos2) -> Option<String> {
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        let script = [
            vec![Event::PointerMoved(p_from)],
            vec![button(p_from, true)],
            vec![Event::PointerMoved(p_to)],
            vec![Event::PointerMoved(p_to)],
            vec![button(p_to, false)],
        ];
        for events in script {
            self.run_frame(events);
        }

        self.copy()
    }

    /// Emits a copy command and returns the copied text (`None` if nothing is selected).
    pub fn copy(&mut self) -> Option<String> {
        let output = self.run_frame(vec![Event::Copy]).1;
        output
            .platform_output
            .commands
            .into_iter()
            .rev()
            .find_map(|c| match c {
                OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
    }

    pub fn press_escape(&mut self) {
        self.run_frame(vec![Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
    }

    fn run_frame(&mut self, events: Vec<Event>) -> (FrameLayout, egui::FullOutput) {
        self.time += 0.1;
        let press_pos = events.iter().find_map(|e| match e {
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                ..
            } => Some(*pos),
            _ => None,
        });
        // Mirrors FileDiffPane::ui: the press is resolved against the previous frame's rects
        // before any row is laid out.
        let active_side = self.active_side.begin_frame(press_pos, true, true);
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1400.0, 2000.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };

        let mut layout = FrameLayout {
            row_rects: [vec![], vec![]],
            col_pos: [vec![], vec![]],
            char_width: 0.0,
        };
        let (file_source, file_target, rows) = (&self.file_source, &self.file_target, &self.rows);
        let (h_offset, side_widths, wrap) = (self.h_offset, self.side_widths, self.wrap);

        let output = self.ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                layout.char_width = ui.fonts_mut(|f| f.glyph_width(&font_id, 'M'));
                let row_h = ui.text_style_height(&egui::TextStyle::Monospace);
                // Mirrors the table's heterogeneous rows with wrap on.
                let row_heights = if wrap {
                    ui.fonts_mut(|fonts| {
                        measure_wrapped_rows(
                            fonts,
                            &font_id,
                            rows,
                            Some(&**file_source),
                            Some(&**file_target),
                            side_widths.map(wrap_width),
                        )
                    })
                } else {
                    vec![row_h; rows.len()]
                };

                ui.horizontal_top(|ui| {
                    for side in [Side::Left, Side::Right] {
                        ui.vertical(|ui| {
                            let side_width = side_widths[side_index(side)];
                            ui.set_width(side_width);
                            for (row_index, row) in rows.iter().enumerate() {
                                let content = match side {
                                    Side::Left => &row.left,
                                    Side::Right => &row.right,
                                };
                                // Mirrors the real table: a fixed-size cell per row and side, with
                                // its own id salt, and the side drawn scrolled inside it.
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(side_width, row_heights[row_index]),
                                    egui::Sense::hover(),
                                );
                                let cell = egui::UiBuilder::new()
                                    .max_rect(rect)
                                    .id_salt((side_index(side), row_index));
                                ui.scope_builder(cell, |ui| {
                                    show_scrolled(ui, h_offset, |ui| {
                                        FileDiffPane::render_side_row(
                                            ui,
                                            Some(file_source.clone()),
                                            Some(file_target.clone()),
                                            content,
                                            side_width + h_offset,
                                            wrap,
                                            false,
                                            active_side == active_side_of(side),
                                            "rs",
                                        );
                                    });
                                });
                                layout.row_rects[side_index(side)].push(rect);
                            }
                        });
                    }
                });
            });
        });

        for side in [Side::Left, Side::Right] {
            let i = side_index(side);
            layout.col_pos[i] = layout.row_rects[i]
                .iter()
                .zip(rows.iter())
                .map(|(rect, row)| {
                    let content = match side {
                        Side::Left => &row.left,
                        Side::Right => &row.right,
                    };
                    let real_text = match content {
                        LineContent::Code { tokens, .. } => {
                            build_row_text(tokens, Some(&**file_source), Some(&**file_target)).text
                        }
                        _ => String::new(),
                    };
                    col_pos(&output, *rect, h_offset, &real_text)
                })
                .collect();
        }
        let side_rect = |i: usize| {
            layout.row_rects[i]
                .iter()
                .fold(Rect::NOTHING, |acc, r| acc.union(*r))
        };
        self.active_side
            .end_frame(side_rect(0), side_rect(1));
        (layout, output)
    }
}

/// Runs `measure` with the fonts and monospace font of a fresh context, as the pane sees them.
pub fn measure_in_fresh_context<R>(
    measure: impl FnOnce(&mut egui::epaint::FontsView<'_>, &egui::FontId) -> R,
) -> R {
    let ctx = egui::Context::default();
    let mut measure = Some(measure);
    let mut result = None;
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        let font_id = egui::TextStyle::Monospace.resolve(&ctx.style());
        let measure = measure.take().expect("one pass");
        result = Some(ctx.fonts_mut(|fonts| measure(fonts, &font_id)));
    });
    result.expect("the frame ran")
}

/// Position of every column of `real_text` as painted in `rect`, plus the end of the text, each
/// at the vertical center of its galley row. Found from the painted shapes so the harness does
/// not depend on how the renderer lays out its gutter or widget margins. The shape is identified
/// by its text; the rightmost match is the code text (the gutter is left of it). Scrolled text
/// starts `h_offset` left of `rect`, so the other side's identical text is out of range. Empty
/// if the row paints no such text.
fn col_pos(output: &egui::FullOutput, rect: Rect, h_offset: f32, real_text: &str) -> Vec<Pos2> {
    if real_text.is_empty() {
        return Vec::new();
    }
    let mut shapes = Vec::new();
    for clipped in &output.shapes {
        collect_text_shapes(&clipped.shape, &mut shapes);
    }
    let Some((pos, galley)) = shapes
        .into_iter()
        .filter(|(p, g)| {
            p.y >= rect.top()
                && p.y <= rect.bottom()
                && p.x < rect.right() - h_offset
                && g.text() == real_text
        })
        .max_by(|a, b| a.0.x.total_cmp(&b.0.x))
    else {
        return Vec::new();
    };
    let row_center = |row: &egui::epaint::text::PlacedRow| pos.y + row.pos.y + row.height() / 2.0;
    let mut cols: Vec<Pos2> = galley
        .rows
        .iter()
        .flat_map(|row| {
            let y = row_center(row);
            row.glyphs
                .iter()
                .map(move |g| Pos2::new(pos.x + g.pos.x, y))
        })
        .collect();
    let last = galley.rows.last().expect("a galley has at least one row");
    let end_x = last.glyphs.last().map_or(0.0, |g| g.max_x());
    cols.push(Pos2::new(pos.x + end_x, row_center(last)));
    cols
}
