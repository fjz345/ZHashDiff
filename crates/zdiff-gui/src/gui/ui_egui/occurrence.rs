//! Highlighting the other occurrences of the selected text in the visible rows.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use eframe::egui::{
    self,
    text_selection::{
        LabelSelectionState, TextCursorState, text_cursor_state::byte_index_from_char_index,
    },
};

use crate::ui_egui::active_side::ActiveSide;

/// Background of a highlighted occurrence. Purple, to stay apart from the amber of find matches,
/// the red and green of changes, and egui's blue selection.
pub const OCCURRENCE_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(66, 40, 96, 110);

pub const FIND_BG: egui::Color32 = egui::Color32::from_rgba_premultiplied(90, 66, 0, 140);
pub const FIND_CURRENT_BG: egui::Color32 = egui::Color32::from_rgb(176, 104, 0);

#[derive(Clone, Copy, Debug, Default)]
pub struct RowFind<'a> {
    pub needle: &'a str,
    pub case_sensitive: bool,
    /// Ordinal of the current find hit among this row's matches.
    pub current: Option<usize>,
}

pub fn find_ranges(text: &str, find: RowFind) -> (Vec<Range<usize>>, Option<Range<usize>>) {
    if find.needle.is_empty() {
        return (Vec::new(), None);
    }
    let all = find_occurrences(text, find.needle, find.case_sensitive)
        .take(MAX_OCCURRENCES_PER_ROW)
        .collect();
    let current = find
        .current
        .and_then(|ordinal| find_occurrences(text, find.needle, find.case_sensitive).nth(ordinal));
    (all, current)
}

/// The selection inside one row's text.
struct RowSelection {
    side: ActiveSide,
    row: usize,
    cursor: TextCursorState,
    /// Byte range of the selection in the row text.
    range: Range<usize>,
    text: String,
    /// Set while a drag is above or below the row: egui's selection then spans rows, which no
    /// row's text can contain.
    spans_rows: bool,
}

/// Per-pane state of the occurrence highlight. egui keeps its label selection private (only
/// `has_selection` is public), so the selection is mirrored here for one row at a time: a press,
/// drag, double-click (word) or triple-click (line) on a selectable text row, through egui's own
/// `TextCursorState`. Keyboard changes to the selection (Shift+arrows) aren't followed.
///
/// The selection is read while the rows are drawn, so this frame's rows search the previous
/// frame's selection; a change asks for one more frame.
#[derive(Default)]
pub struct OccurrenceState {
    selection: Option<RowSelection>,
    /// Searched for in this frame's rows.
    needle: Option<String>,
    /// A primary press this frame that no text row took. egui starts a new selection elsewhere,
    /// or none, so the tracked one ends.
    unclaimed_press: bool,
    searched_rows: usize,
    search_time: Duration,
}

impl OccurrenceState {
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        // Escape and clicks outside any label clear egui's selection at the end of a frame.
        if !ctx.plugin::<LabelSelectionState>().lock().has_selection() {
            self.selection = None;
        }
        self.needle = self.selected_needle();
        self.unclaimed_press = ctx.input(|i| i.pointer.primary_pressed());
        self.searched_rows = 0;
        self.search_time = Duration::ZERO;
    }

    pub fn end_frame(&mut self, ctx: &egui::Context) {
        if self.unclaimed_press {
            self.selection = None;
        }
        if self.selected_needle() != self.needle {
            ctx.request_repaint();
        }
        if self.needle.is_some() {
            log::trace!(
                "occurrence search: {} rows in {:?}",
                self.searched_rows,
                self.search_time
            );
        }
    }

    fn selected_needle(&self) -> Option<String> {
        let selection = self.selection.as_ref().filter(|s| !s.spans_rows)?;
        occurrence_needle(&selection.text).map(str::to_owned)
    }

    /// Follows the selection on `side`'s text row `row`. `response` and `galley` (at
    /// `galley_pos`) are the ones given to `LabelSelectionState::label_text_selection`.
    pub fn track(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        galley_pos: egui::Pos2,
        galley: &egui::Galley,
        side: ActiveSide,
        row: usize,
    ) {
        let Some(pointer) = ui.ctx().pointer_interact_pos() else {
            return;
        };
        let at_pointer = galley.cursor_from_pos(pointer - galley_pos);
        let (pressed, down, shift) = ui.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.modifiers.shift,
            )
        });
        let in_row = |s: &RowSelection| s.side == side && s.row == row;

        let clicked = response.double_clicked() || response.triple_clicked();
        if response.contains_pointer() && (pressed || clicked) {
            if pressed {
                self.unclaimed_press = false;
            }
            let mut cursor = match &mut self.selection {
                Some(selection) if in_row(selection) => selection.cursor.clone(),
                // Shift extends egui's selection from another row to this one.
                Some(selection) if shift => {
                    selection.spans_rows = true;
                    return;
                }
                _ => TextCursorState::default(),
            };
            // What egui's label selection does with the same press or click.
            cursor.pointer_interaction(ui, response, at_pointer, galley, false);
            self.selection = Some(RowSelection {
                side,
                row,
                cursor,
                range: 0..0,
                text: String::new(),
                spans_rows: false,
            });
        } else if down && let Some(selection) = self.selection.as_mut().filter(|s| in_row(s)) {
            // egui moves the selection's end to the pointer while it is over the row, and into
            // the rows above or below once it leaves them vertically.
            if !response.rect.y_range().contains(pointer.y) {
                selection.spans_rows = true;
                return;
            }
            selection.spans_rows = false;
            if response.contains_pointer()
                && let Some(mut range) = selection.cursor.range(galley)
            {
                range.primary = at_pointer;
                selection.cursor.set_char_range(Some(range));
            }
        } else {
            return;
        }

        let selection = self.selection.as_mut().expect("set above");
        let chars = selection
            .cursor
            .range(galley)
            .map_or(0..0, |r| r.as_sorted_char_range());
        let text = galley.text();
        let range = byte_index_from_char_index(text, chars.start)
            ..byte_index_from_char_index(text, chars.end);
        selection.text = text[range.clone()].to_owned();
        selection.range = range;
    }

    /// Byte ranges to highlight in `text`, `side`'s row `row`: every occurrence of the selected
    /// text except the selection itself.
    pub fn ranges(&mut self, side: ActiveSide, row: usize, text: &str) -> Vec<Range<usize>> {
        let Some(needle) = &self.needle else {
            return Vec::new();
        };
        let started = Instant::now();
        let selected = self
            .selection
            .as_ref()
            .filter(|s| s.side == side && s.row == row && s.text == *needle)
            .map(|s| s.range.clone());
        let ranges = find_occurrences(text, needle, true)
            .filter(|range| Some(range) != selected.as_ref())
            .take(MAX_OCCURRENCES_PER_ROW)
            .collect();
        self.searched_rows += 1;
        self.search_time += started.elapsed();
        ranges
    }

    #[cfg(test)]
    pub fn needle(&self) -> Option<&str> {
        self.needle.as_deref()
    }

    #[cfg(test)]
    pub fn searched_rows(&self) -> usize {
        self.searched_rows
    }

    #[cfg(test)]
    pub fn with_needle(needle: &str) -> Self {
        Self {
            needle: Some(needle.to_owned()),
            ..Default::default()
        }
    }
}

/// Shorter selections highlight nothing: a single character would light up most rows.
pub const MIN_OCCURRENCE_CHARS: usize = 2;

/// Matches highlighted per row at most. A row like "aaaa..." searched for "aa" has a match at
/// every character; past this many the search stops, to keep within the per-frame budget.
pub const MAX_OCCURRENCES_PER_ROW: usize = 128;

/// The text to search for when `selected` is selected, or `None` when it is too short (in
/// characters) or only whitespace.
pub fn occurrence_needle(selected: &str) -> Option<&str> {
    let long_enough = selected.chars().nth(MIN_OCCURRENCE_CHARS - 1).is_some();
    (long_enough && !selected.trim().is_empty()).then_some(selected)
}

/// Byte ranges of every occurrence of `needle` in `text`, in order. Matches may overlap: "aa"
/// occurs twice in "aaa". Lazy, so a caller can stop early.
pub fn find_occurrences<'t>(
    text: &'t str,
    needle: &'t str,
    case_sensitive: bool,
) -> impl Iterator<Item = Range<usize>> + 't {
    assert!(!needle.is_empty(), "an empty needle matches everywhere");
    let mut from = 0;
    std::iter::from_fn(move || {
        let range = if case_sensitive {
            let start = from + text[from..].find(needle)?;
            start..start + needle.len()
        } else {
            text[from..].char_indices().find_map(|(offset, _)| {
                let start = from + offset;
                let len = match_len_ignoring_case(&text[start..], needle)?;
                Some(start..start + len)
            })?
        };
        // The next match may start inside this one, but only on a char boundary.
        from = range.start
            + text[range.start..]
                .chars()
                .next()
                .expect("not empty")
                .len_utf8();
        Some(range)
    })
}

/// Measured in `text`: a char and its other case may differ in byte length (KELVIN SIGN, 'k').
fn match_len_ignoring_case(text: &str, needle: &str) -> Option<usize> {
    let mut text_chars = text.char_indices();
    for n in needle.chars() {
        let (_, t) = text_chars.next()?;
        let equal = if t.is_ascii() && n.is_ascii() {
            t.eq_ignore_ascii_case(&n)
        } else {
            t.to_lowercase().eq(n.to_lowercase())
        };
        if !equal {
            return None;
        }
    }
    Some(text_chars.next().map_or(text.len(), |(end, _)| end))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(text: &str, needle: &str) -> Vec<Range<usize>> {
        find_occurrences(text, needle, true).collect()
    }

    fn find_any_case(text: &str, needle: &str) -> Vec<Range<usize>> {
        find_occurrences(text, needle, false).collect()
    }

    #[test]
    fn exact_matches_are_found_in_order() {
        assert_eq!(
            find("let value = value_2 + value;", "value"),
            vec![4..9, 12..17, 22..27]
        );
        assert!(find("let x = 1;", "value").is_empty());
        assert!(find("val", "value").is_empty());
        assert_eq!(find("value", "value"), vec![0..5]);
    }

    #[test]
    fn matching_is_case_sensitive() {
        assert_eq!(find("Value value VALUE", "value"), vec![6..11]);
        assert_eq!(find("Value value VALUE", "Value"), vec![0..5]);
    }

    #[test]
    fn ignoring_case_matches_any_casing() {
        assert_eq!(
            find_any_case("Value value VALUE vAlUe", "VALUE"),
            vec![0..5, 6..11, 12..17, 18..23]
        );
        assert_eq!(find_any_case("École école", "ÉCOLE"), vec![0..6, 7..13]);
        assert!(find_any_case("let x = 1;", "value").is_empty());
        assert!(find_any_case("val", "VALUE").is_empty());
    }

    #[test]
    fn ignoring_case_gives_ranges_of_the_text_when_cases_differ_in_byte_length() {
        let text = "\u{212A}ey key";
        let ranges = find_any_case(text, "key");
        assert_eq!(ranges, vec![0..5, 6..9]);
        assert_eq!(&text[ranges[0].clone()], "\u{212A}ey");
    }

    #[test]
    fn ignoring_case_finds_overlapping_matches() {
        assert_eq!(find_any_case("aAaA", "AA"), vec![0..2, 1..3, 2..4]);
        assert_eq!(find_any_case("ÉéÉ", "éé"), vec![0..4, 2..6]);
    }

    #[test]
    fn overlapping_matches_are_all_found() {
        assert_eq!(find("aaaa", "aa"), vec![0..2, 1..3, 2..4]);
        assert_eq!(find("abababa", "aba"), vec![0..3, 2..5, 4..7]);
    }

    #[test]
    fn multi_byte_text_gives_byte_ranges_on_char_boundaries() {
        let text = "é€😀é€😀";
        let ranges = find(text, "€😀");
        assert_eq!(ranges, vec![2..9, 11..18]);
        for range in &ranges {
            assert_eq!(&text[range.clone()], "€😀");
        }
        // Overlapping matches step over whole characters, never into one.
        assert_eq!(find("ééé", "éé"), vec![0..4, 2..6]);
        assert_eq!(find("日本日本日", "日本日"), vec![0..9, 6..15]);
    }

    #[test]
    fn a_row_highlights_at_most_the_cap() {
        let mut state = OccurrenceState::with_needle("aa");
        let row = "a".repeat(1000);
        assert_eq!(
            state.ranges(ActiveSide::Left, 0, &row).len(),
            MAX_OCCURRENCES_PER_ROW
        );
    }

    #[test]
    fn find_paints_every_match_in_the_row_and_the_current_one_apart() {
        let find = |needle, current| {
            find_ranges(
                "ab ab ab",
                RowFind {
                    needle,
                    case_sensitive: true,
                    current,
                },
            )
        };

        assert_eq!(find("ab", Some(1)), (vec![0..2, 3..5, 6..8], Some(3..5)));
        assert_eq!(find("ab", None), (vec![0..2, 3..5, 6..8], None));
        assert_eq!(find("ab", Some(3)), (vec![0..2, 3..5, 6..8], None));
        assert_eq!(find("zz", Some(0)), (vec![], None));
        assert_eq!(find("", Some(0)), (vec![], None));
    }

    #[test]
    fn find_paints_at_most_the_cap_per_row_but_finds_the_current_one_past_it() {
        let row = "a".repeat(1000);
        let current = MAX_OCCURRENCES_PER_ROW + 10;
        let (all, found) = find_ranges(
            &row,
            RowFind {
                needle: "aa",
                case_sensitive: true,
                current: Some(current),
            },
        );
        assert_eq!(all.len(), MAX_OCCURRENCES_PER_ROW);
        assert_eq!(found, Some(current..current + 2));
    }

    #[test]
    fn find_ignoring_case_paints_every_casing() {
        let find = |case_sensitive| {
            find_ranges(
                "ab AB Ab",
                RowFind {
                    needle: "ab",
                    case_sensitive,
                    current: Some(2),
                },
            )
        };

        assert_eq!(find(false), (vec![0..2, 3..5, 6..8], Some(6..8)));
        assert_eq!(find(true), (vec![0..2], None));
    }

    #[test]
    fn short_selections_are_ignored() {
        assert_eq!(occurrence_needle(""), None);
        assert_eq!(occurrence_needle("x"), None);
        assert_eq!(occurrence_needle("xy"), Some("xy"));
        // Counted in characters: one multi-byte character is still too short.
        assert_eq!(occurrence_needle("😀"), None);
        assert_eq!(occurrence_needle("é€"), Some("é€"));
    }

    #[test]
    fn whitespace_only_selections_are_ignored() {
        assert_eq!(occurrence_needle("  "), None);
        assert_eq!(occurrence_needle("\t \t"), None);
        assert_eq!(occurrence_needle("    "), None);
        // Whitespace around text is part of the selection and is matched exactly.
        assert_eq!(occurrence_needle(" x "), Some(" x "));
        assert_eq!(occurrence_needle("x "), Some("x "));
    }
}
