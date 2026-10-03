use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
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

pub struct ResolveConflictsInput {
    pub conflict_map: HashMap<String, Vec<PathBuf>>,
    pub conflict_map_resolved: HashMap<String, PathBuf>,
}

pub struct ResolveConflictsOutput {
    pub removed_files: Vec<PathBuf>,
}

pub fn execute_resolution(input: &ResolveConflictsInput) -> ResolveConflictsOutput {
    let mut output = ResolveConflictsOutput {
        removed_files: Vec::new(),
    };

    log::info!("Starting file resolution process...");

    let conflicts = &input.conflict_map;
    let resolutions = &input.conflict_map_resolved;

    for (hash, paths) in conflicts {
        if let Some(path_to_keep) = resolutions.get(hash) {
            // A keeper outside the group (e.g. the table's empty-path placeholder) would
            // otherwise delete every copy.
            if !paths.contains(path_to_keep) {
                log::error!(
                    "Skipping conflict {hash}: keeper {path_to_keep:?} is not one of its files"
                );
                continue;
            }
            for path in paths {
                if path != path_to_keep {
                    match std::fs::remove_file(&path) {
                        Ok(_) => {
                            log::info!("Deleted duplicate: {:?}", path);
                            output.removed_files.push(path.clone());
                        }
                        Err(e) => {
                            log::error!("Failed to delete {:?}: {}", path, e);
                        }
                    }
                }
            }
        }
    }

    log::info!(
        "Resolution complete. Removed {} files.",
        output.removed_files.len()
    );
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

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

    #[test]
    fn execute_resolution_skips_group_whose_keeper_is_not_a_member() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "same").unwrap();
        fs::write(&b, "same").unwrap();

        let input = ResolveConflictsInput {
            conflict_map: HashMap::from([("h".to_string(), vec![a.clone(), b.clone()])]),
            // The conflicts table toggle stores an empty path as a "resolved" placeholder.
            conflict_map_resolved: HashMap::from([("h".to_string(), PathBuf::new())]),
        };

        let output = execute_resolution(&input);

        assert!(output.removed_files.is_empty());
        assert!(a.exists());
        assert!(b.exists());
    }

    #[test]
    fn execute_resolution_deletes_non_keepers_only() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, "same").unwrap();
        fs::write(&b, "same").unwrap();

        let input = ResolveConflictsInput {
            conflict_map: HashMap::from([("h".to_string(), vec![a.clone(), b.clone()])]),
            conflict_map_resolved: HashMap::from([("h".to_string(), a.clone())]),
        };

        let output = execute_resolution(&input);

        assert_eq!(output.removed_files, vec![b.clone()]);
        assert!(a.exists());
        assert!(!b.exists());
    }
}
