use crc32fast::Hasher as Crc32Hasher;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use crate::hash::hash_file;

pub enum PathComparissonMethod {
    Byte,
    Hash,
    CrC,
}

pub enum PathComparisonResult {
    Byte { likeness: f32 },
    Hash { identical: bool },
    Crc { identical: bool },
}

impl PathComparisonResult {
    pub fn identical(&self) -> bool {
        match self {
            PathComparisonResult::Byte { likeness } => *likeness == 1.0,
            PathComparisonResult::Hash { identical } => *identical,
            PathComparisonResult::Crc { identical } => *identical,
        }
    }

    pub fn likeness(&self) -> f32 {
        match self {
            PathComparisonResult::Byte { likeness } => *likeness,
            PathComparisonResult::Hash { identical } | PathComparisonResult::Crc { identical } => {
                if *identical {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }
}

pub fn compare_paths(
    path1: &Path,
    path2: &Path,
    method: &PathComparissonMethod,
) -> io::Result<PathComparisonResult> {
    match method {
        PathComparissonMethod::Byte => compare_bytes(path1, path2),
        PathComparissonMethod::Hash => compare_hash(path1, path2),
        PathComparissonMethod::CrC => compare_crc(path1, path2),
    }
}

pub fn compare_bytes(path1: &Path, path2: &Path) -> io::Result<PathComparisonResult> {
    let mut file1 = File::open(path1)?;
    let mut file2 = File::open(path2)?;

    let metadata1 = file1.metadata()?;
    let metadata2 = file2.metadata()?;

    let len1 = metadata1.len();
    let len2 = metadata2.len();

    let max_len = len1.max(len2);

    // both are empty
    if max_len == 0 {
        return Ok(PathComparisonResult::Byte { likeness: 1.0 });
    }

    // TODO: tweak
    const BUF_SIZE: usize = 64 * 1024;
    let mut buf1 = [0u8; BUF_SIZE];
    let mut buf2 = [0u8; BUF_SIZE];

    let mut total_matches: u64 = 0;
    let mut total_compared: u64 = 0;

    loop {
        let n1 = file1.read(&mut buf1)?;
        let n2 = file2.read(&mut buf2)?;

        if n1 == 0 && n2 == 0 {
            break;
        }

        let min_read = n1.min(n2);

        total_matches += buf1[..min_read]
            .iter()
            .zip(&buf2[..min_read])
            .filter(|(a, b)| a == b)
            .count() as u64;

        total_compared += min_read as u64;
    }

    let likeness = total_matches as f32 / total_compared as f32;

    Ok(PathComparisonResult::Byte { likeness })
}

pub fn compare_hash(path1: &Path, path2: &Path) -> io::Result<PathComparisonResult> {
    let hash1 = hash_file(path1)?;
    let hash2 = hash_file(path2)?;

    Ok(PathComparisonResult::Hash {
        identical: hash1 == hash2,
    })
}

pub fn compare_crc(path1: &Path, path2: &Path) -> io::Result<PathComparisonResult> {
    fn crc_of_file(path: &Path) -> io::Result<u32> {
        let mut file = File::open(path)?;
        let mut hasher = Crc32Hasher::new();
        let mut buffer = [0u8; 8192];

        loop {
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }

        Ok(hasher.finalize())
    }

    let crc1 = crc_of_file(path1)?;
    let crc2 = crc_of_file(path2)?;

    Ok(PathComparisonResult::Crc {
        identical: crc1 == crc2,
    })
}
