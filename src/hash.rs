use std::{
    fs::File,
    io::{self, Read},
};

use memmap2::MmapOptions;

pub fn hash_file(path: &str) -> io::Result<String> {
    let file = File::open(path)?;

    // Safety: Memory mapping is unsafe because the file could be
    // truncated by another process while we are reading it.
    // For a file hasher, this is a standard risk to accept.
    let mmap = unsafe { MmapOptions::new().map(&file)? };

    let mut hasher = blake3::Hasher::new();
    hasher.update_rayon(&mmap);

    Ok(hasher.finalize().to_hex().to_string())
}
