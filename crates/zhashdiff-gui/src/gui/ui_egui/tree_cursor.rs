/// One row of the two-folder tree as the cursor sees it, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorRow<'a> {
    /// Relative to the roots, with `/` separators.
    pub rel_path: &'a str,
    /// Not drawn: inside a collapsed folder, or the root row.
    pub hidden: bool,
    pub is_dir: bool,
    /// A folder collapsed on a side it exists on, so some of its rows are hidden.
    pub collapsed: bool,
    /// Different, Partial, or only on one side.
    pub differs: bool,
}

impl CursorRow<'_> {
    /// Only files: a differing folder just says something below it differs.
    fn is_diff_stop(&self) -> bool {
        self.differs && !self.is_dir
    }
}

/// What a cursor operation asks the GUI to do to a row, by relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CursorRequest {
    Expand(String),
    Collapse(String),
    /// Open the file in the external diff tool.
    Open(String),
}

impl CursorRequest {
    pub fn rel_path(&self) -> &str {
        match self {
            Self::Expand(rel_path) | Self::Collapse(rel_path) | Self::Open(rel_path) => rel_path,
        }
    }
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

    /// Collapses an expanded folder, otherwise moves to the parent. A top-level entry stays
    /// put: the root row is never drawn.
    pub fn left(&self, rows: &[CursorRow]) -> (Self, Option<CursorRequest>) {
        let Some(row) = self.drawn_row(rows) else {
            return (self.clone(), None);
        };
        if row.is_dir && !row.collapsed {
            return (
                self.clone(),
                Some(CursorRequest::Collapse(row.rel_path.into())),
            );
        }
        let parent = row.rel_path.rsplit_once('/').map(|(parent, _)| parent);
        let parent_row = parent.and_then(|parent| {
            rows.iter()
                .find(|row| row.rel_path == parent && !row.hidden)
        });
        (self.moved_to(parent_row), None)
    }

    /// Expands a collapsed folder.
    pub fn right(&self, rows: &[CursorRow]) -> Option<CursorRequest> {
        let row = self.drawn_row(rows)?;
        (row.is_dir && row.collapsed).then(|| CursorRequest::Expand(row.rel_path.into()))
    }

    /// Opens a file, toggles a folder.
    pub fn enter(&self, rows: &[CursorRow]) -> Option<CursorRequest> {
        let row = self.drawn_row(rows)?;
        let rel_path = row.rel_path.into();
        Some(match (row.is_dir, row.collapsed) {
            (false, _) => CursorRequest::Open(rel_path),
            (true, true) => CursorRequest::Expand(rel_path),
            (true, false) => CursorRequest::Collapse(rel_path),
        })
    }

    /// The next differing file in display order, hidden rows included, with the requests that
    /// expand its collapsed ancestors. Without a cursor, the first one. `None` is the "no more
    /// diffs" notice: there is no wraparound, so the cursor stays put.
    pub fn next_diff(&self, rows: &[CursorRow]) -> Option<(Self, Vec<CursorRequest>)> {
        let start = self.index_in(rows).map_or(0, |index| index + 1);
        let target = rows[start..].iter().find(|row| row.is_diff_stop())?;
        Some(Self::jump_to(rows, target))
    }

    /// The previous differing file, as `next_diff`. Without a cursor, the last one.
    pub fn prev_diff(&self, rows: &[CursorRow]) -> Option<(Self, Vec<CursorRequest>)> {
        let end = self.index_in(rows).unwrap_or(rows.len());
        let target = rows[..end].iter().rev().find(|row| row.is_diff_stop())?;
        Some(Self::jump_to(rows, target))
    }

    /// The target with expand requests for its collapsed ancestors, outermost first.
    fn jump_to(rows: &[CursorRow], target: &CursorRow) -> (Self, Vec<CursorRequest>) {
        let expand = target
            .rel_path
            .match_indices('/')
            .map(|(end, _)| &target.rel_path[..end])
            .filter(|ancestor| {
                rows.iter()
                    .any(|row| row.rel_path == *ancestor && row.collapsed)
            })
            .map(|ancestor| CursorRequest::Expand(ancestor.into()))
            .collect();
        (Self::at(target.rel_path), expand)
    }

    /// The cursor's row if it is drawn. Acting on a row the user can't see would be a
    /// surprise.
    fn drawn_row<'a>(&self, rows: &'a [CursorRow<'a>]) -> Option<&'a CursorRow<'a>> {
        self.index_in(rows)
            .map(|index| &rows[index])
            .filter(|row| !row.hidden)
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
    use super::{CursorRequest, CursorRow, TreeCursor};

    /// A `-` prefix marks a row hidden by collapse, then a `*` prefix a differing entry. A `/`
    /// suffix marks an expanded folder and a `>` suffix a collapsed one.
    fn rows<'a>(spec: &[&'a str]) -> Vec<CursorRow<'a>> {
        spec.iter()
            .map(|s| {
                let (hidden, s) = s.strip_prefix('-').map_or((false, *s), |s| (true, s));
                let (differs, s) = s.strip_prefix('*').map_or((false, s), |s| (true, s));
                let (is_dir, collapsed, rel_path) = if let Some(s) = s.strip_suffix('/') {
                    (true, false, s)
                } else if let Some(s) = s.strip_suffix('>') {
                    (true, true, s)
                } else {
                    (false, false, s)
                };
                CursorRow {
                    rel_path,
                    hidden,
                    is_dir,
                    collapsed,
                    differs,
                }
            })
            .collect()
    }

    /// `b` is a differing folder, `e` and `e/f` are collapsed differing folders, and `e/f/g`
    /// is a differing file inside both.
    const DIFFS: &[&str] = &[
        "-/", "a", "*b/", "*b/c", "b/d", "*e>", "-*e/f>", "-*e/f/g", "-e/h", "*i", "j",
    ];

    fn jump_to(rel_path: &str, expand: &[&str]) -> Option<(TreeCursor, Vec<CursorRequest>)> {
        let expand = expand
            .iter()
            .map(|rel_path| CursorRequest::Expand(rel_path.to_string()))
            .collect();
        Some((TreeCursor::at(rel_path), expand))
    }

    fn jumped_to(jump: Option<(TreeCursor, Vec<CursorRequest>)>) -> Option<String> {
        jump.and_then(|(cursor, _)| cursor.rel_path().map(str::to_string))
    }

    #[test]
    fn next_and_prev_diff_skip_same_files_and_all_folders() {
        let rows = rows(DIFFS);

        let next = |at| jumped_to(TreeCursor::at(at).next_diff(&rows));
        assert_eq!(
            next("a").as_deref(),
            Some("b/c"),
            "differing folder b skipped"
        );
        assert_eq!(next("b/c").as_deref(), Some("e/f/g"));
        assert_eq!(next("e/f/g").as_deref(), Some("i"));

        let prev = |at| jumped_to(TreeCursor::at(at).prev_diff(&rows));
        assert_eq!(prev("j").as_deref(), Some("i"));
        assert_eq!(prev("i").as_deref(), Some("e/f/g"));
        assert_eq!(prev("e/f/g").as_deref(), Some("b/c"));
    }

    #[test]
    fn a_diff_jump_expands_only_the_targets_collapsed_ancestors_outermost_first() {
        let rows = rows(DIFFS);

        assert_eq!(
            TreeCursor::at("b/c").next_diff(&rows),
            jump_to("e/f/g", &["e", "e/f"])
        );
        assert_eq!(
            TreeCursor::at("i").prev_diff(&rows),
            jump_to("e/f/g", &["e", "e/f"])
        );
        assert_eq!(
            TreeCursor::at("a").next_diff(&rows),
            jump_to("b/c", &[]),
            "b is expanded"
        );
    }

    #[test]
    fn there_is_no_wraparound_past_the_first_or_last_diff() {
        let rows = rows(DIFFS);

        assert_eq!(TreeCursor::at("i").next_diff(&rows), None);
        assert_eq!(TreeCursor::at("j").next_diff(&rows), None);
        assert_eq!(TreeCursor::at("b/c").prev_diff(&rows), None);
        assert_eq!(TreeCursor::at("a").prev_diff(&rows), None);
    }

    #[test]
    fn without_a_cursor_next_diff_goes_to_the_first_diff_and_prev_diff_to_the_last() {
        let rows = rows(DIFFS);

        // A cursor whose row is gone has no position, as for up and down.
        for cursor in [TreeCursor::default(), TreeCursor::at("gone")] {
            assert_eq!(cursor.next_diff(&rows), jump_to("b/c", &[]), "{cursor:?}");
            assert_eq!(cursor.prev_diff(&rows), jump_to("i", &[]), "{cursor:?}");
        }
    }

    #[test]
    fn a_diff_jump_from_a_hidden_row_starts_at_that_row() {
        let rows = rows(DIFFS);

        assert_eq!(TreeCursor::at("e/h").next_diff(&rows), jump_to("i", &[]));
        assert_eq!(
            TreeCursor::at("e/h").prev_diff(&rows),
            jump_to("e/f/g", &["e", "e/f"])
        );
    }

    #[test]
    fn with_no_differing_files_there_is_no_diff_to_jump_to() {
        let rows = rows(TREE);

        assert_eq!(TreeCursor::default().next_diff(&rows), None);
        assert_eq!(TreeCursor::default().prev_diff(&rows), None);
    }

    const TREE: &[&str] = &["-/", "a", "b>", "-b/x", "-b/y", "c", "-d"];

    /// `e` is expanded, `e/f` collapsed, `k` collapsed at the top level, `m` a top-level file.
    const FOLDERS: &[&str] = &["-/", "e/", "e/f>", "-e/f/g", "e/h", "k>", "-k/l", "m"];

    fn expand(rel_path: &str) -> Option<CursorRequest> {
        Some(CursorRequest::Expand(rel_path.to_string()))
    }

    fn collapse(rel_path: &str) -> Option<CursorRequest> {
        Some(CursorRequest::Collapse(rel_path.to_string()))
    }

    fn open(rel_path: &str) -> Option<CursorRequest> {
        Some(CursorRequest::Open(rel_path.to_string()))
    }

    #[test]
    fn right_on_a_collapsed_folder_requests_expand() {
        let rows = rows(FOLDERS);

        assert_eq!(TreeCursor::at("e/f").right(&rows), expand("e/f"));
        assert_eq!(TreeCursor::at("e").right(&rows), None, "already expanded");
        assert_eq!(TreeCursor::at("e/h").right(&rows), None, "file");
    }

    #[test]
    fn left_on_an_expanded_folder_requests_collapse_and_keeps_the_cursor() {
        let rows = rows(FOLDERS);

        assert_eq!(
            TreeCursor::at("e").left(&rows),
            (TreeCursor::at("e"), collapse("e"))
        );
    }

    #[test]
    fn left_on_a_file_or_collapsed_folder_moves_to_the_parent() {
        let rows = rows(FOLDERS);

        assert_eq!(
            TreeCursor::at("e/h").left(&rows),
            (TreeCursor::at("e"), None)
        );
        assert_eq!(
            TreeCursor::at("e/f").left(&rows),
            (TreeCursor::at("e"), None)
        );
    }

    #[test]
    fn left_on_a_top_level_entry_stays_put_since_the_root_row_is_never_drawn() {
        let rows = rows(FOLDERS);

        assert_eq!(TreeCursor::at("k").left(&rows), (TreeCursor::at("k"), None));
        assert_eq!(TreeCursor::at("m").left(&rows), (TreeCursor::at("m"), None));
    }

    #[test]
    fn enter_opens_a_file_and_toggles_a_folder() {
        let rows = rows(FOLDERS);

        assert_eq!(TreeCursor::at("e/h").enter(&rows), open("e/h"));
        assert_eq!(TreeCursor::at("e/f").enter(&rows), expand("e/f"));
        assert_eq!(TreeCursor::at("e").enter(&rows), collapse("e"));
    }

    #[test]
    fn left_right_and_enter_do_nothing_without_a_drawn_cursor_row() {
        let rows = rows(FOLDERS);

        // Issue 04 moves a cursor on a hidden row to its nearest visible ancestor.
        for cursor in [
            TreeCursor::default(),
            TreeCursor::at("gone"),
            TreeCursor::at("e/f/g"),
        ] {
            assert_eq!(cursor.left(&rows), (cursor.clone(), None), "{cursor:?}");
            assert_eq!(cursor.right(&rows), None, "{cursor:?}");
            assert_eq!(cursor.enter(&rows), None, "{cursor:?}");
        }
    }

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
