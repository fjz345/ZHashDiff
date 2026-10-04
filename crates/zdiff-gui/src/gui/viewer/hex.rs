//! Hex viewer: offset-aligned byte comparison computed off the UI thread, drawn as rows of
//! 16 bytes (offset, hex, ASCII) per side. Rows are built only for the visible window.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use eframe::egui::{self, Color32, FontId, text::LayoutJob};
use zdiff::hex::{HexDiff, HexSide, hex_diff};

use crate::file::LoadedFile;

pub const HEX_ROW_BYTES: usize = 16;

const DIFFERENT_BG: Color32 = Color32::from_rgba_premultiplied(110, 30, 30, 110);
const ONLY_HERE_BG: Color32 = Color32::from_rgba_premultiplied(30, 90, 30, 110);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexByteState {
    /// Past the end of this side.
    Missing,
    Same,
    Different,
    /// In the size-mismatch tail, so only this side has it.
    OnlyHere,
}

pub fn hex_row_count(len_1: usize, len_2: usize) -> usize {
    len_1.max(len_2).div_ceil(HEX_ROW_BYTES)
}

/// Byte states of one side's `row`. Without a diff (one-sided pair, or still comparing) present
/// bytes are `Same`.
pub fn hex_row_states(
    diff: Option<&HexDiff>,
    side_len: usize,
    row: usize,
) -> [HexByteState; HEX_ROW_BYTES] {
    let row_start = row * HEX_ROW_BYTES;
    let mut states = [HexByteState::Missing; HEX_ROW_BYTES];
    for (i, state) in states.iter_mut().enumerate() {
        if row_start + i < side_len {
            *state = HexByteState::Same;
        }
    }
    let Some(diff) = diff else {
        return states;
    };

    let row_end = row_start + HEX_ROW_BYTES;
    let first = diff.ranges.partition_point(|range| range.end <= row_start);
    for range in diff.ranges[first..]
        .iter()
        .take_while(|range| range.start < row_end)
    {
        for offset in range.start.max(row_start)..range.end.min(row_end) {
            states[offset - row_start] = HexByteState::Different;
        }
    }
    if let Some(tail) = &diff.tail {
        for offset in tail.range.start.max(row_start)..tail.range.end.min(row_end) {
            // Tail bytes exist only on the longer side; on the shorter one they stay Missing.
            if offset < side_len {
                states[offset - row_start] = HexByteState::OnlyHere;
            }
        }
    }
    states
}

/// Runs the comparison for the current pair on a worker thread. A new pair cancels the running
/// one, and a result for an older pair is dropped.
#[derive(Debug)]
pub struct HexDiffProcessor {
    file_1: Option<LoadedFile>,
    file_2: Option<LoadedFile>,
    diff: Option<HexDiff>,
    in_progress: bool,
    generation: u64,
    cancel_flag: Arc<AtomicBool>,
    channel: (mpsc::Sender<(u64, HexDiff)>, mpsc::Receiver<(u64, HexDiff)>),
}

impl Default for HexDiffProcessor {
    fn default() -> Self {
        Self {
            file_1: None,
            file_2: None,
            diff: None,
            in_progress: false,
            generation: 0,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            channel: mpsc::channel(),
        }
    }
}

fn is_same_side(a: &Option<LoadedFile>, b: &Option<LoadedFile>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.is_same_load(b),
        _ => false,
    }
}

impl HexDiffProcessor {
    /// No-op for the pair already requested.
    pub fn request(&mut self, file_1: Option<LoadedFile>, file_2: Option<LoadedFile>) {
        if is_same_side(&self.file_1, &file_1) && is_same_side(&self.file_2, &file_2) {
            return;
        }

        self.cancel_flag.store(true, Ordering::Release);
        self.cancel_flag = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.diff = None;
        self.in_progress = false;
        self.file_1 = file_1;
        self.file_2 = file_2;

        if let (Some(file_1), Some(file_2)) = (&self.file_1, &self.file_2) {
            log::info!(
                "Hex diff requested:\nSource: {}\nTarget: {}",
                file_1.path(),
                file_2.path()
            );
            let (file_1, file_2) = (file_1.clone(), file_2.clone());
            let cancel_flag = self.cancel_flag.clone();
            let tx = self.channel.0.clone();
            let generation = self.generation;
            self.in_progress = true;
            std::thread::spawn(move || {
                if let Some(diff) = hex_diff(file_1.bytes(), file_2.bytes(), &cancel_flag) {
                    let _ = tx.send((generation, diff));
                }
            });
        }
    }

    pub fn poll(&mut self) {
        while let Ok((generation, diff)) = self.channel.1.try_recv() {
            if generation == self.generation {
                self.diff = Some(diff);
                self.in_progress = false;
            }
        }
    }

    pub fn view_ctx(&self) -> HexViewCtx<'_> {
        HexViewCtx {
            file_1: self.file_1.as_ref(),
            file_2: self.file_2.as_ref(),
            diff: self.diff.as_ref(),
            computing: self.in_progress,
        }
    }
}

pub struct HexViewCtx<'a> {
    pub file_1: Option<&'a LoadedFile>,
    pub file_2: Option<&'a LoadedFile>,
    pub diff: Option<&'a HexDiff>,
    pub computing: bool,
}

impl HexViewCtx<'_> {
    pub fn status_text(&self) -> String {
        if self.computing {
            return "Comparing bytes...".to_string();
        }
        let Some(diff) = self.diff else {
            return String::new();
        };
        if diff.ranges.is_empty() && diff.tail.is_none() {
            return "Identical bytes".to_string();
        }
        let mut text = format!("{} differing ranges", diff.ranges.len());
        if let Some(tail) = &diff.tail {
            let side = match tail.longer {
                HexSide::First => "left",
                HexSide::Second => "right",
            };
            text += &format!(", {} bytes only on the {side}", tail.range.len());
        }
        text
    }
}

/// Fills the diff pane's three-column table body: left side, change marker, right side.
pub fn table_body(
    body: egui_extras::TableBody<'_>,
    ctx: &HexViewCtx,
    row_height: f32,
    left_rect: &mut egui::Rect,
    right_rect: &mut egui::Rect,
) {
    let bytes_1 = ctx.file_1.map(|f| f.bytes()).unwrap_or_default();
    let bytes_2 = ctx.file_2.map(|f| f.bytes()).unwrap_or_default();
    let total_rows = hex_row_count(bytes_1.len(), bytes_2.len());

    body.rows(row_height, total_rows, |mut row| {
        let row_index = row.index();
        let states_1 = hex_row_states(ctx.diff, bytes_1.len(), row_index);
        let states_2 = hex_row_states(ctx.diff, bytes_2.len(), row_index);

        row.col(|ui| {
            if ctx.file_1.is_some() {
                side_row_label(ui, bytes_1, &states_1, row_index);
            }
            *left_rect = left_rect.union(ui.max_rect());
        });
        row.col(|ui| {
            let changed = |states: &[HexByteState]| {
                states
                    .iter()
                    .any(|s| matches!(s, HexByteState::Different | HexByteState::OnlyHere))
            };
            let marker = if changed(&states_1) || changed(&states_2) {
                "≠"
            } else {
                " "
            };
            ui.centered_and_justified(|ui| {
                ui.add(
                    egui::Label::new(egui::RichText::new(marker).color(Color32::DARK_GRAY))
                        .selectable(false),
                );
            });
        });
        row.col(|ui| {
            if ctx.file_2.is_some() {
                side_row_label(ui, bytes_2, &states_2, row_index);
            }
            *right_rect = right_rect.union(ui.max_rect());
        });
    });
}

fn side_row_label(
    ui: &mut egui::Ui,
    bytes: &[u8],
    states: &[HexByteState; HEX_ROW_BYTES],
    row: usize,
) {
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let text_color = ui.style().visuals.text_color();
    let job = side_row_job(bytes, states, row, font_id, text_color);
    ui.add(egui::Label::new(job).selectable(false).extend());
}

fn side_row_job(
    bytes: &[u8],
    states: &[HexByteState; HEX_ROW_BYTES],
    row: usize,
    font_id: FontId,
    text_color: Color32,
) -> LayoutJob {
    let row_start = row * HEX_ROW_BYTES;
    let mut job = LayoutJob::default();
    let append = |job: &mut LayoutJob, text: &str, color: Color32, background: Color32| {
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font_id.clone(),
                color,
                background,
                ..Default::default()
            },
        );
    };
    let background = |state: HexByteState| match state {
        HexByteState::Different => DIFFERENT_BG,
        HexByteState::OnlyHere => ONLY_HERE_BG,
        HexByteState::Missing | HexByteState::Same => Color32::TRANSPARENT,
    };

    append(
        &mut job,
        &format!("{row_start:08X}  "),
        Color32::from_gray(100),
        Color32::TRANSPARENT,
    );
    for (i, &state) in states.iter().enumerate() {
        let hex = match state {
            HexByteState::Missing => "  ".to_string(),
            _ => format!("{:02X}", bytes[row_start + i]),
        };
        append(&mut job, &hex, text_color, background(state));
        append(
            &mut job,
            if i == HEX_ROW_BYTES / 2 - 1 {
                "  "
            } else {
                " "
            },
            text_color,
            Color32::TRANSPARENT,
        );
    }
    append(&mut job, " ", text_color, Color32::TRANSPARENT);
    for (i, &state) in states.iter().enumerate() {
        let ch = match state {
            HexByteState::Missing => ' ',
            _ => match bytes[row_start + i] {
                byte @ 0x20..=0x7E => byte as char,
                _ => '.',
            },
        };
        append(&mut job, &ch.to_string(), text_color, background(state));
    }
    job
}
