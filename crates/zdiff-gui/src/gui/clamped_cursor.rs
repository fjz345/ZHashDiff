use std::usize::MAX;

#[derive(Debug)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ClampedCursor {
    #[cfg_attr(feature = "serde", serde(skip))]
    cursor: usize,
    #[cfg_attr(feature = "serde", serde(skip))]
    prev_cursor: usize,
    #[cfg_attr(feature = "serde", serde(skip))]
    max: usize,
}
impl Default for ClampedCursor {
    fn default() -> Self {
        Self {
            cursor: 0,
            prev_cursor: 0,
            max: 0,
        }
    }
}
impl ClampedCursor {
    pub fn new(cursor: usize, max: usize) -> Self {
        Self {
            cursor,
            prev_cursor: cursor,
            max,
        }
    }
    pub fn inc(&mut self) {
        self.prev_cursor = self.cursor;
        self.cursor = (self.cursor + 1).min(self.max);
        log::trace!("ClampedCursor++ @{}", self.cursor);
    }
    pub fn dec(&mut self) {
        self.prev_cursor = self.cursor;
        self.cursor = self.cursor.saturating_sub(1);
        log::trace!("ClampedCursor-- @{}", self.cursor);
    }
    pub fn set(&mut self, new_cursor: usize) {
        self.prev_cursor = self.cursor;
        self.cursor = new_cursor;
    }
    pub fn invalidate_ack(&mut self) {
        assert_ne!(
            self.cursor,
            usize::MAX,
            "Cursor should not be MAX when invalidating"
        );
        self.prev_cursor = usize::MAX;
    }
    pub fn set_max(&mut self, new_max: usize) {
        self.max = new_max;
    }
    pub fn get(&self) -> usize {
        self.cursor
    }
    pub fn get_prev(&self) -> usize {
        self.cursor
    }
    pub fn diff(&self) -> isize {
        if self.cursor >= self.prev_cursor {
            (self.cursor - self.prev_cursor) as isize
        } else {
            -((self.prev_cursor - self.cursor) as isize)
        }
    }
    pub fn has_changed(&self) -> bool {
        self.diff() != 0
    }
    pub fn ack_change(&mut self) {
        self.prev_cursor = self.cursor;
    }
}
