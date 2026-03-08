use std::{collections::HashMap, path::PathBuf};

// pub fn find_conflicts(
//     hashes: &HashMap<FsNodeId, Option<HashRepresentation>>,
//     selected: &HashMap<FsNodeId, bool>,
// ) -> HashMap<String, Vec<PathBuf>> {
//     todo!();

//     // Hashes can not be FsNodeId, hashing reads file from fs
//     let mut groups: HashMap<String, Vec<PathBuf>> = HashMap::new();
//     for (path, hash) in hashes {
//         if selected.get(path).copied().unwrap_or(false) {
//             if let Some(h) = hash {
//                 groups.entry(h.clone()).or_default().push("".into()); // path.clone()
//             }
//         }
//     }
//     groups.retain(|_, v| v.len() > 1);
//     groups
// }

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
