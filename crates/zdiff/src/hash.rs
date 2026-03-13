use std::{io, path::Path};

pub fn hash_file(path: impl AsRef<Path>) -> io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_mmap_rayon(path)?;
    Ok(hasher.finalize().to_hex().to_string())
}