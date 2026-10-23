use std::path::{Component, Path, PathBuf};

use buglogin_domain::errors::Problem;

pub fn confined_path(root: &Path, relative: &Path) -> Result<PathBuf, Problem> {
    if relative.is_absolute() {
        return Err(invalid_path());
    }
    let root = root.canonicalize().map_err(|_| {
        Problem::new(
            "storage_root_unavailable",
            "Storage root unavailable",
            503,
            "storage",
        )
    })?;
    let mut candidate = root.clone();
    for component in relative.components() {
        let Component::Normal(part) = component else {
            return Err(invalid_path());
        };
        candidate.push(part);
        if candidate.exists()
            && candidate
                .symlink_metadata()
                .map_err(|_| invalid_path())?
                .file_type()
                .is_symlink()
        {
            return Err(invalid_path());
        }
    }
    if !candidate.starts_with(&root) {
        return Err(invalid_path());
    }
    Ok(candidate)
}

fn invalid_path() -> Problem {
    Problem::new(
        "invalid_storage_path",
        "Invalid storage path",
        400,
        "storage",
    )
}
