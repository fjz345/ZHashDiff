//! Which side of the file diff pane is active. Transient UI state: never persisted.

use eframe::egui;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ActiveSide {
    #[default]
    Left,
    Right,
}

/// With only one file loaded that side is always active, otherwise the stored side stands.
pub fn effective_side(stored: ActiveSide, left_loaded: bool, right_loaded: bool) -> ActiveSide {
    match (left_loaded, right_loaded) {
        (true, false) => ActiveSide::Left,
        (false, true) => ActiveSide::Right,
        _ => stored,
    }
}

/// `left` and `right` are the previous frame's side rectangles; `NOTHING` contains no point.
pub fn side_at_press(pos: egui::Pos2, left: egui::Rect, right: egui::Rect) -> Option<ActiveSide> {
    if left.contains(pos) {
        Some(ActiveSide::Left)
    } else if right.contains(pos) {
        Some(ActiveSide::Right)
    } else {
        None
    }
}

/// Per-pane transient state. The press is resolved against the previous frame's rectangles, so
/// the side is known before this frame's rows are laid out.
pub struct ActiveSideState {
    side: ActiveSide,
    left_rect: egui::Rect,
    right_rect: egui::Rect,
}

impl Default for ActiveSideState {
    fn default() -> Self {
        Self {
            side: ActiveSide::default(),
            left_rect: egui::Rect::NOTHING,
            right_rect: egui::Rect::NOTHING,
        }
    }
}

impl ActiveSideState {
    pub fn begin_frame(
        &mut self,
        press_pos: Option<egui::Pos2>,
        left_loaded: bool,
        right_loaded: bool,
    ) -> ActiveSide {
        if let Some(pressed) = press_pos.and_then(|p| side_at_press(p, self.left_rect, self.right_rect))
        {
            self.side = pressed;
        }
        // Written back so a forced side stays active once the other file loads.
        self.side = effective_side(self.side, left_loaded, right_loaded);
        self.side
    }

    pub fn end_frame(&mut self, left_rect: egui::Rect, right_rect: egui::Rect) {
        self.left_rect = left_rect;
        self.right_rect = right_rect;
    }
}

/// Style of the outline around the active side's text block.
pub fn outline_stroke() -> egui::Stroke {
    egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 90, 130))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_sided_keeps_stored_side() {
        assert_eq!(
            effective_side(ActiveSide::Left, true, true),
            ActiveSide::Left
        );
        assert_eq!(
            effective_side(ActiveSide::Right, true, true),
            ActiveSide::Right
        );
    }

    #[test]
    fn left_only_forces_left() {
        assert_eq!(
            effective_side(ActiveSide::Right, true, false),
            ActiveSide::Left
        );
    }

    #[test]
    fn right_only_forces_right() {
        assert_eq!(
            effective_side(ActiveSide::Left, false, true),
            ActiveSide::Right
        );
    }

    #[test]
    fn nothing_loaded_keeps_stored_side() {
        assert_eq!(
            effective_side(ActiveSide::Right, false, false),
            ActiveSide::Right
        );
    }

    #[test]
    fn press_resolves_to_the_side_rect_it_lands_in() {
        let left = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 50.0));
        let right = egui::Rect::from_min_max(egui::pos2(112.0, 0.0), egui::pos2(200.0, 50.0));

        assert_eq!(
            side_at_press(egui::pos2(10.0, 10.0), left, right),
            Some(ActiveSide::Left)
        );
        assert_eq!(
            side_at_press(egui::pos2(150.0, 10.0), left, right),
            Some(ActiveSide::Right)
        );
        // Gap between the sides and anything outside both rects changes nothing.
        assert_eq!(side_at_press(egui::pos2(105.0, 10.0), left, right), None);
        assert_eq!(side_at_press(egui::pos2(10.0, 80.0), left, right), None);
    }

    fn state_with_rects() -> ActiveSideState {
        let mut state = ActiveSideState::default();
        state.end_frame(
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(100.0, 50.0)),
            egui::Rect::from_min_max(egui::pos2(112.0, 0.0), egui::pos2(200.0, 50.0)),
        );
        state
    }

    #[test]
    fn starts_with_left_active() {
        let mut state = ActiveSideState::default();
        assert_eq!(state.begin_frame(None, true, true), ActiveSide::Left);
    }

    #[test]
    fn press_on_other_side_switches_in_the_same_frame_and_sticks() {
        let mut state = state_with_rects();

        let press = Some(egui::pos2(150.0, 10.0));
        assert_eq!(state.begin_frame(press, true, true), ActiveSide::Right);
        assert_eq!(state.begin_frame(None, true, true), ActiveSide::Right);

        let press = Some(egui::pos2(10.0, 10.0));
        assert_eq!(state.begin_frame(press, true, true), ActiveSide::Left);
    }

    #[test]
    fn press_cannot_deactivate_the_only_loaded_side() {
        let mut state = state_with_rects();

        let press_left = Some(egui::pos2(10.0, 10.0));
        assert_eq!(state.begin_frame(press_left, false, true), ActiveSide::Right);
        assert_eq!(state.begin_frame(None, false, true), ActiveSide::Right);
    }

    #[test]
    fn forced_side_stays_active_when_the_other_file_loads() {
        let mut state = ActiveSideState::default();
        assert_eq!(state.begin_frame(None, false, true), ActiveSide::Right);
        assert_eq!(state.begin_frame(None, true, true), ActiveSide::Right);
    }

    #[test]
    fn press_before_any_rect_exists_changes_nothing() {
        assert_eq!(
            side_at_press(
                egui::pos2(10.0, 10.0),
                egui::Rect::NOTHING,
                egui::Rect::NOTHING
            ),
            None
        );
    }
}
