//! Hex viewer: offset-aligned byte comparison computed off the UI thread, drawn as rows of
//! 16 bytes (offset, hex, ASCII) per side. Rows are built only for the visible window.

use std::{
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

use eframe::egui::{self, Color32, FontId, text::LayoutJob};
use zdiff::hex::{HexDiff, HexSide, hex_diff};

use crate::{diff_ctx::ScrollSpan, file::LoadedFile};

pub const HEX_ROW_BYTES: usize = 16;

const DIFFERENT_BG: Color32 = Color32::from_rgba_premultiplied(110, 30, 30, 110);
const ONLY_HERE_BG: Color32 = Color32::from_rgba_premultiplied(30, 90, 30, 110);
const HIGHLIGHT: Color32 = Color32::from_rgb(255, 210, 60);

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

pub fn hex_row_for_offset(offset: usize) -> usize {
    offset / HEX_ROW_BYTES
}

/// The rows `bytes` (non-empty) covers.
fn hex_rows_span(bytes: &Range<usize>) -> ScrollSpan {
    ScrollSpan {
        start: hex_row_for_offset(bytes.start),
        maybe_end: Some(hex_row_for_offset(bytes.end - 1)),
    }
}

/// Navigation stops: the differing ranges, then the size-mismatch tail, so a pair that only
/// differs in size still has a stop.
pub fn hex_nav_count(diff: Option<&HexDiff>) -> usize {
    diff.map_or(0, |diff| {
        diff.ranges.len() + usize::from(diff.tail.is_some())
    })
}

/// Rows and bytes of stop `cursor`, 1-based like the text conflict cursor (0 is no stop). `None`
/// past the last stop: the conflict cursor is shared with the text viewer and isn't clamped.
pub fn hex_nav_span(diff: &HexDiff, cursor: usize) -> Option<(ScrollSpan, Range<usize>)> {
    let index = cursor.checked_sub(1)?;
    let bytes = match diff.ranges.get(index) {
        Some(range) => range.clone(),
        None if index == diff.ranges.len() => diff.tail.as_ref()?.range.clone(),
        None => return None,
    };
    Some((hex_rows_span(&bytes), bytes))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HexOffsetError {
    Invalid,
    /// At or past the end of the longer side.
    OutOfRange {
        len: usize,
    },
}

impl std::fmt::Display for HexOffsetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HexOffsetError::Invalid => write!(f, "expected a decimal or 0x hex byte offset"),
            HexOffsetError::OutOfRange { len } => {
                write!(f, "offset is past the end ({len} bytes, 0x{len:X})")
            }
        }
    }
}

/// A goto offset in decimal or `0x` hex. Rejected rather than clamped when it is past the longer
/// side (`len`), so a typo doesn't silently land on the last byte.
pub fn parse_hex_offset(text: &str, len: usize) -> Result<usize, HexOffsetError> {
    let text = text.trim();
    let parsed = match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        Some(digits) if digits.bytes().all(|b| b.is_ascii_hexdigit()) => {
            usize::from_str_radix(digits, 16)
        }
        None if text.bytes().all(|b| b.is_ascii_digit()) => text.parse(),
        _ => return Err(HexOffsetError::Invalid),
    };
    let offset = parsed.map_err(|_| HexOffsetError::Invalid)?;
    if offset >= len {
        return Err(HexOffsetError::OutOfRange { len });
    }
    Ok(offset)
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

    /// One-shot, so going to the same offset again scrolls again.
    goto_offset: Option<usize>,
    /// The stop last scrolled to, so the table is scrolled once per cursor change, not pinned.
    last_nav: Option<Range<usize>>,
    /// Bytes of the last goto or stop, drawn on both sides.
    highlight: Option<Range<usize>>,
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
            goto_offset: None,
            last_nav: None,
            highlight: None,
        }
    }
}

pub(super) fn is_same_side(a: &Option<LoadedFile>, b: &Option<LoadedFile>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.is_same_load(b),
        _ => false,
    }
}

impl HexDiffProcessor {
    /// No-op for the pair already requested. Returns whether the pair changed, which also resets
    /// navigation; the caller resets the conflict cursor.
    pub fn request(&mut self, file_1: Option<LoadedFile>, file_2: Option<LoadedFile>) -> bool {
        if is_same_side(&self.file_1, &file_1) && is_same_side(&self.file_2, &file_2) {
            return false;
        }

        self.cancel_flag.store(true, Ordering::Release);
        self.cancel_flag = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.diff = None;
        self.in_progress = false;
        self.file_1 = file_1;
        self.file_2 = file_2;
        self.goto_offset = None;
        self.last_nav = None;
        self.highlight = None;

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
        true
    }

    pub fn nav_count(&self) -> usize {
        hex_nav_count(self.diff.as_ref())
    }

    /// Length of the longer side: goto offsets must be below it.
    pub fn max_len(&self) -> usize {
        let len = |file: &Option<LoadedFile>| file.as_ref().map_or(0, |f| f.bytes().len());
        len(&self.file_1).max(len(&self.file_2))
    }

    pub fn goto(&mut self, offset: usize) {
        log::info!("Goto byte offset: {offset} (0x{offset:X})");
        self.goto_offset = Some(offset);
    }

    /// The rows to scroll to this frame, from a pending goto or a changed conflict cursor (goto
    /// wins, as in the text viewer), and highlights the target bytes.
    pub fn scroll_to_row(&mut self, conflict_cursor: usize) -> Option<ScrollSpan> {
        let nav = self
            .diff
            .as_ref()
            .and_then(|diff| hex_nav_span(diff, conflict_cursor))
            .map(|(_, bytes)| bytes);
        let nav_changed = nav != self.last_nav;
        self.last_nav = nav.clone();

        let goto = self.goto_offset.take().map(|offset| offset..offset + 1);
        let target = goto.or(nav.filter(|_| nav_changed))?;
        let span = hex_rows_span(&target);
        self.highlight = Some(target);
        Some(span)
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
            highlight: self.highlight.clone(),
        }
    }
}

pub struct HexViewCtx<'a> {
    pub file_1: Option<&'a LoadedFile>,
    pub file_2: Option<&'a LoadedFile>,
    pub diff: Option<&'a HexDiff>,
    pub computing: bool,
    pub highlight: Option<Range<usize>>,
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
                side_row_label(ui, bytes_1, &states_1, row_index, ctx.highlight.as_ref());
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
                side_row_label(ui, bytes_2, &states_2, row_index, ctx.highlight.as_ref());
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
    highlight: Option<&Range<usize>>,
) {
    let font_id = egui::TextStyle::Monospace.resolve(ui.style());
    let text_color = ui.style().visuals.text_color();
    let job = side_row_job(bytes, states, row, highlight, font_id, text_color);
    ui.add(egui::Label::new(job).selectable(false).extend());
}

fn side_row_job(
    bytes: &[u8],
    states: &[HexByteState; HEX_ROW_BYTES],
    row: usize,
    highlight: Option<&Range<usize>>,
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
    // The goto or navigation target is recolored and underlined, so the diff background stays.
    // Not on the shorter side's missing bytes.
    let append_byte = |job: &mut LayoutJob, text: &str, i: usize, state: HexByteState| {
        let marked = state != HexByteState::Missing
            && highlight.is_some_and(|range| range.contains(&(row_start + i)));
        job.append(
            text,
            0.0,
            egui::TextFormat {
                font_id: font_id.clone(),
                color: if marked { HIGHLIGHT } else { text_color },
                background: background(state),
                underline: if marked {
                    egui::Stroke::new(1.0, HIGHLIGHT)
                } else {
                    egui::Stroke::NONE
                },
                ..Default::default()
            },
        );
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
        append_byte(&mut job, &hex, i, state);
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
        append_byte(&mut job, &ch.to_string(), i, state);
    }
    job
}

#[cfg(test)]
mod tests {
    use zdiff::hex::HexTail;

    use super::*;

    fn diff(ranges: Vec<Range<usize>>, tail: Option<HexTail>) -> HexDiff {
        HexDiff { ranges, tail }
    }

    fn span(start: usize, end: usize) -> ScrollSpan {
        ScrollSpan {
            start,
            maybe_end: Some(end),
        }
    }

    #[test]
    fn offset_maps_to_its_16_byte_row() {
        assert_eq!(hex_row_for_offset(0), 0);
        assert_eq!(hex_row_for_offset(15), 0);
        assert_eq!(hex_row_for_offset(16), 1);
        assert_eq!(hex_row_for_offset(0x1234), 0x123);
    }

    #[test]
    fn range_index_maps_to_the_rows_the_range_covers() {
        let d = diff(vec![3..4, 20..40, 47..49], None);
        assert_eq!(hex_nav_count(Some(&d)), 3);
        // The cursor is 1-based like the text conflict cursor; 0 is no stop.
        assert_eq!(hex_nav_span(&d, 0), None);
        assert_eq!(hex_nav_span(&d, 1), Some((span(0, 0), 3..4)));
        assert_eq!(hex_nav_span(&d, 2), Some((span(1, 2), 20..40)));
        // A range crossing a row boundary covers both rows.
        assert_eq!(hex_nav_span(&d, 3), Some((span(2, 3), 47..49)));
    }

    #[test]
    fn the_size_mismatch_tail_is_the_last_stop() {
        let d = diff(
            vec![5..6],
            Some(HexTail {
                longer: HexSide::Second,
                range: 32..100,
            }),
        );
        assert_eq!(hex_nav_count(Some(&d)), 2);
        assert_eq!(hex_nav_span(&d, 2), Some((span(2, 6), 32..100)));

        let size_only = diff(
            vec![],
            Some(HexTail {
                longer: HexSide::First,
                range: 8..9,
            }),
        );
        assert_eq!(hex_nav_count(Some(&size_only)), 1);
        assert_eq!(hex_nav_span(&size_only, 1), Some((span(0, 0), 8..9)));
    }

    #[test]
    fn a_cursor_past_the_last_stop_or_no_diff_has_no_stop() {
        // The shared conflict cursor can be left above the hex count (set_max doesn't clamp).
        let d = diff(vec![3..4], None);
        assert_eq!(hex_nav_span(&d, 2), None);
        assert_eq!(hex_nav_span(&diff(vec![], None), 1), None);
        assert_eq!(hex_nav_count(Some(&diff(vec![], None))), 0);
        assert_eq!(hex_nav_count(None), 0);
    }

    #[test]
    fn offset_parses_as_decimal_or_0x_hex() {
        assert_eq!(parse_hex_offset("0", 100), Ok(0));
        assert_eq!(parse_hex_offset("42", 100), Ok(42));
        assert_eq!(parse_hex_offset("0x1F", 100), Ok(0x1F));
        assert_eq!(parse_hex_offset("0X1f", 100), Ok(0x1F));
        assert_eq!(parse_hex_offset("  0x10 ", 100), Ok(16));
    }

    #[test]
    fn invalid_offset_text_is_rejected() {
        for text in [
            "", "  ", "0x", "1F", "-1", "0x-1", "12a", "0xG", "x10", "1.5",
        ] {
            assert_eq!(
                parse_hex_offset(text, 100),
                Err(HexOffsetError::Invalid),
                "{text:?}"
            );
        }
        assert_eq!(
            parse_hex_offset("0xFFFFFFFFFFFFFFFFFF", 100),
            Err(HexOffsetError::Invalid)
        );
    }

    #[test]
    fn offset_past_the_longer_side_is_rejected() {
        assert_eq!(parse_hex_offset("99", 100), Ok(99));
        assert_eq!(
            parse_hex_offset("100", 100),
            Err(HexOffsetError::OutOfRange { len: 100 })
        );
        assert_eq!(
            parse_hex_offset("0x64", 100),
            Err(HexOffsetError::OutOfRange { len: 100 })
        );
        assert_eq!(
            parse_hex_offset("0", 0),
            Err(HexOffsetError::OutOfRange { len: 0 })
        );
    }
}
