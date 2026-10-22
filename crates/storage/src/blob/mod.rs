use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

use buglogin_domain::errors::Problem;
use sha2::{Digest, Sha256};

use crate::paths::confined_path;

pub fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn checksum_reader(reader: impl Read) -> std::io::Result<(String, u64)> {
    let mut reader = reader;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "input size overflow")
        })?;
        hasher.update(&buffer[..read]);
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

pub fn write_verified(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    expected: &str,
) -> Result<(), Problem> {
    if checksum(bytes) != expected {
        return Err(Problem::new(
            "blob_checksum_mismatch",
            "Blob checksum mismatch",
            409,
            "storage",
        ));
    }
    let target = confined_path(root, relative)?;
    let parent = target.parent().ok_or_else(|| {
        Problem::new(
            "invalid_storage_path",
            "Invalid storage path",
            400,
            "storage",
        )
    })?;
    fs::create_dir_all(parent)
        .map_err(|_| Problem::new("blob_write_failed", "Blob write failed", 500, "storage"))?;
    let temp = target.with_extension("tmp");
    let mut file = fs::File::create(&temp)
        .map_err(|_| Problem::new("blob_write_failed", "Blob write failed", 500, "storage"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| Problem::new("blob_write_failed", "Blob write failed", 500, "storage"))?;
    fs::rename(&temp, &target)
        .map_err(|_| Problem::new("blob_write_failed", "Blob write failed", 500, "storage"))?;
    Ok(())
}

pub fn read_verified(root: &Path, relative: &Path, expected: &str) -> Result<Vec<u8>, Problem> {
    let bytes = fs::read(confined_path(root, relative)?)
        .map_err(|_| Problem::new("blob_not_found", "Blob not found", 404, "storage"))?;
    if checksum(&bytes) != expected {
        return Err(Problem::new(
            "blob_checksum_mismatch",
            "Blob checksum mismatch",
            409,
            "storage",
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::{checksum, checksum_reader};

    #[test]
    fn streaming_checksum_matches_byte_slice_checksum_and_reports_size() {
        let bytes = vec![0x5a; 256 * 1024 + 17];
        assert_eq!(
            checksum_reader(Cursor::new(&bytes)).unwrap(),
            (checksum(&bytes), bytes.len() as u64)
        );
    }
}
