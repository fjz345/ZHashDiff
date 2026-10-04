/// One row of the two-folder tree as the cursor sees it, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorRow<'a> {
    /// Relative to the roots, with `/` separators.
    pub rel_path: &'a str,
    /// Not drawn: inside a collapsed folder, or the root row.
    pub hidden: bool,
}

/// The current row of the two-folder tree. Held as the entry's relative path rather than
/// a row index or node id, so it follows the entry through row rebuilds and rescans.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TreeCursor {
    rel_path: Option<String>,
}

impl TreeCursor {
    pub fn at(rel_path: &str) -> Self {
        Self {
            rel_path: Some(rel_path.to_string()),
        }
    }

    pub fn rel_path(&self) -> Option<&str> {
        self.rel_path.as_deref()
    }

    pub fn is_at(&self, rel_path: &str) -> bool {
        self.rel_path() == Some(rel_path)
    }

    /// The next visible row, or the first one without a cursor. Stays put at the last.
    pub fn down(&self, rows: &[CursorRow]) -> Self {
        let start = self.index_in(rows).map_or(0, |index| index + 1);
        self.moved_to(rows[start..].iter().find(|row| !row.hidden))
    }

    /// The previous visible row, or the last one without a cursor. Stays put at the first.
    pub fn up(&self, rows: &[CursorRow]) -> Self {
        let end = self.index_in(rows).unwrap_or(rows.len());
        self.moved_to(rows[..end].iter().rev().find(|row| !row.hidden))
    }

    /// A cursor whose row is gone has no position, so it moves like no cursor.
    fn index_in(&self, rows: &[CursorRow]) -> Option<usize> {
        let rel_path = self.rel_path()?;
        rows.iter().position(|row| row.rel_path == rel_path)
    }

    fn moved_to(&self, row: Option<&CursorRow>) -> Self {
        row.map_or_else(|| self.clone(), |row| Self::at(row.rel_path))
    }
}

#[cfg(test)]
mod tests {
    use super::{CursorRow, TreeCursor};

    /// `-` marks a row hidden by collapse.
    fn rows<'a>(spec: &[&'a str]) -> Vec<CursorRow<'a>> {
        spec.iter()
            .map(|s| match s.strip_prefix('-') {
                Some(rel_path) => CursorRow {
                    rel_path,
                    hidden: true,
                },
                None => CursorRow {
                    rel_path: s,
                    hidden: false,
                },
            })
            .collect()
    }

    const TREE: &[&str] = &["-", "a", "b", "-b/x", "-b/y", "c", "-d"];

    #[test]
    fn down_and_up_skip_collapse_hidden_rows() {
        let rows = rows(TREE);

        let cursor = TreeCursor::at("b").down(&rows);
        assert_eq!(cursor.rel_path(), Some("c"));

        let cursor = cursor.up(&rows);
        assert_eq!(cursor.rel_path(), Some("b"));
    }

    #[test]
    fn cursor_stops_at_the_first_and_last_visible_row() {
        let rows = rows(TREE);

        assert_eq!(TreeCursor::at("c").down(&rows).rel_path(), Some("c"));
        assert_eq!(TreeCursor::at("a").up(&rows).rel_path(), Some("a"));
    }

    #[test]
    fn without_a_cursor_down_goes_to_the_first_visible_row_and_up_to_the_last() {
        let rows = rows(TREE);

        assert_eq!(TreeCursor::default().down(&rows).rel_path(), Some("a"));
        assert_eq!(TreeCursor::default().up(&rows).rel_path(), Some("c"));
    }

    #[test]
    fn a_cursor_on_a_hidden_row_moves_to_the_visible_neighbours() {
        let rows = rows(TREE);

        assert_eq!(TreeCursor::at("b/x").down(&rows).rel_path(), Some("c"));
        assert_eq!(TreeCursor::at("b/y").up(&rows).rel_path(), Some("b"));
    }

    #[test]
    fn a_cursor_whose_row_is_gone_moves_like_no_cursor() {
        let rows = rows(TREE);

        assert_eq!(TreeCursor::at("gone").down(&rows).rel_path(), Some("a"));
        assert_eq!(TreeCursor::at("gone").up(&rows).rel_path(), Some("c"));
    }

    #[test]
    fn with_no_visible_rows_the_cursor_stays_put() {
        let rows = rows(&["-", "-a"]);

        assert_eq!(TreeCursor::default().down(&rows), TreeCursor::default());
        assert_eq!(TreeCursor::at("a").up(&rows), TreeCursor::at("a"));
    }
}
