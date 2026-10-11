use std::{fs, io::Write, path::Path};

use buglogin_domain::errors::Problem;
use sha2::{Digest, Sha256};

use crate::paths::confined_path;

pub fn checksum(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
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
