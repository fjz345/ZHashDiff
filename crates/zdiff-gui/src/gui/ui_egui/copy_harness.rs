//! Headless harness for the diff pane's text selection and copy behavior.
//!
//! Renders both sides of a list of diff rows through `FileDiffPane::render_side_row` in a
//! plain egui `Context` (no window, no GPU), drives synthetic pointer events and returns the
//! text of the resulting copy command.

use std::sync::{Arc, atomic::AtomicBool};

use eframe::egui::{self, Event, Modifiers, OutputCommand, PointerButton, Pos2, Rect, Shape};
use zdiff::{
    cached_file::{CachedFile, FileMetadata},
    diff_builder::{DiffBuilderOptions, DiffRow, build_diff_rows},
    diff_ir::DiffIR,
    lexer::{LEXER_MODE_DEFAULT, LexerDefault, RawToken},
    myers::{MyersDiffAlgorithm, myers_diff_path},
    universal_path::UniversalPath,
};

use crate::ui_egui::diff_pane::FileDiffPane;

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
}

struct FrameLayout {
    row_rects: [Vec<Rect>; 2],
    text_origins: [Vec<f32>; 2],
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

fn side_index(side: Side) -> usize {
    match side {
        Side::Left => 0,
        Side::Right => 1,
    }
}

fn collect_text_shapes(shape: &Shape, out: &mut Vec<Pos2>) {
    match shape {
        Shape::Text(text) => out.push(text.pos),
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
            Arc::new(AtomicBool::new(false)),
        )
        .expect("myers was not cancelled");
        let diff_ir = DiffIR::new(&path, true, Arc::new(AtomicBool::new(false)))
            .expect("diff ir was not cancelled");
        let rows = build_diff_rows(
            diff_ir,
            Some(&file_source.tokens),
            Some(&file_target.tokens),
            options,
            file_source
                .metadata
                .num_lines()
                .max(file_target.metadata.num_lines()),
        );

        Self {
            ctx: egui::Context::default(),
            file_source: Arc::new(file_source),
            file_target: Arc::new(file_target),
            rows,
            time: 0.0,
        }
    }

    pub fn rows(&self) -> &[DiffRow] {
        &self.rows
    }

    /// Replaces a row, e.g. to inject Collapsed rows that are created outside the zdiff pipeline.
    pub fn set_row(&mut self, index: usize, row: DiffRow) {
        self.rows[index] = row;
    }

    /// Drags from `from` to `to` on `side`, copies, and returns the copied text
    /// (`None` if no copy command was emitted).
    pub fn drag_and_copy(&mut self, side: Side, from: RowCol, to: RowCol) -> Option<String> {
        let layout = self.run_frame(vec![]).0;
        let pos_of = |(row, col): RowCol| -> Pos2 {
            let i = side_index(side);
            let rect = layout.row_rects[i][row];
            Pos2::new(
                layout.text_origins[i][row] + col as f32 * layout.char_width,
                rect.center().y,
            )
        };
        let (p_from, p_to) = (pos_of(from), pos_of(to));

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

    fn run_frame(&mut self, events: Vec<Event>) -> (FrameLayout, egui::FullOutput) {
        self.time += 0.1;
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(1400.0, 2000.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };

        let mut layout = FrameLayout {
            row_rects: [vec![], vec![]],
            text_origins: [vec![], vec![]],
            char_width: 0.0,
        };
        let (file_source, file_target, rows) = (&self.file_source, &self.file_target, &self.rows);

        let output = self.ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                let font_id = egui::TextStyle::Monospace.resolve(ui.style());
                layout.char_width = ui.fonts_mut(|f| f.glyph_width(&font_id, 'M'));

                ui.horizontal_top(|ui| {
                    for side in [Side::Left, Side::Right] {
                        ui.vertical(|ui| {
                            ui.set_width(SIDE_WIDTH);
                            for (row_index, row) in rows.iter().enumerate() {
                                let content = match side {
                                    Side::Left => &row.left,
                                    Side::Right => &row.right,
                                };
                                // Mirrors the per-row, per-side id salt of the real table.
                                let rect = ui
                                    .push_id((side_index(side), row_index), |ui| {
                                        FileDiffPane::render_side_row(
                                            ui,
                                            Some(file_source.clone()),
                                            Some(file_target.clone()),
                                            content,
                                            SIDE_WIDTH,
                                            false,
                                            "rs",
                                        );
                                    })
                                    .response
                                    .rect;
                                layout.row_rects[side_index(side)].push(rect);
                            }
                        });
                    }
                });
            });
        });

        for i in 0..2 {
            layout.text_origins[i] = layout.row_rects[i]
                .iter()
                .map(|rect| text_origin_x(&output, *rect))
                .collect();
        }
        (layout, output)
    }
}

/// X of the text painted in `rect`, found from the painted shapes so the harness does not
/// depend on how the renderer lays out its gutter or widget margins. The code text is the
/// rightmost text shape in the row; the gutter number is always left of it.
fn text_origin_x(output: &egui::FullOutput, rect: Rect) -> f32 {
    let mut positions = Vec::new();
    for clipped in &output.shapes {
        collect_text_shapes(&clipped.shape, &mut positions);
    }
    positions
        .into_iter()
        .filter(|p| p.y >= rect.top() && p.y <= rect.bottom() && p.x < rect.right())
        .map(|p| p.x)
        .reduce(f32::max)
        .unwrap_or(rect.left() + FALLBACK_TEXT_OFFSET)
}
