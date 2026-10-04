use std::{cmp::Ordering, collections::HashMap};

use serde::{Deserialize, Serialize};

use crate::ui_egui::fs_tree::{DiffState, VisibleRowTwoFolderDiff};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SortKey {
    #[default]
    Name,
    State,
}

/// Row order of the two-folder diff: a tree with sorted siblings, folders before files in either
/// direction, or flat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TreeSort {
    pub key: SortKey,
    pub descending: bool,
    #[serde(default)]
    pub flat: bool,
}

impl TreeSort {
    /// A click on `key`'s header: the active key reverses, another key starts ascending.
    pub fn clicked(self, key: SortKey) -> Self {
        Self {
            key,
            descending: key == self.key && !self.descending,
            ..self
        }
    }

    /// Ties are `Equal`, so a stable sort keeps them in their previous order.
    pub fn compare_siblings(
        self,
        a: &VisibleRowTwoFolderDiff,
        b: &VisibleRowTwoFolderDiff,
    ) -> Ordering {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| self.compare_by_key(a, b))
    }

    /// Only the key is reversed, so ties stay `Equal`.
    fn compare_by_key(self, a: &VisibleRowTwoFolderDiff, b: &VisibleRowTwoFolderDiff) -> Ordering {
        let by_key = match self.key {
            // Case-insensitive, like the file systems on the primary platform.
            SortKey::Name => name(a)
                .chars()
                .flat_map(char::to_lowercase)
                .cmp(name(b).chars().flat_map(char::to_lowercase)),
            SortKey::State => state_rank(&a.diff_state).cmp(&state_rank(&b.diff_state)),
        };
        if self.descending {
            by_key.reverse()
        } else {
            by_key
        }
    }
}

fn name(row: &VisibleRowTwoFolderDiff) -> &str {
    row.rel_path.rsplit('/').next().unwrap_or_default()
}

/// Ascending order: the rows that need a look first.
fn state_rank(state: &DiffState) -> u8 {
    match state {
        DiffState::Different(..) => 0,
        DiffState::Partial(..) => 1,
        DiffState::OnlyInFirst(..) => 2,
        DiffState::OnlyInSecond(..) => 3,
        DiffState::Same(..) => 4,
    }
}

/// The rows as a depth-first walk with every folder's children in `sort` order, so children stay
/// under their parent. Flat: one list by depth, then the key, with no folders first. Each row
/// holds both sides, so the sides stay aligned.
pub fn sort_two_folder_rows(
    mut rows: Vec<VisibleRowTwoFolderDiff>,
    sort: TreeSort,
) -> Vec<VisibleRowTwoFolderDiff> {
    if sort.flat {
        rows.sort_by(|a, b| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| sort.compare_by_key(a, b))
        });
        return rows;
    }

    // Grouped by path, not by is_dir: a path that is a file on one side can be a folder with
    // children on the other.
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    let mut top = Vec::new();
    {
        let index: HashMap<&str, usize> = rows
            .iter()
            .enumerate()
            .map(|(i, row)| (row.rel_path.as_str(), i))
            .collect();
        for (i, row) in rows.iter().enumerate() {
            if row.rel_path.is_empty() {
                top.push(i);
                continue;
            }
            let parent = row
                .rel_path
                .rsplit_once('/')
                .map_or("", |(parent, _)| parent);
            let parent = *index
                .get(parent)
                .unwrap_or_else(|| panic!("the parent of row {:?} is not a row", row.rel_path));
            children[parent].push(i);
        }
    }
    for siblings in &mut children {
        siblings.sort_by(|&a, &b| sort.compare_siblings(&rows[a], &rows[b]));
    }

    let mut order = Vec::with_capacity(rows.len());
    let mut stack = top;
    while let Some(i) = stack.pop() {
        order.push(i);
        stack.extend(children[i].iter().rev());
    }
    assert_eq!(
        order.len(),
        rows.len(),
        "every row is reached from the root"
    );

    let mut rows: Vec<Option<VisibleRowTwoFolderDiff>> = rows.into_iter().map(Some).collect();
    order
        .into_iter()
        .map(|i| rows[i].take().expect("each row is visited once"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{SortKey, TreeSort, sort_two_folder_rows};
    use crate::ui_egui::fs_tree::{DiffState, VisibleRowTwoFolderDiff};

    fn row(rel_path: &str, is_dir: bool, diff_state: DiffState) -> VisibleRowTwoFolderDiff {
        VisibleRowTwoFolderDiff {
            rel_path: rel_path.to_owned(),
            is_dir,
            // As built: the root is 0.
            depth: rel_path.split('/').filter(|c| !c.is_empty()).count() as _,
            diff_state,
        }
    }

    fn file(rel_path: &str) -> VisibleRowTwoFolderDiff {
        row(rel_path, false, DiffState::Same(0, 0))
    }

    fn folder(rel_path: &str) -> VisibleRowTwoFolderDiff {
        row(rel_path, true, DiffState::Same(0, 0))
    }

    /// Sorts siblings given in this order and returns their paths.
    fn sorted(mut siblings: Vec<VisibleRowTwoFolderDiff>, sort: TreeSort) -> Vec<String> {
        siblings.sort_by(|a, b| sort.compare_siblings(a, b));
        siblings.into_iter().map(|r| r.rel_path).collect()
    }

    const NAME_ASC: TreeSort = TreeSort {
        key: SortKey::Name,
        descending: false,
        flat: false,
    };
    const NAME_DESC: TreeSort = TreeSort {
        key: SortKey::Name,
        descending: true,
        flat: false,
    };
    const STATE_ASC: TreeSort = TreeSort {
        key: SortKey::State,
        descending: false,
        flat: false,
    };
    const STATE_DESC: TreeSort = TreeSort {
        key: SortKey::State,
        descending: true,
        flat: false,
    };

    #[test]
    fn name_sorts_ascending_and_descending_by_the_last_component_ignoring_case() {
        let siblings = || vec![file("x/b.txt"), file("x/C.txt"), file("x/a.txt")];
        assert_eq!(
            sorted(siblings(), NAME_ASC),
            ["x/a.txt", "x/b.txt", "x/C.txt"]
        );
        assert_eq!(
            sorted(siblings(), NAME_DESC),
            ["x/C.txt", "x/b.txt", "x/a.txt"]
        );
    }

    #[test]
    fn state_sorts_different_partial_only_in_first_only_in_second_same() {
        let siblings = || {
            vec![
                row("same", false, DiffState::Same(0, 0)),
                row("second", false, DiffState::OnlyInSecond(0)),
                row("first", false, DiffState::OnlyInFirst(0)),
                row("partial", false, DiffState::Partial(0, 0)),
                row("different", false, DiffState::Different(0, 0)),
            ]
        };
        assert_eq!(
            sorted(siblings(), STATE_ASC),
            ["different", "partial", "first", "second", "same"]
        );
        assert_eq!(
            sorted(siblings(), STATE_DESC),
            ["same", "second", "first", "partial", "different"]
        );
    }

    #[test]
    fn folders_come_before_files_in_either_direction() {
        let siblings = || {
            vec![
                file("a.txt"),
                row("z", true, DiffState::Same(0, 0)),
                row("b", true, DiffState::Different(0, 0)),
                row("c.txt", false, DiffState::Different(0, 0)),
            ]
        };
        assert_eq!(sorted(siblings(), NAME_ASC), ["b", "z", "a.txt", "c.txt"]);
        assert_eq!(sorted(siblings(), NAME_DESC), ["z", "b", "c.txt", "a.txt"]);
        assert_eq!(sorted(siblings(), STATE_ASC), ["b", "z", "c.txt", "a.txt"]);
        assert_eq!(sorted(siblings(), STATE_DESC), ["z", "b", "a.txt", "c.txt"]);
    }

    #[test]
    fn ties_keep_their_previous_order_in_either_direction() {
        let same_state = || vec![file("b"), file("a"), file("c")];
        assert_eq!(sorted(same_state(), STATE_ASC), ["b", "a", "c"]);
        assert_eq!(sorted(same_state(), STATE_DESC), ["b", "a", "c"]);

        let same_name_but_case = || vec![file("A"), file("a")];
        assert_eq!(sorted(same_name_but_case(), NAME_ASC), ["A", "a"]);
        assert_eq!(sorted(same_name_but_case(), NAME_DESC), ["A", "a"]);
    }

    #[test]
    fn a_header_click_reverses_the_active_key_and_starts_another_key_ascending() {
        assert_eq!(NAME_ASC.clicked(SortKey::Name), NAME_DESC);
        assert_eq!(NAME_DESC.clicked(SortKey::Name), NAME_ASC);
        assert_eq!(NAME_DESC.clicked(SortKey::State), STATE_ASC);
        assert_eq!(STATE_DESC.clicked(SortKey::Name), NAME_ASC);
        assert_eq!(TreeSort::default(), NAME_ASC);
    }

    #[test]
    fn sorting_walks_each_folder_after_its_parent_with_sorted_siblings() {
        // Byte order, as the rows are built: '.' sorts before '/', so a.txt lands between a and
        // a's children.
        let rows = || {
            vec![
                folder(""),
                folder("a"),
                file("a.txt"),
                file("a/b.txt"),
                folder("a/z"),
                file("a/z/deep.txt"),
                folder("c"),
                file("c/d.txt"),
            ]
        };
        let paths = |sort| -> Vec<String> {
            sort_two_folder_rows(rows(), sort)
                .into_iter()
                .map(|r| r.rel_path)
                .collect()
        };
        assert_eq!(
            paths(NAME_ASC),
            [
                "",
                "a",
                "a/z",
                "a/z/deep.txt",
                "a/b.txt",
                "c",
                "c/d.txt",
                "a.txt"
            ]
        );
        assert_eq!(
            paths(NAME_DESC),
            [
                "",
                "c",
                "c/d.txt",
                "a",
                "a/z",
                "a/z/deep.txt",
                "a/b.txt",
                "a.txt"
            ]
        );
    }

    const FLAT: TreeSort = TreeSort {
        key: SortKey::Name,
        descending: false,
        flat: true,
    };

    fn sorted_paths(rows: Vec<VisibleRowTwoFolderDiff>, sort: TreeSort) -> Vec<String> {
        sort_two_folder_rows(rows, sort)
            .into_iter()
            .map(|r| r.rel_path)
            .collect()
    }

    #[test]
    fn flat_mode_orders_by_depth_then_name_ignoring_case_and_hierarchy() {
        let rows = vec![
            folder(""),
            folder("a"),
            file("a.txt"),
            file("a/B.txt"),
            folder("a/z"),
            file("a/z/deep.txt"),
            folder("c"),
            file("c/a.txt"),
        ];
        // Folders don't come first: a.txt sits between a and c.
        assert_eq!(
            sorted_paths(rows, FLAT),
            [
                "",
                "a",
                "a.txt",
                "c",
                "c/a.txt",
                "a/B.txt",
                "a/z",
                "a/z/deep.txt"
            ]
        );
    }

    #[test]
    fn flat_mode_orders_each_depth_by_the_active_header_and_keeps_ties_in_order() {
        let rows = || {
            vec![
                folder(""),
                row("a", true, DiffState::Same(0, 0)),
                row("a/x.txt", false, DiffState::Different(0, 0)),
                row("b", true, DiffState::Different(0, 0)),
                row("b/x.txt", false, DiffState::Same(0, 0)),
                row("c.txt", false, DiffState::Same(0, 0)),
            ]
        };
        let name_desc = FLAT.clicked(SortKey::Name);
        assert_eq!(
            sorted_paths(rows(), name_desc),
            ["", "c.txt", "b", "a", "a/x.txt", "b/x.txt"]
        );
        let state_asc = FLAT.clicked(SortKey::State);
        assert_eq!(
            sorted_paths(rows(), state_asc),
            ["", "b", "a", "c.txt", "a/x.txt", "b/x.txt"]
        );
    }

    #[test]
    fn a_header_click_keeps_flat_mode() {
        assert!(FLAT.clicked(SortKey::Name).flat);
        assert!(FLAT.clicked(SortKey::State).flat);
        assert!(!TreeSort::default().flat);
    }

    #[test]
    fn flat_mode_survives_a_save_and_a_sort_saved_before_it_existed_loads_as_a_tree() {
        let saved = serde_json::to_string(&FLAT).unwrap();
        assert_eq!(serde_json::from_str::<TreeSort>(&saved).unwrap(), FLAT);
        let old: TreeSort = serde_json::from_str(r#"{"key":"State","descending":true}"#).unwrap();
        assert_eq!(old, STATE_DESC);
    }

    #[test]
    fn a_path_that_is_a_file_on_one_side_keeps_the_other_sides_children() {
        // x is a file on the left (so the row's is_dir is false) and a folder on the right.
        let rows = vec![
            folder(""),
            row("x", false, DiffState::Different(1, 1)),
            row("x/y.txt", false, DiffState::OnlyInSecond(2)),
            file("a.txt"),
        ];
        let paths: Vec<String> = sort_two_folder_rows(rows, NAME_ASC)
            .into_iter()
            .map(|r| r.rel_path)
            .collect();
        assert_eq!(paths, ["", "a.txt", "x", "x/y.txt"]);
    }
}
