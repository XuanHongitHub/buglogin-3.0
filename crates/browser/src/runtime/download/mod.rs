use std::{
    fs,
    path::{Path, PathBuf},
};

use buglogin_domain::errors::Problem;
use buglogin_storage::{blob::write_verified, paths::confined_path};

use super::catalog::VerifiedRuntimeRelease;

pub trait RuntimeArtifactSource {
    fn fetch(&self, source: &str, maximum_bytes: u64) -> Result<Vec<u8>, Problem>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeInstallReceipt {
    pub installed_path: PathBuf,
    pub rollback_available: bool,
}

pub fn install(
    root: &Path,
    release: &VerifiedRuntimeRelease,
    source: &impl RuntimeArtifactSource,
) -> Result<RuntimeInstallReceipt, Problem> {
    let bytes = source.fetch(&release.release().source, release.release().size_bytes)?;
    release.verify_artifact(&bytes)?;
    let release = release.release();

    let relative = Path::new(&release.relative_path);
    let target = confined_path(root, relative)?;
    let backup = rollback_path(&target)?;
    let had_existing = target.exists();
    if backup.exists() {
        fs::remove_file(&backup).map_err(|_| {
            problem(
                "runtime_backup_cleanup_failed",
                "Runtime backup cleanup failed",
            )
        })?;
    }
    if had_existing {
        fs::rename(&target, &backup)
            .map_err(|_| problem("runtime_backup_failed", "Runtime backup failed"))?;
    }
    if let Err(error) = write_verified(root, relative, &bytes, &release.sha256.to_ascii_lowercase())
    {
        if had_existing {
            let _ = fs::rename(&backup, &target);
        }
        return Err(error);
    }
    Ok(RuntimeInstallReceipt {
        installed_path: target,
        rollback_available: had_existing,
    })
}

pub fn rollback(root: &Path, release: &VerifiedRuntimeRelease) -> Result<(), Problem> {
    let target = confined_path(root, Path::new(&release.release().relative_path))?;
    let backup = rollback_path(&target)?;
    if !backup.exists() {
        return Err(problem(
            "runtime_rollback_unavailable",
            "Runtime rollback unavailable",
        ));
    }
    if target.exists() {
        fs::remove_file(&target)
            .map_err(|_| problem("runtime_rollback_failed", "Runtime rollback failed"))?;
    }
    fs::rename(backup, target)
        .map_err(|_| problem("runtime_rollback_failed", "Runtime rollback failed"))
}

fn rollback_path(target: &Path) -> Result<PathBuf, Problem> {
    let name = target
        .file_name()
        .ok_or_else(|| problem("runtime_path_invalid", "Runtime path invalid"))?;
    let mut rollback_name = name.to_os_string();
    rollback_name.push(".rollback");
    Ok(target.with_file_name(rollback_name))
}

fn problem(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 409, "runtime-download")
}
