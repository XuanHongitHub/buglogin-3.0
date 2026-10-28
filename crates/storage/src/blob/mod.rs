use std::{
    fs,
    io::{self, Read, Write},
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
    fs::create_dir_all(parent).map_err(blob_write_problem)?;
    // Profiles can be opened from more than one desktop process. A fixed
    // `<blob>.tmp` path lets concurrent hydrations truncate or rename each
    // other's staging file, which surfaced as a misleading `Blob write failed`.
    // NamedTempFile gives each writer a unique same-volume staging file and
    // persist atomically replaces an older cache entry on Windows and Unix.
    let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(blob_write_problem)?;
    staged
        .write_all(bytes)
        .and_then(|_| staged.as_file().sync_all())
        .map_err(blob_write_problem)?;
    match staged.persist(&target) {
        Ok(_) => {}
        Err(error) => {
            // On Windows, two writers can race to replace the same cache path;
            // the losing rename may receive a sharing violation even though
            // the winner has already published the exact verified bytes.
            // Treat that as success only after checking the published checksum.
            if fs::read(&target).is_ok_and(|published| checksum(&published) == expected) {
                return Ok(());
            }
            return Err(blob_write_problem(error.error));
        }
    }
    Ok(())
}

fn blob_write_problem(error: io::Error) -> Problem {
    match error.kind() {
        io::ErrorKind::StorageFull => Problem::new(
            "device_storage_full",
            "Device storage is full",
            507,
            "storage",
        ),
        io::ErrorKind::PermissionDenied => Problem::new(
            "blob_storage_not_writable",
            "Profile storage is not writable",
            500,
            "storage",
        ),
        _ => Problem::new("blob_write_failed", "Blob write failed", 500, "storage"),
    }
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
