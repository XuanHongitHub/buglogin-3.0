use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use buglogin_domain::errors::Problem;
use buglogin_storage::{
    blob::{checksum, checksum_reader},
    paths::confined_path,
};

use super::catalog::{RuntimePackageFormat, RuntimeRelease, VerifiedRuntimeRelease};

pub trait RuntimeArtifactSource {
    fn fetch_to(
        &self,
        source: &str,
        maximum_bytes: u64,
        output: &mut dyn Write,
    ) -> Result<(), Problem>;

    fn on_stage(&self, _stage: &str) {}
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
    let release = release.release();
    if release.package_format == RuntimePackageFormat::Zip {
        return install_zip(root, release, source);
    }
    if release.package_format == RuntimePackageFormat::Dmg {
        return install_dmg(root, release, source);
    }
    let runtime_root = runtime_directory(root, release)?;
    fs::create_dir_all(&runtime_root)
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    let relative = Path::new(&release.relative_path);
    let target = confined_path(&runtime_root, relative)?;
    let parent = target
        .parent()
        .ok_or_else(|| problem("runtime_path_invalid", "Runtime path invalid"))?;
    fs::create_dir_all(parent)
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    if target.exists()
        && !target
            .symlink_metadata()
            .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?
            .file_type()
            .is_file()
    {
        return Err(problem(
            "runtime_target_invalid",
            "Runtime install target is not a regular file",
        ));
    }
    let mut staged = create_staging_file(&target)?;
    let mut file = staged
        .file
        .take()
        .ok_or_else(|| problem("runtime_install_failed", "Runtime install failed"))?;
    let mut bounded = BoundedWriter::new(&mut file, release.size_bytes);
    source.fetch_to(&release.source, release.size_bytes, &mut bounded)?;
    bounded
        .flush()
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    if bounded.written != release.size_bytes {
        return Err(problem("runtime_size_mismatch", "Runtime size mismatch"));
    }
    drop(bounded);
    file.sync_all()
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    drop(file);
    let (actual_checksum, actual_size) = checksum_reader(
        fs::File::open(&staged.path)
            .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?,
    )
    .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    if actual_size != release.size_bytes {
        return Err(problem("runtime_size_mismatch", "Runtime size mismatch"));
    }
    if actual_checksum != release.sha256.to_ascii_lowercase() {
        return Err(problem(
            "runtime_checksum_mismatch",
            "Runtime checksum mismatch",
        ));
    }
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
    if let Err(_) = fs::rename(&staged.path, &target) {
        if had_existing {
            let _ = fs::rename(&backup, &target);
        }
        return Err(problem("runtime_install_failed", "Runtime install failed"));
    }
    Ok(RuntimeInstallReceipt {
        installed_path: target,
        rollback_available: had_existing,
    })
}

const MAX_ARCHIVE_FILES: usize = 100_000;
const MAX_EXPANDED_BYTES: u64 = 3 * 1024 * 1024 * 1024;

fn install_zip(
    root: &Path,
    release: &RuntimeRelease,
    source: &impl RuntimeArtifactSource,
) -> Result<RuntimeInstallReceipt, Problem> {
    let target_directory = runtime_directory(root, release)?;
    let parent = target_directory
        .parent()
        .ok_or_else(|| problem("runtime_path_invalid", "Runtime path invalid"))?;
    fs::create_dir_all(parent)
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;

    let archive_target = parent.join("runtime-package.zip");
    let mut staged_archive = create_staging_file(&archive_target)?;
    let mut file = staged_archive
        .file
        .take()
        .ok_or_else(|| problem("runtime_install_failed", "Runtime install failed"))?;
    let mut bounded = BoundedWriter::new(&mut file, release.size_bytes);
    source.fetch_to(&release.source, release.size_bytes, &mut bounded)?;
    bounded
        .flush()
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    if bounded.written != release.size_bytes {
        return Err(problem("runtime_size_mismatch", "Runtime size mismatch"));
    }
    drop(bounded);
    file.sync_all()
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    drop(file);
    source.on_stage("verifying");
    verify_staged_artifact(&staged_archive.path, release)?;

    let staged_directory = create_staging_directory(parent, &target_directory)?;
    source.on_stage("extracting");
    extract_zip(&staged_archive.path, &staged_directory.path, release)?;
    let executable = staged_directory.path.join(&release.relative_path);
    let executable_metadata = fs::symlink_metadata(&executable)
        .map_err(|_| problem("runtime_executable_missing", "Runtime executable missing"))?;
    if !executable_metadata.file_type().is_file() {
        return Err(problem(
            "runtime_executable_invalid",
            "Runtime executable is not a regular file",
        ));
    }

    let backup = rollback_path(&target_directory)?;
    let had_existing = match fs::symlink_metadata(&target_directory) {
        Ok(metadata) if metadata.file_type().is_dir() => true,
        Ok(_) => {
            return Err(problem(
                "runtime_target_invalid",
                "Runtime install target is not a directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return Err(problem("runtime_install_failed", "Runtime install failed")),
    };
    remove_existing_directory_backup(&backup)?;
    if had_existing {
        fs::rename(&target_directory, &backup)
            .map_err(|_| problem("runtime_backup_failed", "Runtime backup failed"))?;
    }
    if fs::rename(&staged_directory.path, &target_directory).is_err() {
        if had_existing {
            let _ = fs::rename(&backup, &target_directory);
        }
        return Err(problem("runtime_install_failed", "Runtime install failed"));
    }
    Ok(RuntimeInstallReceipt {
        installed_path: target_directory.join(&release.relative_path),
        rollback_available: had_existing,
    })
}

#[cfg(not(target_os = "macos"))]
fn install_dmg(
    _root: &Path,
    _release: &RuntimeRelease,
    _source: &impl RuntimeArtifactSource,
) -> Result<RuntimeInstallReceipt, Problem> {
    Err(problem(
        "runtime_package_format_unsupported",
        "DMG runtime packages require macOS",
    ))
}

#[cfg(target_os = "macos")]
fn install_dmg(
    root: &Path,
    release: &RuntimeRelease,
    source: &impl RuntimeArtifactSource,
) -> Result<RuntimeInstallReceipt, Problem> {
    use std::{process::Command, time::Duration};

    let target_directory = runtime_directory(root, release)?;
    let parent = target_directory
        .parent()
        .ok_or_else(|| problem("runtime_path_invalid", "Runtime path invalid"))?;
    fs::create_dir_all(parent)
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;

    let staged_artifact = create_staging_file(&parent.join("runtime-package.dmg"))?;
    let mut file = staged_artifact
        .file
        .take()
        .ok_or_else(|| problem("runtime_install_failed", "Runtime install failed"))?;
    let mut bounded = BoundedWriter::new(&mut file, release.size_bytes);
    source.fetch_to(&release.source, release.size_bytes, &mut bounded)?;
    bounded
        .flush()
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    if bounded.written != release.size_bytes {
        return Err(problem("runtime_size_mismatch", "Runtime size mismatch"));
    }
    drop(bounded);
    file.sync_all()
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    drop(file);
    source.on_stage("verifying");
    verify_staged_artifact(&staged_artifact.path, release)?;

    let staged_directory = create_staging_directory(parent, &target_directory)?;
    let app_bundle = Path::new(&release.relative_path)
        .components()
        .next()
        .ok_or_else(|| problem("runtime_path_invalid", "Runtime path invalid"))?
        .as_os_str();
    let mountpoint = staged_directory.path.join(".mount");
    fs::create_dir(&mountpoint)
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    let mut mounted = DmgMount {
        mountpoint: mountpoint.clone(),
        mounted: true,
    };
    let mut attach = Command::new("hdiutil");
    attach
        .args([
            "attach",
            "-readonly",
            "-nobrowse",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(&mountpoint)
        .arg(&staged_artifact.path);
    run_macos_tool(
        &mut attach,
        Duration::from_secs(120),
        "runtime_dmg_mount_failed",
        "Runtime DMG mount failed",
    )?;
    let source_app = mountpoint.join(app_bundle);
    let source_app_metadata = fs::symlink_metadata(&source_app)
        .map_err(|_| problem("runtime_app_bundle_missing", "Runtime app bundle missing"))?;
    if !source_app_metadata.file_type().is_dir() {
        return Err(problem(
            "runtime_app_bundle_invalid",
            "Runtime app bundle is not a directory",
        ));
    }
    let source_executable = mountpoint.join(&release.relative_path);
    if !fs::symlink_metadata(&source_executable)
        .is_ok_and(|metadata| metadata.file_type().is_file())
    {
        return Err(problem(
            "runtime_executable_missing",
            "Runtime executable missing from DMG",
        ));
    }
    source.on_stage("extracting");
    let staged_app = staged_directory.path.join(app_bundle);
    let mut copy = Command::new("ditto");
    copy.args(["--rsrc", "--extattr", "--acl"])
        .arg(&source_app)
        .arg(&staged_app);
    run_macos_tool(
        &mut copy,
        Duration::from_secs(600),
        "runtime_install_failed",
        "Runtime install failed",
    )?;
    mounted.detach()?;
    fs::remove_dir(&mountpoint)
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    let executable = staged_directory.path.join(&release.relative_path);
    if !fs::symlink_metadata(&executable).is_ok_and(|metadata| metadata.file_type().is_file()) {
        return Err(problem(
            "runtime_executable_missing",
            "Runtime executable missing after DMG extraction",
        ));
    }

    let backup = runtime_directory_backup_path(&target_directory)?;
    let had_existing = match fs::symlink_metadata(&target_directory) {
        Ok(metadata) if metadata.file_type().is_dir() => true,
        Ok(_) => {
            return Err(problem(
                "runtime_target_invalid",
                "Runtime install target is not a directory",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => return Err(problem("runtime_install_failed", "Runtime install failed")),
    };
    remove_existing_directory_backup(&backup)?;
    if had_existing {
        fs::rename(&target_directory, &backup)
            .map_err(|_| problem("runtime_backup_failed", "Runtime backup failed"))?;
    }
    if fs::rename(&staged_directory.path, &target_directory).is_err() {
        if had_existing {
            let _ = fs::rename(&backup, &target_directory);
        }
        return Err(problem("runtime_install_failed", "Runtime install failed"));
    }
    Ok(RuntimeInstallReceipt {
        installed_path: target_directory.join(&release.relative_path),
        rollback_available: had_existing,
    })
}

#[cfg(target_os = "macos")]
struct DmgMount {
    mountpoint: PathBuf,
    mounted: bool,
}

#[cfg(target_os = "macos")]
impl DmgMount {
    fn detach(&mut self) -> Result<(), Problem> {
        let mut command = std::process::Command::new("hdiutil");
        command.args(["detach", "-quiet"]).arg(&self.mountpoint);
        run_macos_tool(
            &mut command,
            std::time::Duration::from_secs(120),
            "runtime_dmg_detach_failed",
            "Runtime DMG detach failed",
        )?;
        self.mounted = false;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
impl Drop for DmgMount {
    fn drop(&mut self) {
        if self.mounted {
            let mut command = std::process::Command::new("hdiutil");
            command
                .args(["detach", "-force", "-quiet"])
                .arg(&self.mountpoint);
            let _ = run_macos_tool(
                &mut command,
                std::time::Duration::from_secs(10),
                "runtime_dmg_detach_failed",
                "Runtime DMG detach failed",
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn run_macos_tool(
    command: &mut std::process::Command,
    timeout: std::time::Duration,
    error_code: &'static str,
    error_title: &'static str,
) -> Result<(), Problem> {
    use std::{
        process::Stdio,
        thread,
        time::{Duration, Instant},
    };

    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| problem(error_code, error_title))?;
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err(problem(error_code, error_title)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(problem(error_code, error_title));
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(problem(error_code, error_title));
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn verify_staged_artifact(path: &Path, release: &RuntimeRelease) -> Result<(), Problem> {
    let (actual_checksum, actual_size) = checksum_reader(
        fs::File::open(path)
            .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?,
    )
    .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
    if actual_size != release.size_bytes {
        return Err(problem("runtime_size_mismatch", "Runtime size mismatch"));
    }
    if actual_checksum != release.sha256.to_ascii_lowercase() {
        return Err(problem(
            "runtime_checksum_mismatch",
            "Runtime checksum mismatch",
        ));
    }
    Ok(())
}

pub fn runtime_directory(root: &Path, release: &RuntimeRelease) -> Result<PathBuf, Problem> {
    let runtime_root = runtime_directory_for(
        root,
        &release.runtime,
        &release.platform,
        &release.architecture,
    )?;
    let version_key = checksum(release.version.as_bytes());
    Ok(runtime_root.join("versions").join(version_key))
}

pub fn runtime_directory_for(
    root: &Path,
    runtime: &buglogin_domain::entities::Runtime,
    platform: &buglogin_domain::entities::RuntimePlatform,
    architecture: &buglogin_domain::entities::RuntimeArchitecture,
) -> Result<PathBuf, Problem> {
    let runtime = match runtime {
        buglogin_domain::entities::Runtime::Wayfern => "wayfern",
        buglogin_domain::entities::Runtime::Bugmini => "bugmini",
        _ => return Err(problem("runtime_not_supported", "Runtime not supported")),
    };
    let platform = match platform {
        buglogin_domain::entities::RuntimePlatform::Windows => "windows",
        buglogin_domain::entities::RuntimePlatform::Macos => "macos",
        buglogin_domain::entities::RuntimePlatform::Linux => "linux",
    };
    let architecture = match architecture {
        buglogin_domain::entities::RuntimeArchitecture::X64 => "x64",
        buglogin_domain::entities::RuntimeArchitecture::Arm64 => "arm64",
    };
    Ok(root.join(runtime).join(platform).join(architecture))
}

fn extract_zip(
    archive_path: &Path,
    destination: &Path,
    release: &RuntimeRelease,
) -> Result<(), Problem> {
    let archive_file = fs::File::open(archive_path)
        .map_err(|_| problem("runtime_archive_invalid", "Runtime archive invalid"))?;
    let mut archive = zip::ZipArchive::new(archive_file)
        .map_err(|_| problem("runtime_archive_invalid", "Runtime archive invalid"))?;
    if archive.len() == 0 || archive.len() > MAX_ARCHIVE_FILES {
        return Err(problem(
            "runtime_archive_limits_exceeded",
            "Runtime archive limits exceeded",
        ));
    }

    let prefix = release.archive_prefix.as_deref().map(Path::new);
    let mut explicit = HashMap::<String, (u32, u64)>::new();
    let mut kinds = HashMap::<String, bool>::new();
    let mut expanded_bytes = 0_u64;
    let mut expected_executable_found = false;

    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|_| problem("runtime_archive_invalid", "Runtime archive invalid"))?;
        let entry_name = entry.name().to_owned();
        let is_directory = entry.is_dir();
        if !safe_zip_entry_name(&entry_name, is_directory) || entry.is_symlink() {
            return Err(problem(
                "runtime_archive_entry_invalid",
                "Runtime archive entry invalid",
            ));
        }
        let enclosed = entry.enclosed_name().ok_or_else(|| {
            problem(
                "runtime_archive_path_invalid",
                "Runtime archive path invalid",
            )
        })?;
        if !safe_archive_path(&enclosed) {
            return Err(problem(
                "runtime_archive_path_invalid",
                "Runtime archive path invalid",
            ));
        }
        let relative = match prefix {
            Some(prefix) => match enclosed.strip_prefix(prefix) {
                Ok(path) => path.to_path_buf(),
                Err(_) if is_directory && prefix.starts_with(&enclosed) => continue,
                Err(_) => {
                    return Err(problem(
                        "runtime_archive_prefix_mismatch",
                        "Runtime archive prefix mismatch",
                    ));
                }
            },
            None => enclosed,
        };
        if relative.as_os_str().is_empty() {
            if is_directory {
                continue;
            }
            return Err(problem(
                "runtime_archive_path_invalid",
                "Runtime archive path invalid",
            ));
        }
        if !safe_archive_path(&relative) {
            return Err(problem(
                "runtime_archive_path_invalid",
                "Runtime archive path invalid",
            ));
        }
        let relative_key = archive_path_key(&relative);
        if let Some(&(prev_crc, prev_size)) = explicit.get(&relative_key) {
            if prev_crc == entry.crc32() && prev_size == entry.size() {
                if relative == Path::new(&release.relative_path) {
                    expected_executable_found = true;
                }
                continue;
            }
            return Err(problem(
                "runtime_archive_duplicate_path",
                "Runtime archive contains duplicate paths",
            ));
        }
        explicit.insert(relative_key.clone(), (entry.crc32(), entry.size()));
        register_archive_path(&relative, is_directory, &mut kinds)?;
        if !is_directory {
            if entry.size() > MAX_EXPANDED_BYTES.saturating_sub(expanded_bytes) {
                return Err(problem(
                    "runtime_archive_limits_exceeded",
                    "Runtime archive limits exceeded",
                ));
            }
            let output_path = destination.join(&relative);
            let output_parent = output_path.parent().ok_or_else(|| {
                problem(
                    "runtime_archive_path_invalid",
                    "Runtime archive path invalid",
                )
            })?;
            fs::create_dir_all(output_parent)
                .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output_path)
                .map_err(|_| {
                    problem(
                        "runtime_archive_duplicate_path",
                        "Runtime archive path conflict",
                    )
                })?;
            let copied =
                copy_expanded(&mut entry, &mut output, MAX_EXPANDED_BYTES - expanded_bytes)?;
            expanded_bytes = expanded_bytes.saturating_add(copied);
            output
                .sync_all()
                .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
            set_extracted_file_permissions(
                &output_path,
                entry.unix_mode(),
                &relative,
                &release.relative_path,
            )?;
            if relative == Path::new(&release.relative_path) {
                expected_executable_found = true;
            }
        } else {
            fs::create_dir_all(destination.join(&relative))
                .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
        }
    }

    if !expected_executable_found {
        return Err(problem(
            "runtime_executable_missing",
            "Runtime executable missing from archive",
        ));
    }
    Ok(())
}

fn copy_expanded(
    reader: &mut impl Read,
    writer: &mut impl Write,
    remaining: u64,
) -> Result<u64, Problem> {
    let mut copied = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|_| problem("runtime_archive_invalid", "Runtime archive invalid"))?;
        if count == 0 {
            return Ok(copied);
        }
        if count as u64 > remaining.saturating_sub(copied) {
            return Err(problem(
                "runtime_archive_limits_exceeded",
                "Runtime archive limits exceeded",
            ));
        }
        writer
            .write_all(&buffer[..count])
            .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
        copied = copied.saturating_add(count as u64);
    }
}

fn safe_archive_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

fn safe_zip_entry_name(name: &str, is_directory: bool) -> bool {
    if name.is_empty() || name.starts_with('/') || name.contains('\\') || name.contains(':') {
        return false;
    }
    let path = if is_directory {
        name.strip_suffix('/').unwrap_or(name)
    } else {
        name
    };
    !path.is_empty()
        && path
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
}

fn archive_path_key(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

fn register_archive_path(
    path: &Path,
    is_directory: bool,
    kinds: &mut HashMap<String, bool>,
) -> Result<(), Problem> {
    let components: Vec<_> = path.components().collect();
    let mut current = PathBuf::new();
    for (index, component) in components.iter().enumerate() {
        current.push(component.as_os_str());
        let key = archive_path_key(&current);
        let is_final = index + 1 == components.len();
        let required_directory = !is_final || is_directory;
        if kinds
            .get(&key)
            .is_some_and(|known_directory| !known_directory || !required_directory)
        {
            return Err(problem(
                "runtime_archive_path_conflict",
                "Runtime archive contains conflicting paths",
            ));
        }
        kinds.entry(key).or_insert(required_directory);
    }
    Ok(())
}

#[cfg(unix)]
fn set_extracted_file_permissions(
    path: &Path,
    archive_mode: Option<u32>,
    relative: &Path,
    executable: &str,
) -> Result<(), Problem> {
    use std::os::unix::fs::PermissionsExt;
    let is_executable = relative == Path::new(executable);
    let mut mode = archive_mode.map(|value| value & 0o777).unwrap_or(0o644);
    if is_executable {
        mode |= 0o111;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))
}

#[cfg(not(unix))]
fn set_extracted_file_permissions(
    _path: &Path,
    _archive_mode: Option<u32>,
    _relative: &Path,
    _executable: &str,
) -> Result<(), Problem> {
    Ok(())
}

fn runtime_directory_backup_path(directory: &Path) -> Result<PathBuf, Problem> {
    let name = directory
        .file_name()
        .ok_or_else(|| problem("runtime_path_invalid", "Runtime path invalid"))?;
    let mut backup_name = name.to_os_string();
    backup_name.push(".rollback");
    Ok(directory.with_file_name(backup_name))
}

fn remove_existing_directory_backup(backup: &Path) -> Result<(), Problem> {
    match fs::symlink_metadata(backup) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(problem(
            "runtime_backup_cleanup_failed",
            "Runtime backup cleanup failed",
        )),
        Ok(metadata) if metadata.file_type().is_dir() => fs::remove_dir_all(backup).map_err(|_| {
            problem(
                "runtime_backup_cleanup_failed",
                "Runtime backup cleanup failed",
            )
        }),
        Ok(_) => Err(problem(
            "runtime_backup_invalid",
            "Runtime rollback backup is not a directory",
        )),
    }
}

fn rollback_directory(root: &Path, release: &RuntimeRelease) -> Result<(), Problem> {
    let target = runtime_directory(root, release)?;
    let backup = runtime_directory_backup_path(&target)?;
    let backup_metadata = fs::symlink_metadata(&backup).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            problem(
                "runtime_rollback_unavailable",
                "Runtime rollback unavailable",
            )
        } else {
            problem("runtime_rollback_failed", "Runtime rollback failed")
        }
    })?;
    if !backup_metadata.file_type().is_dir() {
        return Err(problem(
            "runtime_backup_invalid",
            "Runtime rollback backup is not a directory",
        ));
    }
    match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return fs::rename(&backup, &target)
                .map_err(|_| problem("runtime_rollback_failed", "Runtime rollback failed"));
        }
        Err(_) => {
            return Err(problem(
                "runtime_rollback_failed",
                "Runtime rollback failed",
            ));
        }
        Ok(metadata) if !metadata.file_type().is_dir() => {
            return Err(problem(
                "runtime_target_invalid",
                "Runtime rollback target is not a directory",
            ));
        }
        Ok(_) => {}
    }
    let holding = create_rollback_holding_directory(&target)?;
    let held = holding.join("runtime");
    if fs::rename(&target, &held).is_err() {
        let _ = fs::remove_dir(&holding);
        return Err(problem(
            "runtime_rollback_failed",
            "Runtime rollback failed",
        ));
    }
    if fs::rename(&backup, &target).is_err() {
        let restored = fs::rename(&held, &target).is_ok();
        let _ = fs::remove_dir(&holding);
        return Err(problem(
            if restored {
                "runtime_rollback_failed"
            } else {
                "runtime_rollback_restore_failed"
            },
            "Runtime rollback failed",
        ));
    }
    fs::remove_dir_all(held).map_err(|_| {
        problem(
            "runtime_rollback_cleanup_failed",
            "Runtime rollback cleanup failed",
        )
    })?;
    fs::remove_dir(holding).map_err(|_| {
        problem(
            "runtime_rollback_cleanup_failed",
            "Runtime rollback cleanup failed",
        )
    })
}

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(1);

struct StagingFile {
    path: PathBuf,
    file: Option<fs::File>,
}

impl Drop for StagingFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

struct StagingDirectory {
    path: PathBuf,
}

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn create_staging_directory(parent: &Path, target: &Path) -> Result<StagingDirectory, Problem> {
    let name = target
        .file_name()
        .ok_or_else(|| problem("runtime_path_invalid", "Runtime path invalid"))?
        .to_string_lossy();
    for _ in 0..8 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(".{name}.{}.{}.stage", std::process::id(), sequence));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(StagingDirectory { path }),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(problem("runtime_install_failed", "Runtime install failed")),
        }
    }
    Err(problem(
        "runtime_install_in_progress",
        "Runtime install staging is busy",
    ))
}

fn create_staging_file(target: &Path) -> Result<StagingFile, Problem> {
    let name = target
        .file_name()
        .ok_or_else(|| problem("runtime_path_invalid", "Runtime path invalid"))?
        .to_string_lossy();
    for _ in 0..8 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = target.with_file_name(format!(
            ".{name}.{}.{}.download",
            std::process::id(),
            sequence
        ));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                return Ok(StagingFile {
                    path,
                    file: Some(file),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(problem("runtime_install_failed", "Runtime install failed")),
        }
    }
    Err(problem(
        "runtime_install_in_progress",
        "Runtime install staging is busy",
    ))
}

struct BoundedWriter<'a> {
    inner: &'a mut dyn Write,
    maximum: u64,
    written: u64,
}

impl<'a> BoundedWriter<'a> {
    fn new(inner: &'a mut dyn Write, maximum: u64) -> Self {
        Self {
            inner,
            maximum,
            written: 0,
        }
    }
}

impl Write for BoundedWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let remaining = self.maximum.saturating_sub(self.written);
        if buffer.len() as u64 > remaining {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "runtime artifact exceeds signed size",
            ));
        }
        let written = self.inner.write(buffer)?;
        self.written += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

pub fn rollback(root: &Path, release: &VerifiedRuntimeRelease) -> Result<(), Problem> {
    if matches!(
        release.release().package_format,
        RuntimePackageFormat::Zip | RuntimePackageFormat::Dmg
    ) {
        return rollback_directory(root, release.release());
    }
    let runtime_root = runtime_directory(root, release.release())?;
    let target = confined_path(&runtime_root, Path::new(&release.release().relative_path))?;
    let backup = rollback_path(&target)?;
    let backup_metadata = fs::symlink_metadata(&backup).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            problem(
                "runtime_rollback_unavailable",
                "Runtime rollback unavailable",
            )
        } else {
            problem("runtime_rollback_failed", "Runtime rollback failed")
        }
    })?;
    if !backup_metadata.file_type().is_file() {
        return Err(problem(
            "runtime_backup_invalid",
            "Runtime rollback backup is not a regular file",
        ));
    }

    match fs::symlink_metadata(&target) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return fs::rename(backup, target)
                .map_err(|_| problem("runtime_rollback_failed", "Runtime rollback failed"));
        }
        Err(_) => {
            return Err(problem(
                "runtime_rollback_failed",
                "Runtime rollback failed",
            ));
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(problem(
                "runtime_target_invalid",
                "Runtime install target is not a regular file",
            ));
        }
        Ok(_) => {}
    }

    let holding_directory = create_rollback_holding_directory(&target)?;
    let held_runtime = holding_directory.join(
        target
            .file_name()
            .ok_or_else(|| problem("runtime_path_invalid", "Runtime path invalid"))?,
    );
    if fs::rename(&target, &held_runtime).is_err() {
        let _ = fs::remove_dir(&holding_directory);
        return Err(problem(
            "runtime_rollback_failed",
            "Runtime rollback failed",
        ));
    }
    if fs::rename(&backup, &target).is_err() {
        let restored = fs::rename(&held_runtime, &target).is_ok();
        let _ = fs::remove_dir(&holding_directory);
        return Err(problem(
            if restored {
                "runtime_rollback_failed"
            } else {
                "runtime_rollback_restore_failed"
            },
            "Runtime rollback failed",
        ));
    }
    fs::remove_file(&held_runtime).map_err(|_| {
        problem(
            "runtime_rollback_cleanup_failed",
            "Runtime rollback cleanup failed",
        )
    })?;
    fs::remove_dir(&holding_directory).map_err(|_| {
        problem(
            "runtime_rollback_cleanup_failed",
            "Runtime rollback cleanup failed",
        )
    })
}

fn create_rollback_holding_directory(target: &Path) -> Result<PathBuf, Problem> {
    let name = target
        .file_name()
        .ok_or_else(|| problem("runtime_path_invalid", "Runtime path invalid"))?
        .to_string_lossy();
    for _ in 0..8 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = target.with_file_name(format!(
            ".{name}.{}.{}.rollback-current",
            std::process::id(),
            sequence
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => {
                return Err(problem(
                    "runtime_rollback_failed",
                    "Runtime rollback failed",
                ));
            }
        }
    }
    Err(problem(
        "runtime_rollback_in_progress",
        "Runtime rollback staging is busy",
    ))
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
    let status = match code {
        "runtime_path_invalid"
        | "runtime_archive_entry_invalid"
        | "runtime_archive_path_invalid"
        | "runtime_archive_prefix_mismatch" => 400,
        "runtime_size_mismatch" | "runtime_checksum_mismatch" => 422,
        "runtime_archive_invalid"
        | "runtime_archive_duplicate_path"
        | "runtime_archive_path_conflict"
        | "runtime_executable_missing"
        | "runtime_executable_invalid" => 422,
        "runtime_archive_limits_exceeded" => 413,
        "runtime_target_invalid"
        | "runtime_backup_invalid"
        | "runtime_rollback_unavailable"
        | "runtime_install_in_progress"
        | "runtime_rollback_in_progress" => 409,
        _ => 500,
    };
    Problem::new(code, title, status, "runtime-download")
}
