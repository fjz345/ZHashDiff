use std::{
    collections::{BTreeMap, HashMap},
    fmt::Display,
    path::{Path, PathBuf},
};

/// Groups files by content hash. Only groups of two or more files are returned,
/// ordered by hash with each group's paths sorted, so the result does not depend
/// on the input order.
pub fn group_duplicates(
    files: impl IntoIterator<Item = (PathBuf, String)>,
) -> Vec<(String, Vec<PathBuf>)> {
    let mut groups: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    for (path, hash) in files {
        groups.entry(hash).or_default().push(path);
    }

    groups
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .map(|(hash, mut paths)| {
            paths.sort();
            (hash, paths)
        })
        .collect()
}

/// What a resolve removes.
#[derive(Debug, Default, PartialEq)]
pub struct ResolutionPlan {
    /// Every non-keeper of each group whose keeper is one of its files, ordered by
    /// group hash, then by the group's order.
    pub deletions: Vec<PathBuf>,
    /// Groups whose keeper is not one of their files. Nothing of them is planned.
    pub rejected: Vec<String>,
}

/// Groups without a keeper are left out.
pub fn plan_resolution(
    conflict_map: &HashMap<String, Vec<PathBuf>>,
    keepers: &HashMap<String, PathBuf>,
) -> ResolutionPlan {
    let mut hashes: Vec<&String> = conflict_map.keys().collect();
    hashes.sort();

    let mut plan = ResolutionPlan::default();
    for hash in hashes {
        let Some(keeper) = keepers.get(hash) else {
            continue;
        };
        let paths = &conflict_map[hash];
        // A keeper outside the group would otherwise remove every copy.
        if !paths.contains(keeper) {
            plan.rejected.push(hash.clone());
            continue;
        }
        plan.deletions
            .extend(paths.iter().filter(|path| *path != keeper).cloned());
    }
    plan
}

#[derive(Debug, Default, PartialEq)]
pub struct ResolutionSummary {
    pub removed: Vec<PathBuf>,
    /// Files `delete` failed on, with its error. They are left in place.
    pub failed: Vec<(PathBuf, String)>,
}

/// Removes the planned files through `delete`, which is `recycle` outside of tests.
pub fn execute_resolution<E: Display>(
    plan: &ResolutionPlan,
    mut delete: impl FnMut(&Path) -> Result<(), E>,
) -> ResolutionSummary {
    for hash in &plan.rejected {
        log::error!("Skipping conflict {hash}: its keeper is not one of its files");
    }

    let mut summary = ResolutionSummary::default();
    for path in &plan.deletions {
        match delete(path) {
            Ok(()) => {
                log::info!("Removed duplicate {path:?}");
                summary.removed.push(path.clone());
            }
            Err(e) => {
                log::error!("Failed to remove {path:?}: {e}");
                summary.failed.push((path.clone(), e.to_string()));
            }
        }
    }

    log::info!(
        "Resolution complete: {} removed, {} failed",
        summary.removed.len(),
        summary.failed.len()
    );
    summary
}

/// Moves `path` to the Recycle Bin.
pub fn recycle(path: &Path) -> Result<(), trash::Error> {
    trash::delete(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io;
    use tempfile::{TempDir, tempdir};

    fn pair(path: &str, hash: &str) -> (PathBuf, String) {
        (PathBuf::from(path), hash.to_string())
    }

    fn group(hash: &str, paths: &[&str]) -> (String, Vec<PathBuf>) {
        (hash.to_string(), paths.iter().map(PathBuf::from).collect())
    }

    #[test]
    fn group_duplicates_of_empty_input_is_empty() {
        assert!(group_duplicates(Vec::new()).is_empty());
    }

    #[test]
    fn group_duplicates_drops_singletons() {
        let files = vec![
            pair("a/one.txt", "h1"),
            pair("b/two.txt", "h2"),
            pair("c/three.txt", "h1"),
            pair("d/four.txt", "h3"),
        ];

        assert_eq!(
            group_duplicates(files),
            vec![group("h1", &["a/one.txt", "c/three.txt"])]
        );
    }

    #[test]
    fn group_duplicates_order_is_stable_across_input_permutations() {
        let files = vec![
            pair("x/1", "hb"),
            pair("x/2", "ha"),
            pair("y/1", "hb"),
            pair("y/2", "ha"),
            pair("z/1", "hb"),
            pair("z/2", "hc"),
        ];
        let expected = vec![
            group("ha", &["x/2", "y/2"]),
            group("hb", &["x/1", "y/1", "z/1"]),
        ];

        for rotation in 0..files.len() {
            let mut rotated = files.clone();
            rotated.rotate_left(rotation);
            assert_eq!(group_duplicates(rotated.clone()), expected);

            rotated.reverse();
            assert_eq!(group_duplicates(rotated), expected);
        }
    }

    #[test]
    fn group_duplicates_many_groups() {
        // Group g has g + 2 members; every tenth hash also gets a singleton that must be dropped.
        let mut files = Vec::new();
        for g in 0..100 {
            for member in 0..g + 2 {
                files.push((
                    PathBuf::from(format!("dir{member}/file{g:03}")),
                    format!("hash{g:03}"),
                ));
            }
            if g % 10 == 0 {
                files.push((PathBuf::from(format!("lonely{g}")), format!("single{g:03}")));
            }
        }

        let groups = group_duplicates(files);

        assert_eq!(groups.len(), 100);
        for (g, (hash, paths)) in groups.iter().enumerate() {
            assert_eq!(hash, &format!("hash{g:03}"));
            assert_eq!(paths.len(), g + 2);
            assert!(paths.is_sorted(), "paths of {hash} not sorted: {paths:?}");
        }
    }

    fn conflict_map(groups: &[(&str, &[&str])]) -> HashMap<String, Vec<PathBuf>> {
        groups
            .iter()
            .map(|(hash, paths)| (hash.to_string(), paths.iter().map(PathBuf::from).collect()))
            .collect()
    }

    fn keepers(keepers: &[(&str, &str)]) -> HashMap<String, PathBuf> {
        keepers
            .iter()
            .map(|(hash, path)| (hash.to_string(), PathBuf::from(path)))
            .collect()
    }

    fn paths(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn plan_never_includes_the_keeper() {
        let groups = conflict_map(&[("h", &["a", "b", "c"])]);

        for keeper in ["a", "b", "c"] {
            let plan = plan_resolution(&groups, &keepers(&[("h", keeper)]));

            let others: Vec<&str> = ["a", "b", "c"]
                .into_iter()
                .filter(|path| *path != keeper)
                .collect();
            assert_eq!(plan.deletions, paths(&others));
            assert!(plan.rejected.is_empty());
        }
    }

    #[test]
    fn plan_leaves_groups_without_a_keeper_untouched() {
        let groups = conflict_map(&[("h1", &["a", "b"]), ("h2", &["c", "d"])]);

        assert_eq!(
            plan_resolution(&groups, &HashMap::new()),
            ResolutionPlan::default()
        );
        assert_eq!(
            plan_resolution(&groups, &keepers(&[("h2", "d")])).deletions,
            paths(&["c"])
        );
        // A keeper for a group that no longer exists plans nothing and rejects nothing.
        assert_eq!(
            plan_resolution(&groups, &keepers(&[("gone", "a")])),
            ResolutionPlan::default()
        );
    }

    #[test]
    fn plan_covers_multiple_groups_in_hash_order() {
        let groups = conflict_map(&[
            ("hb", &["x/1", "y/1", "z/1"]),
            ("hc", &["q", "r"]),
            ("ha", &["x/2", "y/2"]),
        ]);

        let plan = plan_resolution(&groups, &keepers(&[("hb", "y/1"), ("ha", "x/2")]));

        assert_eq!(plan.deletions, paths(&["y/2", "x/1", "z/1"]));
        assert!(plan.rejected.is_empty());
    }

    #[test]
    fn plan_rejects_a_keeper_that_is_not_a_member() {
        let groups = conflict_map(&[
            ("h1", &["a", "b"]),
            ("h2", &["c", "d"]),
            ("h3", &["e", "f"]),
        ]);

        // An empty path was the old conflicts table placeholder.
        let plan = plan_resolution(
            &groups,
            &keepers(&[("h1", ""), ("h2", "c"), ("h3", "elsewhere")]),
        );

        assert_eq!(plan.deletions, paths(&["d"]));
        assert_eq!(plan.rejected, vec!["h1".to_string(), "h3".to_string()]);
    }

    /// a.txt, b.txt and c.txt in a temp dir, all with the same content.
    fn same_files() -> (TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = tempdir().unwrap();
        let [a, b, c] = ["a.txt", "b.txt", "c.txt"].map(|name| dir.path().join(name));
        for path in [&a, &b, &c] {
            fs::write(path, "same").unwrap();
        }
        (dir, a, b, c)
    }

    #[test]
    fn execute_resolution_removes_non_keepers_only() {
        let (_dir, a, b, c) = same_files();
        let groups = HashMap::from([("h".to_string(), vec![a.clone(), b.clone(), c.clone()])]);
        let plan = plan_resolution(&groups, &HashMap::from([("h".to_string(), a.clone())]));

        let summary = execute_resolution(&plan, |path| fs::remove_file(path));

        assert_eq!(summary.removed, vec![b.clone(), c.clone()]);
        assert!(summary.failed.is_empty());
        assert!(a.exists());
        assert!(!b.exists());
        assert!(!c.exists());
    }

    #[test]
    fn execute_resolution_of_a_placeholder_keeper_removes_nothing() {
        let (_dir, a, b, c) = same_files();
        let groups = HashMap::from([("h".to_string(), vec![a.clone(), b.clone(), c.clone()])]);
        let plan = plan_resolution(&groups, &HashMap::from([("h".to_string(), PathBuf::new())]));

        let summary = execute_resolution(&plan, |path| fs::remove_file(path));

        assert_eq!(summary, ResolutionSummary::default());
        assert!(a.exists());
        assert!(b.exists());
        assert!(c.exists());
    }

    #[test]
    fn execute_resolution_reports_a_failed_removal_and_leaves_the_file() {
        let (_dir, a, b, c) = same_files();
        let groups = HashMap::from([("h".to_string(), vec![a.clone(), b.clone(), c.clone()])]);
        let plan = plan_resolution(&groups, &HashMap::from([("h".to_string(), a.clone())]));

        let summary = execute_resolution(&plan, |path| {
            if path == b {
                Err(io::Error::other("file is in use"))
            } else {
                fs::remove_file(path)
            }
        });

        assert_eq!(summary.removed, vec![c.clone()]);
        assert_eq!(
            summary.failed,
            vec![(b.clone(), "file is in use".to_string())]
        );
        assert!(a.exists(), "the keeper is never touched");
        assert!(b.exists(), "a failed removal leaves the file in place");
        assert!(!c.exists());
    }
}
