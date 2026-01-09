use std::io::{self};

pub fn hash_file(path: &str) -> io::Result<String> {
    // Safety: Memory mapping is unsafe because the file could be
    // truncated by another process while we are reading it.
    // For a file hasher, this is a standard risk to accept.
    let mut hasher = blake3::Hasher::new();
    hasher.update_mmap_rayon(&path)?;

    Ok(hasher.finalize().to_hex().to_string())
}
