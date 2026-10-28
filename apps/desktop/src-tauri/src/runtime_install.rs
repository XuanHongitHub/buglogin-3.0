use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::Path,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};

use buglogin_browser::runtime::{RuntimeBinary, catalog::VerifiedRuntimeRelease, download};
use buglogin_domain::entities::{RuntimeArchitecture, RuntimePlatform};
use buglogin_domain::{entities::Runtime, errors::Problem};
use buglogin_storage::{blob::checksum_reader, paths::confined_path};
use fs2::FileExt;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::adapter::{RuntimeTarget, RuntimeTargets};

const MAX_INSTALL_RECORD_BYTES: u64 = 64 * 1024;
const RUNTIME_DOWNLOAD_REQUEST_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const RUNTIME_DOWNLOAD_TOTAL_TIMEOUT: Duration = Duration::from_secs(45 * 60);
const RUNTIME_DOWNLOAD_MAX_RETRIES: u8 = 4;
static INSTALL_RECORD_SEQUENCE: AtomicU64 = AtomicU64::new(1);
static RUNTIME_OPERATION_GATES: OnceLock<
    std::sync::Mutex<BTreeMap<String, Arc<tokio::sync::RwLock<()>>>>,
> = OnceLock::new();

fn runtime_operation_gate(key: &str) -> Arc<tokio::sync::RwLock<()>> {
    let gates = RUNTIME_OPERATION_GATES.get_or_init(|| std::sync::Mutex::new(BTreeMap::new()));
    let mut gates = gates
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(
        gates
            .entry(key.to_owned())
            .or_insert_with(|| Arc::new(tokio::sync::RwLock::new(()))),
    )
}

fn runtime_operation_key(
    runtime: &buglogin_domain::entities::Runtime,
    platform: &RuntimePlatform,
    architecture: &RuntimeArchitecture,
    version: &str,
) -> String {
    buglogin_storage::blob::checksum(
        format!(
            "{}\0{}\0{}\0{version}",
            runtime_name(runtime),
            runtime_platform_name(platform),
            runtime_architecture_name(architecture)
        )
        .as_bytes(),
    )
}

fn confined_runtime_path(root: &Path, path: &Path) -> Result<PathBuf, Problem> {
    let relative = path.strip_prefix(root).map_err(|_| {
        problem(
            "runtime_install_path_invalid",
            "Runtime path is outside the runtime root",
        )
    })?;
    confined_path(root, relative).map_err(|_| {
        problem(
            "runtime_install_path_invalid",
            "Runtime path is outside the runtime root",
        )
    })
}

fn runtime_platform_name(platform: &RuntimePlatform) -> &'static str {
    match platform {
        RuntimePlatform::Windows => "windows",
        RuntimePlatform::Macos => "macos",
        RuntimePlatform::Linux => "linux",
    }
}

fn runtime_architecture_name(architecture: &RuntimeArchitecture) -> &'static str {
    match architecture {
        RuntimeArchitecture::X64 => "x64",
        RuntimeArchitecture::Arm64 => "arm64",
    }
}

fn runtime_install_busy(correlation_id: &str) -> Problem {
    Problem::new(
        "runtime_install_in_progress",
        "A browser runtime update is in progress; retry profile launch shortly",
        409,
        correlation_id,
    )
}

pub(crate) struct RuntimeLaunchGuard {
    _process_guard: tokio::sync::OwnedRwLockReadGuard<()>,
    _file_guard: File,
}

pub(crate) async fn acquire_runtime_launch_guard(
    runtime: &buglogin_domain::entities::Runtime,
    version: &str,
    correlation_id: &str,
) -> Result<RuntimeLaunchGuard, Problem> {
    let (platform, architecture) = current_platform_architecture(correlation_id)?;
    let key = runtime_operation_key(runtime, &platform, &architecture, version);
    let process_guard = runtime_operation_gate(&key)
        .try_read_owned()
        .map_err(|_| runtime_install_busy(correlation_id))?;
    let correlation_id = correlation_id.to_owned();
    let worker_correlation_id = correlation_id.clone();
    let file_guard = tauri::async_runtime::spawn_blocking(move || {
        acquire_runtime_operation_lock(
            &crate::runtime_config::runtime_install_root()?,
            &key,
            false,
            &worker_correlation_id,
        )
    })
    .await
    .map_err(|_| {
        Problem::new(
            "runtime_operation_lock_unavailable",
            "Browser runtime operation lock is unavailable",
            503,
            correlation_id,
        )
    })??;
    Ok(RuntimeLaunchGuard {
        _process_guard: process_guard,
        _file_guard: file_guard,
    })
}

fn acquire_runtime_operation_lock(
    root: &Path,
    key: &str,
    exclusive: bool,
    correlation_id: &str,
) -> Result<File, Problem> {
    fs::create_dir_all(root).map_err(|_| {
        Problem::new(
            "runtime_install_path_unavailable",
            "Runtime install path unavailable",
            503,
            correlation_id,
        )
    })?;
    let root = root.canonicalize().map_err(|_| {
        Problem::new(
            "runtime_install_path_unavailable",
            "Runtime install path unavailable",
            503,
            correlation_id,
        )
    })?;
    let lock_name = format!(".runtime-operation-{key}.lock");
    let lock_path = confined_path(&root, Path::new(&lock_name)).map_err(|_| {
        Problem::new(
            "runtime_operation_lock_unavailable",
            "Browser runtime operation lock is unavailable",
            503,
            correlation_id,
        )
    })?;
    let file = open_runtime_operation_lock_file(&lock_path).map_err(|_| {
        Problem::new(
            "runtime_operation_lock_unavailable",
            "Browser runtime operation lock is unavailable",
            503,
            correlation_id,
        )
    })?;
    let lock_result = if exclusive {
        FileExt::try_lock_exclusive(&file)
    } else {
        FileExt::try_lock_shared(&file)
    };
    lock_result.map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock
            || cfg!(windows) && error.raw_os_error() == Some(33)
        {
            runtime_install_busy(correlation_id)
        } else {
            Problem::new(
                "runtime_operation_lock_unavailable",
                "Browser runtime operation lock is unavailable",
                503,
                correlation_id,
            )
        }
    })?;
    Ok(file)
}

fn open_runtime_operation_lock_file(path: &Path) -> io::Result<File> {
    for _ in 0..3 {
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                match OpenOptions::new().read(true).write(true).open(path) {
                    Ok(file) => return Ok(file),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                    Err(error) => return Err(error),
                }
            }
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "runtime lock path is not a regular file",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match OpenOptions::new()
                    .create_new(true)
                    .read(true)
                    .write(true)
                    .open(path)
                {
                    Ok(file) => return Ok(file),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::WouldBlock,
        "runtime lock path changed while it was opened",
    ))
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InstalledRuntimeRecord {
    schema_version: u32,
    catalog_schema_version: u32,
    release: buglogin_browser::runtime::catalog::RuntimeRelease,
    executable_sha256: String,
}

pub(crate) fn current_platform_architecture(
    correlation_id: &str,
) -> Result<(RuntimePlatform, RuntimeArchitecture), Problem> {
    let platform = if cfg!(target_os = "windows") {
        RuntimePlatform::Windows
    } else if cfg!(target_os = "macos") {
        RuntimePlatform::Macos
    } else if cfg!(target_os = "linux") {
        RuntimePlatform::Linux
    } else {
        return Err(Problem::new(
            "runtime_platform_unsupported",
            "This device operating system has no supported browser runtime",
            409,
            correlation_id,
        ));
    };
    let architecture = match std::env::consts::ARCH {
        "x86_64" => RuntimeArchitecture::X64,
        "aarch64" => RuntimeArchitecture::Arm64,
        _ => {
            return Err(Problem::new(
                "runtime_architecture_unsupported",
                "This device architecture has no supported browser runtime",
                409,
                correlation_id,
            ));
        }
    };
    Ok((platform, architecture))
}

pub(crate) fn has_install_records(root: &Path) -> Result<bool, Problem> {
    let (platform, architecture) = current_platform_architecture("desktop-runtime-bootstrap")?;
    Ok(!install_record_paths(root, &Runtime::Wayfern, &platform, &architecture)?.is_empty())
}

pub(crate) fn persist_install_record(
    root: &Path,
    catalog_schema_version: u32,
    release: &buglogin_browser::runtime::catalog::RuntimeRelease,
    target: &RuntimeTarget,
) -> Result<(), Problem> {
    let record_path = download::runtime_directory(root, release)?.join(".buglogin-install.json");
    let parent = record_path.parent().ok_or_else(|| {
        problem(
            "runtime_install_path_unavailable",
            "Runtime install path unavailable",
        )
    })?;
    fs::create_dir_all(parent).map_err(|_| {
        problem(
            "runtime_install_path_unavailable",
            "Runtime install path unavailable",
        )
    })?;
    let record = InstalledRuntimeRecord {
        schema_version: 1,
        catalog_schema_version,
        release: release.clone(),
        executable_sha256: target.binary.expected_checksum.clone(),
    };
    let bytes = serde_json::to_vec(&record).map_err(|_| {
        problem(
            "runtime_install_record_invalid",
            "Runtime install record invalid",
        )
    })?;
    if bytes.len() as u64 > MAX_INSTALL_RECORD_BYTES {
        return Err(problem(
            "runtime_install_record_invalid",
            "Runtime install record invalid",
        ));
    }
    let sequence = INSTALL_RECORD_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staged_path = record_path.with_file_name(format!(
        ".runtime-install.{}.{}.tmp",
        std::process::id(),
        sequence
    ));
    let mut staged = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged_path)
        .map_err(|_| {
            problem(
                "runtime_install_record_write_failed",
                "Runtime install record could not be written",
            )
        })?;
    let write_result = staged.write_all(&bytes).and_then(|()| staged.sync_all());
    drop(staged);
    if write_result.is_err() {
        let _ = fs::remove_file(&staged_path);
        return Err(problem(
            "runtime_install_record_write_failed",
            "Runtime install record could not be written",
        ));
    }
    if let Err(error) = replace_file(&staged_path, &record_path) {
        let _ = fs::remove_file(&staged_path);
        return Err(error);
    }
    Ok(())
}

pub(crate) fn load_installed_targets(
    root: &Path,
    verifier: &impl buglogin_browser::runtime::catalog::RuntimeSignatureVerifier,
) -> Result<RuntimeTargets, Problem> {
    let (platform, architecture) = current_platform_architecture("desktop-runtime-bootstrap")?;
    let mut targets = RuntimeTargets::default();
    let runtime = Runtime::Wayfern;
    let root = match root.canonicalize() {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(targets),
        Err(_) => {
            return Err(problem(
                "runtime_install_path_unavailable",
                "Runtime install path unavailable",
            ));
        }
    };
    let root_metadata = fs::metadata(&root).map_err(|_| {
        problem(
            "runtime_install_path_unavailable",
            "Runtime install path unavailable",
        )
    })?;
    if !root_metadata.is_dir() {
        return Err(problem(
            "runtime_install_path_invalid",
            "Runtime install path is not a directory",
        ));
    }
    for path in install_record_paths(&root, &runtime, &platform, &architecture)? {
        match load_installed_target(&root, &path, &runtime, &platform, &architecture, verifier) {
            Ok(target) => targets.insert(runtime.clone(), target),
            Err(error) => tauri_plugin_log::log::warn!(
                "Ignoring unverified installed browser runtime during startup: code={}",
                error.code
            ),
        }
    }
    Ok(targets)
}

fn load_installed_target(
    root: &Path,
    path: &Path,
    runtime: &Runtime,
    platform: &RuntimePlatform,
    architecture: &RuntimeArchitecture,
    verifier: &impl buglogin_browser::runtime::catalog::RuntimeSignatureVerifier,
) -> Result<RuntimeTarget, Problem> {
    let path = confined_runtime_path(root, path)?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| {
        problem(
            "runtime_install_record_unavailable",
            "Runtime install record unavailable",
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(problem(
            "runtime_install_record_invalid",
            "Runtime install record invalid",
        ));
    }
    if metadata.len() > MAX_INSTALL_RECORD_BYTES {
        return Err(problem(
            "runtime_install_record_invalid",
            "Runtime install record invalid",
        ));
    }
    let bytes = fs::read(&path).map_err(|_| {
        problem(
            "runtime_install_record_unavailable",
            "Runtime install record unavailable",
        )
    })?;
    let record: InstalledRuntimeRecord = serde_json::from_slice(&bytes).map_err(|_| {
        problem(
            "runtime_install_record_invalid",
            "Runtime install record invalid",
        )
    })?;
    if record.schema_version != 1
        || &record.release.runtime != runtime
        || &record.release.platform != platform
        || &record.release.architecture != architecture
        || record.executable_sha256.len() != 64
        || !record
            .executable_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(problem(
            "runtime_install_record_invalid",
            "Runtime install record invalid",
        ));
    }
    let catalog = buglogin_browser::runtime::catalog::RuntimeCatalog {
        schema_version: record.catalog_schema_version,
        releases: vec![record.release.clone()],
    };
    let verified = catalog.verify(verifier).map_err(|_| {
        problem(
            "runtime_signature_invalid",
            "Installed runtime signature is invalid",
        )
    })?;
    let verified_release = verified
        .first()
        .ok_or_else(|| problem("runtime_catalog_empty", "Runtime catalog is empty"))?;
    let runtime_dir = path.parent().ok_or_else(|| {
        problem(
            "runtime_install_record_invalid",
            "Runtime install record invalid",
        )
    })?;
    let base_dir = download::runtime_directory_for(
        root,
        &record.release.runtime,
        &record.release.platform,
        &record.release.architecture,
    )?;
    let expected_version_dir = download::runtime_directory(root, &record.release)?;
    let legacy_record = runtime_dir == base_dir;
    if !legacy_record && runtime_dir != expected_version_dir {
        return Err(problem(
            "runtime_install_record_invalid",
            "Runtime install record invalid",
        ));
    }
    let executable = confined_runtime_path(root, &runtime_dir.join(&record.release.relative_path))?;
    let (actual_checksum, _) = checksum_reader(File::open(&executable).map_err(|_| {
        problem(
            "runtime_executable_missing",
            "Installed runtime executable is missing",
        )
    })?)
    .map_err(|_| {
        problem(
            "runtime_executable_invalid",
            "Installed runtime executable is invalid",
        )
    })?;
    if actual_checksum != record.executable_sha256.to_ascii_lowercase() {
        return Err(problem(
            "runtime_executable_checksum_mismatch",
            "Installed runtime executable checksum is invalid",
        ));
    }
    Ok(RuntimeTarget {
        binary: RuntimeBinary {
            path: executable,
            version: verified_release.release().version.clone(),
            expected_checksum: actual_checksum,
        },
        extra_args: Vec::new(),
    })
}

fn install_record_paths(
    root: &Path,
    runtime: &Runtime,
    platform: &RuntimePlatform,
    architecture: &RuntimeArchitecture,
) -> Result<Vec<PathBuf>, Problem> {
    let base = download::runtime_directory_for(root, runtime, platform, architecture)?;
    let _ = confined_runtime_path(root, &base)?;
    match fs::symlink_metadata(&base) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Ok(_) => {
            return Err(problem(
                "runtime_install_path_invalid",
                "Runtime inventory root is not a directory",
            ));
        }
        Err(_) => {
            return Err(problem(
                "runtime_install_path_unavailable",
                "Runtime inventory root is unavailable",
            ));
        }
    }
    let mut records = Vec::new();
    let legacy = base.join(".buglogin-install.json");
    match fs::symlink_metadata(&legacy) {
        Ok(_) => records.push(legacy),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(problem(
                "runtime_install_path_unavailable",
                "Runtime install record is unavailable",
            ));
        }
    }
    let versions = base.join("versions");
    let _ = confined_runtime_path(root, &versions)?;
    match fs::read_dir(&versions) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|_| {
                    problem(
                        "runtime_install_path_unavailable",
                        "Runtime inventory entry is unavailable",
                    )
                })?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !is_version_directory_name(&name) {
                    continue;
                }
                let metadata = fs::symlink_metadata(entry.path()).map_err(|_| {
                    problem(
                        "runtime_install_path_unavailable",
                        "Runtime inventory entry is unavailable",
                    )
                })?;
                if !metadata.file_type().is_dir() {
                    tauri_plugin_log::log::warn!(
                        "Skipping non-directory browser runtime inventory entry"
                    );
                    continue;
                }
                let record = entry.path().join(".buglogin-install.json");
                match fs::symlink_metadata(&record) {
                    Ok(_) => records.push(record),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(_) => {
                        return Err(problem(
                            "runtime_install_path_unavailable",
                            "Runtime install record is unavailable",
                        ));
                    }
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(problem(
                "runtime_install_path_unavailable",
                "Browser runtime inventory is unavailable",
            ));
        }
    }
    Ok(records)
}

fn is_version_directory_name(name: &str) -> bool {
    name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn replace_file(staged: &Path, target: &Path) -> Result<(), Problem> {
    if target.exists() {
        let backup = target.with_file_name(".buglogin-install.json.previous");
        if backup.exists() {
            fs::remove_file(&backup).map_err(|_| {
                problem(
                    "runtime_install_record_write_failed",
                    "Runtime install record could not be written",
                )
            })?;
        }
        fs::rename(target, &backup).map_err(|_| {
            problem(
                "runtime_install_record_write_failed",
                "Runtime install record could not be written",
            )
        })?;
        if fs::rename(staged, target).is_err() {
            let _ = fs::rename(&backup, target);
            return Err(problem(
                "runtime_install_record_write_failed",
                "Runtime install record could not be written",
            ));
        }
        fs::remove_file(backup).map_err(|_| {
            problem(
                "runtime_install_record_cleanup_failed",
                "Runtime install record cleanup failed",
            )
        })?;
        return Ok(());
    }
    fs::rename(staged, target).map_err(|_| {
        problem(
            "runtime_install_record_write_failed",
            "Runtime install record could not be written",
        )
    })
}

pub(crate) fn install(
    root: &Path,
    release: &VerifiedRuntimeRelease,
    catalog_schema_version: u32,
    app: &tauri::AppHandle,
) -> Result<RuntimeTarget, Problem> {
    let (host_platform, host_architecture) =
        current_platform_architecture("desktop-runtime-install")?;
    if release.release().platform != host_platform
        || release.release().architecture != host_architecture
    {
        return Err(Problem::new(
            "runtime_target_host_mismatch",
            "Browser runtime target does not match this device operating system and architecture",
            409,
            "desktop-runtime-install",
        ));
    }
    fs::create_dir_all(root).map_err(|_| {
        problem(
            "runtime_install_path_unavailable",
            "Runtime install path unavailable",
        )
    })?;
    let canonical_root = root.canonicalize().map_err(|_| {
        problem(
            "runtime_install_path_unavailable",
            "Runtime install path unavailable",
        )
    })?;
    let runtime_directory = download::runtime_directory(&canonical_root, release.release())?;
    let _ = confined_runtime_path(&canonical_root, &runtime_directory)?;
    let runtime = &release.release().runtime;
    let version = &release.release().version;
    let operation_key = runtime_operation_key(runtime, &host_platform, &host_architecture, version);
    let _runtime_update_process_guard = runtime_operation_gate(&operation_key)
        .try_write_owned()
        .map_err(|_| runtime_install_busy("desktop-runtime-install"))?;
    let _runtime_update_guard = acquire_runtime_operation_lock(
        &canonical_root,
        &operation_key,
        true,
        "desktop-runtime-install",
    )?;
    let runtime = runtime_name(&release.release().runtime);
    let version = release.release().version.clone();
    // A launch-time install and the startup preloader can race. Recheck after
    // taking the lock so the second caller reuses the completed install.
    if let Some(target) = existing_install(&canonical_root, release, catalog_schema_version)? {
        return Ok(target);
    }
    let source = HttpsArtifactSource::new(app.clone(), runtime.to_owned(), version.clone())?;
    emit_progress(
        &source.app,
        &runtime,
        &version,
        0,
        0,
        0,
        Some(0),
        "downloading",
    );
    let receipt = match download::install(&canonical_root, release, &source) {
        Ok(receipt) => receipt,
        Err(error) => {
            emit_progress(
                &source.app,
                &runtime,
                &version,
                0,
                release.release().size_bytes,
                0,
                None,
                "error",
            );
            return Err(error);
        }
    };
    let prepared = (|| -> Result<RuntimeTarget, Problem> {
        let (expected_checksum, _) = checksum_reader(
            File::open(&receipt.installed_path)
                .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?,
        )
        .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
        let target = RuntimeTarget {
            binary: RuntimeBinary {
                path: receipt.installed_path,
                version: release.release().version.clone(),
                expected_checksum,
            },
            extra_args: Vec::new(),
        };
        persist_install_record(
            &canonical_root,
            catalog_schema_version,
            release.release(),
            &target,
        )?;
        Ok(target)
    })();
    match prepared {
        Ok(target) => Ok(target),
        Err(error) => {
            let cleanup = rollback_or_remove_failed_install(&canonical_root, release);
            emit_progress(
                &source.app,
                &runtime,
                &version,
                0,
                release.release().size_bytes,
                0,
                None,
                "error",
            );
            cleanup?;
            Err(error)
        }
    }
}

fn existing_install(
    root: &Path,
    release: &VerifiedRuntimeRelease,
    catalog_schema_version: u32,
) -> Result<Option<RuntimeTarget>, Problem> {
    let release = release.release();
    let runtime_dir = download::runtime_directory(root, release)?;
    let _ = confined_runtime_path(root, &runtime_dir)?;
    let record_path = runtime_dir.join(".buglogin-install.json");
    let _ = confined_runtime_path(root, &record_path)?;
    let metadata = match fs::symlink_metadata(&record_path) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Ok(_) => {
            return Err(problem(
                "runtime_install_record_invalid",
                "Runtime install record is not a regular file",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(problem(
                "runtime_install_record_unavailable",
                "Runtime install record could not be inspected",
            ));
        }
    };
    if metadata.len() > MAX_INSTALL_RECORD_BYTES {
        return Ok(None);
    }
    let record_bytes = fs::read(&record_path).map_err(|_| {
        problem(
            "runtime_install_record_unavailable",
            "Runtime install record could not be read",
        )
    })?;
    let record: InstalledRuntimeRecord =
        match serde_json::from_slice::<InstalledRuntimeRecord>(&record_bytes) {
            Ok(record)
                if record.schema_version == 1
                    && record.catalog_schema_version == catalog_schema_version
                    && record.release == *release
                    && record.executable_sha256.len() == 64
                    && record
                        .executable_sha256
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit()) =>
            {
                record
            }
            _ => return Ok(None),
        };
    let executable = confined_runtime_path(root, &runtime_dir.join(&release.relative_path))?;
    let executable_metadata = match fs::symlink_metadata(&executable) {
        Ok(metadata) if metadata.file_type().is_file() => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Ok(_) => return Ok(None),
        Err(_) => {
            return Err(problem(
                "runtime_executable_invalid",
                "Runtime executable could not be inspected",
            ));
        }
    };
    let _ = executable_metadata;
    let (actual_checksum, _) = checksum_reader(File::open(&executable).map_err(|_| {
        problem(
            "runtime_executable_invalid",
            "Runtime executable is invalid",
        )
    })?)
    .map_err(|_| {
        problem(
            "runtime_executable_invalid",
            "Runtime executable is invalid",
        )
    })?;
    if actual_checksum != record.executable_sha256.to_ascii_lowercase() {
        return Ok(None);
    }
    Ok(Some(RuntimeTarget {
        binary: RuntimeBinary {
            path: executable,
            version: release.version.clone(),
            expected_checksum: actual_checksum,
        },
        extra_args: Vec::new(),
    }))
}

fn rollback_or_remove_failed_install(
    root: &Path,
    release: &VerifiedRuntimeRelease,
) -> Result<(), Problem> {
    match download::rollback(root, release) {
        Ok(()) => Ok(()),
        Err(error) if error.code == "runtime_rollback_unavailable" => {
            let release = release.release();
            if release.package_format
                == buglogin_browser::runtime::catalog::RuntimePackageFormat::Zip
                || release.package_format
                    == buglogin_browser::runtime::catalog::RuntimePackageFormat::Dmg
            {
                let target = download::runtime_directory(root, release)?;
                match fs::symlink_metadata(&target) {
                    Err(missing) if missing.kind() == io::ErrorKind::NotFound => Ok(()),
                    Ok(metadata) if metadata.file_type().is_dir() => fs::remove_dir_all(target)
                        .map_err(|_| {
                            problem(
                                "runtime_install_cleanup_failed",
                                "Failed runtime installation could not be removed",
                            )
                        }),
                    _ => Err(problem(
                        "runtime_install_cleanup_failed",
                        "Failed runtime installation could not be removed",
                    )),
                }
            } else {
                let target = confined_path(root, Path::new(&release.relative_path))?;
                match fs::symlink_metadata(&target) {
                    Err(missing) if missing.kind() == io::ErrorKind::NotFound => Ok(()),
                    Ok(metadata) if metadata.file_type().is_file() => fs::remove_file(target)
                        .map_err(|_| {
                            problem(
                                "runtime_install_cleanup_failed",
                                "Failed runtime installation could not be removed",
                            )
                        }),
                    _ => Err(problem(
                        "runtime_install_cleanup_failed",
                        "Failed runtime installation could not be removed",
                    )),
                }
            }
        }
        Err(error) => Err(error),
    }
}

#[derive(Clone, Serialize)]
struct RuntimeInstallProgress<'a> {
    browser: &'a str,
    version: &'a str,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    percentage: u8,
    speed_bytes_per_sec: u64,
    eta_seconds: Option<u64>,
    stage: &'a str,
}

pub(crate) fn emit_progress(
    app: &tauri::AppHandle,
    runtime: &str,
    version: &str,
    downloaded_bytes: u64,
    total_bytes: u64,
    speed_bytes_per_sec: u64,
    eta_seconds: Option<u64>,
    stage: &str,
) {
    let percentage = if total_bytes == 0 {
        0
    } else {
        ((downloaded_bytes.saturating_mul(100) / total_bytes).min(100)) as u8
    };
    let event = RuntimeInstallProgress {
        browser: runtime,
        version,
        downloaded_bytes,
        total_bytes: Some(total_bytes),
        percentage,
        speed_bytes_per_sec,
        eta_seconds,
        stage,
    };
    let _ = app.emit("download-progress", event);
}

fn runtime_name(runtime: &buglogin_domain::entities::Runtime) -> &'static str {
    match runtime {
        buglogin_domain::entities::Runtime::Wayfern => "wayfern",
        _ => "unsupported",
    }
}

fn runtime_download_request_timeout(deadline: Instant, now: Instant) -> Option<Duration> {
    let remaining = deadline.saturating_duration_since(now);
    (!remaining.is_zero()).then_some(remaining.min(RUNTIME_DOWNLOAD_REQUEST_TIMEOUT))
}

fn runtime_download_retryable_status(status: reqwest::StatusCode) -> bool {
    matches!(status.as_u16(), 500 | 502 | 503 | 504)
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

fn runtime_retry_after(response: &reqwest::blocking::Response) -> Option<Duration> {
    let value = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?;
    value
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
        .or_else(|| {
            httpdate::parse_http_date(value)
                .ok()?
                .duration_since(std::time::SystemTime::now())
                .ok()
        })
}

fn wait_for_runtime_download_retry(
    retries: &mut u8,
    deadline: Instant,
    retry_after: Option<Duration>,
    retry_identity: &str,
) -> bool {
    if *retries >= RUNTIME_DOWNLOAD_MAX_RETRIES {
        return false;
    }
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return false;
    }
    *retries += 1;
    let retry_number = *retries;
    let backoff_ms = 250_u64
        .saturating_mul(1_u64 << u32::from(retry_number.saturating_sub(1)))
        .min(4_000);
    let retry_time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let retry_seed = format!(
        "{retry_identity}\0{retry_number}\0{}\0{retry_time}",
        std::process::id()
    );
    let jitter_ms = buglogin_storage::blob::checksum(retry_seed.as_bytes())
        .as_bytes()
        .get(0..2)
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .and_then(|value| u64::from_str_radix(value, 16).ok())
        .unwrap_or_default()
        % 251;
    let server_delay = retry_after
        .map(|delay| delay.min(Duration::from_secs(5)))
        .unwrap_or_default();
    let delay = server_delay
        .max(Duration::from_millis(backoff_ms))
        .saturating_add(Duration::from_millis(jitter_ms));
    if delay >= remaining {
        std::thread::sleep(remaining);
        return false;
    }
    std::thread::sleep(delay);
    Instant::now() < deadline
}

struct HttpsArtifactSource {
    client: reqwest::blocking::Client,
    app: tauri::AppHandle,
    runtime: String,
    version: String,
}

impl HttpsArtifactSource {
    fn new(app: tauri::AppHandle, runtime: String, version: String) -> Result<Self, Problem> {
        let client = reqwest::blocking::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::limited(10))
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30 * 60))
            .build()
            .map_err(|_| {
                problem(
                    "runtime_download_unavailable",
                    "Runtime download unavailable",
                )
            })?;
        Ok(Self {
            client,
            app,
            runtime,
            version,
        })
    }
}

impl download::RuntimeArtifactSource for HttpsArtifactSource {
    fn fetch_to(
        &self,
        source: &str,
        maximum_bytes: u64,
        output: &mut dyn Write,
    ) -> Result<(), Problem> {
        let url = Url::parse(source)
            .map_err(|_| problem("runtime_source_invalid", "Runtime source URL is invalid"))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(problem(
                "runtime_source_invalid",
                "Runtime source URL is invalid",
            ));
        }
        // Use the signed package size as the progress denominator. CDN
        // Content-Length can describe a truncated response, which previously
        // showed 100% before the installer rejected it and threw away bytes.
        let total_bytes = maximum_bytes;
        let started_at = Instant::now();
        let download_deadline = started_at + RUNTIME_DOWNLOAD_TOTAL_TIMEOUT;
        let mut downloaded_bytes = 0_u64;
        let mut last_update = Instant::now();
        let mut buffer = [0_u8; 64 * 1024];
        let mut retries = 0_u8;
        while downloaded_bytes < maximum_bytes {
            let Some(request_timeout) =
                runtime_download_request_timeout(download_deadline, Instant::now())
            else {
                return Err(problem(
                    "runtime_download_deadline_exceeded",
                    "Browser runtime download exceeded its total time limit",
                ));
            };
            let mut request = self.client.get(url.clone()).timeout(request_timeout);
            if downloaded_bytes > 0 {
                request =
                    request.header(reqwest::header::RANGE, format!("bytes={downloaded_bytes}-"));
            }
            let response = match request.send() {
                Ok(response) => response,
                Err(error)
                    if (error.is_connect() || error.is_timeout())
                        && wait_for_runtime_download_retry(
                            &mut retries,
                            download_deadline,
                            None,
                            &self.version,
                        ) =>
                {
                    continue;
                }
                Err(error) => {
                    if (error.is_connect() || error.is_timeout())
                        && Instant::now() >= download_deadline
                    {
                        return Err(problem(
                            "runtime_download_deadline_exceeded",
                            "Browser runtime download exceeded its total time limit",
                        ));
                    }
                    return Err(problem(
                        "runtime_download_failed",
                        "Runtime download failed",
                    ));
                }
            };
            if runtime_download_retryable_status(response.status()) {
                let retry_after = runtime_retry_after(&response);
                drop(response);
                if wait_for_runtime_download_retry(
                    &mut retries,
                    download_deadline,
                    retry_after,
                    &self.version,
                ) {
                    continue;
                }
                if Instant::now() >= download_deadline {
                    return Err(problem(
                        "runtime_download_deadline_exceeded",
                        "Browser runtime download exceeded its total time limit",
                    ));
                }
                return Err(problem(
                    "runtime_download_failed",
                    "Runtime download failed",
                ));
            }
            if downloaded_bytes == 0 {
                if !response.status().is_success()
                    || response
                        .content_length()
                        .is_some_and(|length| length > maximum_bytes)
                {
                    return Err(problem(
                        "runtime_download_failed",
                        "Runtime download failed",
                    ));
                }
            } else {
                let expected_prefix = format!("bytes {downloaded_bytes}-");
                let valid_range = response.status() == reqwest::StatusCode::PARTIAL_CONTENT
                    && response
                        .headers()
                        .get(reqwest::header::CONTENT_RANGE)
                        .and_then(|value| value.to_str().ok())
                        .is_some_and(|value| value.starts_with(&expected_prefix));
                if !valid_range {
                    return Err(problem(
                        "runtime_download_resume_unsupported",
                        "The browser download was interrupted and the server could not resume it",
                    ));
                }
            }
            let retry_after = runtime_retry_after(&response);
            let mut limited = response.take(maximum_bytes - downloaded_bytes + 1);
            let mut made_progress = false;
            let mut interrupted = false;
            loop {
                let count = match limited.read(&mut buffer) {
                    Ok(count) => count,
                    Err(_) => {
                        interrupted = true;
                        break;
                    }
                };
                if count == 0 {
                    break;
                }
                downloaded_bytes = downloaded_bytes.saturating_add(count as u64);
                if downloaded_bytes > maximum_bytes {
                    return Err(problem(
                        "runtime_size_mismatch",
                        "Runtime package exceeds its signed size",
                    ));
                }
                output
                    .write_all(&buffer[..count])
                    .map_err(|_| problem("runtime_install_failed", "Runtime install failed"))?;
                made_progress = true;
                if last_update.elapsed() >= Duration::from_millis(250) {
                    let elapsed = started_at.elapsed().as_secs_f64().max(0.001);
                    let speed = (downloaded_bytes as f64 / elapsed) as u64;
                    let remaining = total_bytes.saturating_sub(downloaded_bytes);
                    emit_progress(
                        &self.app,
                        &self.runtime,
                        &self.version,
                        downloaded_bytes,
                        total_bytes,
                        speed,
                        (speed > 0).then_some(remaining / speed.max(1)),
                        "downloading",
                    );
                    last_update = Instant::now();
                }
            }
            if downloaded_bytes == maximum_bytes {
                break;
            }
            if Instant::now() >= download_deadline {
                return Err(problem(
                    "runtime_download_deadline_exceeded",
                    "Browser runtime download exceeded its total time limit",
                ));
            }
            if !made_progress && !interrupted {
                return Err(problem(
                    "runtime_download_interrupted",
                    "The browser download was interrupted before completion",
                ));
            }
            if !wait_for_runtime_download_retry(
                &mut retries,
                download_deadline,
                retry_after,
                &self.version,
            ) {
                if Instant::now() >= download_deadline {
                    return Err(problem(
                        "runtime_download_deadline_exceeded",
                        "Browser runtime download exceeded its total time limit",
                    ));
                }
                return Err(problem(
                    "runtime_download_interrupted",
                    "The browser download was interrupted before completion",
                ));
            }
        }
        emit_progress(
            &self.app,
            &self.runtime,
            &self.version,
            downloaded_bytes,
            total_bytes,
            0,
            None,
            "verifying",
        );
        Ok(())
    }

    fn on_stage(&self, stage: &str) {
        emit_progress(
            &self.app,
            &self.runtime,
            &self.version,
            0,
            0,
            0,
            None,
            stage,
        );
    }
}

fn problem(code: &'static str, title: &'static str) -> Problem {
    let status = match code {
        "runtime_download_unavailable"
        | "runtime_install_path_unavailable"
        | "runtime_install_record_unavailable"
        | "runtime_operation_lock_unavailable" => 503,
        "runtime_install_in_progress" => 409,
        "runtime_download_deadline_exceeded" => 504,
        "runtime_source_invalid" | "runtime_download_failed" => 502,
        "runtime_size_mismatch"
        | "runtime_install_record_invalid"
        | "runtime_signature_invalid"
        | "runtime_executable_missing"
        | "runtime_executable_invalid"
        | "runtime_executable_checksum_mismatch" => 422,
        _ => 500,
    };
    Problem::new(code, title, status, "desktop-runtime-install")
}

#[cfg(test)]
mod tests {
    use super::{
        InstalledRuntimeRecord, acquire_runtime_operation_lock, current_platform_architecture,
        load_installed_targets, persist_install_record, runtime_download_request_timeout,
        runtime_download_retryable_status, runtime_operation_gate, runtime_operation_key,
    };
    #[cfg(unix)]
    use super::{install_record_paths, open_runtime_operation_lock_file};
    use crate::adapter::RuntimeTarget;
    use buglogin_browser::runtime::{
        RuntimeBinary,
        catalog::{RuntimeCatalog, RuntimePackageFormat, RuntimeRelease, RuntimeSignatureVerifier},
        download,
    };
    use buglogin_domain::entities::{Runtime, RuntimeArchitecture, RuntimePlatform};
    use buglogin_storage::blob::checksum;
    use std::{
        fs,
        path::PathBuf,
        time::{Duration, Instant},
    };

    struct ChecksumVerifier;

    #[test]
    fn runtime_download_request_timeout_never_exceeds_remaining_total_budget() {
        let start = Instant::now();
        let deadline = start + Duration::from_secs(45 * 60);
        assert_eq!(
            runtime_download_request_timeout(deadline, start),
            Some(Duration::from_secs(30 * 60))
        );
        assert_eq!(
            runtime_download_request_timeout(deadline, deadline - Duration::from_secs(12)),
            Some(Duration::from_secs(12))
        );
        assert_eq!(runtime_download_request_timeout(deadline, deadline), None);
    }

    #[test]
    fn runtime_download_retries_only_transient_http_statuses() {
        assert!(runtime_download_retryable_status(
            reqwest::StatusCode::REQUEST_TIMEOUT
        ));
        assert!(runtime_download_retryable_status(
            reqwest::StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(runtime_download_retryable_status(
            reqwest::StatusCode::BAD_GATEWAY
        ));
        assert!(runtime_download_retryable_status(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(!runtime_download_retryable_status(
            reqwest::StatusCode::NOT_FOUND
        ));
        assert!(!runtime_download_retryable_status(
            reqwest::StatusCode::NOT_IMPLEMENTED
        ));
    }

    #[test]
    fn installed_runtime_inventory_rejects_non_directory_root() {
        let root = root();
        fs::write(&root, b"not a directory").unwrap();

        let error = load_installed_targets(&root, &ChecksumVerifier).unwrap_err();

        assert_eq!(error.code, "runtime_install_path_invalid");
        fs::remove_file(root).unwrap();
    }

    #[test]
    fn runtime_operation_lock_rejects_launch_while_installer_holds_exclusive_guard() {
        let root = root();
        let installer =
            acquire_runtime_operation_lock(&root, "test-key", true, "runtime-install-test")
                .unwrap();
        let launch =
            acquire_runtime_operation_lock(&root, "test-key", false, "runtime-launch-test")
                .unwrap_err();
        assert_eq!(launch.code, "runtime_install_in_progress");
        drop(installer);
        let launch =
            acquire_runtime_operation_lock(&root, "test-key", false, "runtime-launch-test")
                .unwrap();
        drop(launch);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn runtime_operation_lock_rejects_dangling_symlink() {
        use std::os::unix::fs::symlink;

        let root = root();
        fs::create_dir_all(&root).unwrap();
        let lock_path = root.join("runtime.lock");
        let external_target = root.with_extension("outside");
        symlink(&external_target, &lock_path).unwrap();

        assert!(open_runtime_operation_lock_file(&lock_path).is_err());
        assert!(!external_target.exists());

        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn installed_runtime_inventory_rejects_symlinked_host_directory() {
        use std::os::unix::fs::symlink;

        let root = root();
        fs::create_dir_all(root.join("wayfern")).unwrap();
        let external = root.with_extension("runtime-external");
        fs::create_dir_all(&external).unwrap();
        let (platform, architecture) = current_platform_architecture("runtime-path-test").unwrap();
        let host_directory = match &platform {
            RuntimePlatform::Windows => "windows",
            RuntimePlatform::Macos => "macos",
            RuntimePlatform::Linux => "linux",
        };
        symlink(&external, root.join("wayfern").join(host_directory)).unwrap();

        let inventory = install_record_paths(&root, &Runtime::Wayfern, &platform, &architecture);
        assert!(inventory.is_err());

        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(external).unwrap();
    }

    #[tokio::test]
    async fn runtime_operation_gate_rejects_installer_while_launch_holds_read_guard() {
        let launch = runtime_operation_gate("test-key").read_owned().await;
        assert!(
            runtime_operation_gate("test-key")
                .try_write_owned()
                .is_err()
        );
        drop(launch);
        assert!(runtime_operation_gate("test-key").try_write_owned().is_ok());
    }

    #[test]
    fn runtime_operation_keys_are_isolated_by_runtime_version_and_host() {
        let (platform, architecture) = current_platform_architecture("runtime-key-test").unwrap();
        let first = runtime_operation_key(&Runtime::Wayfern, &platform, &architecture, "1");
        let second = runtime_operation_key(&Runtime::Wayfern, &platform, &architecture, "2");
        let other_host = runtime_operation_key(
            &Runtime::Wayfern,
            &RuntimePlatform::Macos,
            &RuntimeArchitecture::Arm64,
            "1",
        );
        assert_ne!(first, second);
        assert_ne!(first, other_host);
        let first_guard = runtime_operation_gate(&first).try_write_owned().unwrap();
        let second_guard = runtime_operation_gate(&second).try_write_owned().unwrap();
        drop((first_guard, second_guard));
    }

    impl RuntimeSignatureVerifier for ChecksumVerifier {
        fn verify(&self, key_id: &str, payload: &[u8], signature: &str) -> bool {
            key_id == "runtime-test-key" && signature == checksum(payload)
        }
    }

    fn release(version: &str, bytes: &[u8]) -> RuntimeRelease {
        let (platform, architecture) =
            current_platform_architecture("runtime-install-test").unwrap();
        let mut release = RuntimeRelease {
            runtime: Runtime::Wayfern,
            version: version.to_owned(),
            platform,
            architecture,
            relative_path: "wayfern.exe".to_owned(),
            package_format: RuntimePackageFormat::Raw,
            archive_prefix: None,
            source: "https://runtime.example/wayfern.exe".to_owned(),
            sha256: checksum(bytes),
            size_bytes: bytes.len() as u64,
            key_id: "runtime-test-key".to_owned(),
            signature: String::new(),
        };
        release.signature = checksum(&release.signing_payload());
        release
    }

    fn root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "buglogin-runtime-install-versions-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn install_record(root: &std::path::Path, release: &RuntimeRelease, bytes: &[u8]) {
        let directory = download::runtime_directory(root, release).unwrap();
        let executable = directory.join(&release.relative_path);
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(&executable, bytes).unwrap();
        let target = RuntimeTarget {
            binary: RuntimeBinary {
                path: executable,
                version: release.version.clone(),
                expected_checksum: checksum(bytes),
            },
            extra_args: Vec::new(),
        };
        persist_install_record(root, 1, release, &target).unwrap();
    }

    #[test]
    fn installed_version_inventory_keeps_multiple_verified_versions_and_legacy_install() {
        let root = root();
        let _ = fs::remove_dir_all(&root);
        let old_bytes = b"old-wayfern-runtime";
        let new_bytes = b"new-wayfern-runtime";
        let old_release = release("154.0.8037.57", old_bytes);
        let new_release = release("155.0.8059.79", new_bytes);
        install_record(&root, &old_release, old_bytes);
        install_record(&root, &new_release, new_bytes);

        // Install records from the previous layout lived at the unversioned platform directory.
        let base = download::runtime_directory_for(
            &root,
            &old_release.runtime,
            &old_release.platform,
            &old_release.architecture,
        )
        .unwrap();
        let legacy_bytes = b"legacy-wayfern-runtime";
        let legacy = release("153.0.7912.91", legacy_bytes);
        let legacy_executable = base.join(&legacy.relative_path);
        fs::create_dir_all(legacy_executable.parent().unwrap()).unwrap();
        fs::write(&legacy_executable, legacy_bytes).unwrap();
        let legacy_record = InstalledRuntimeRecord {
            schema_version: 1,
            catalog_schema_version: 1,
            release: legacy.clone(),
            executable_sha256: checksum(legacy_bytes),
        };
        fs::write(
            base.join(".buglogin-install.json"),
            serde_json::to_vec(&legacy_record).unwrap(),
        )
        .unwrap();

        let targets = load_installed_targets(&root, &ChecksumVerifier).unwrap();
        assert_eq!(targets.versions(&Runtime::Wayfern).len(), 3);
        assert_eq!(targets.versions(&Runtime::Wayfern)[0], "155.0.8059.79");
        for release in [&old_release, &new_release, &legacy] {
            let target = targets
                .target(&Runtime::Wayfern, Some(&release.version))
                .unwrap();
            assert_eq!(target.binary.version, release.version);
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn corrupted_installed_version_does_not_block_verified_runtime_inventory() {
        let root = root();
        let verified_bytes = b"verified-wayfern-runtime";
        let corrupted_bytes = b"corrupted-wayfern-runtime";
        let verified_release = release("155.0.8059.79", verified_bytes);
        let corrupted_release = release("154.0.8037.57", corrupted_bytes);
        install_record(&root, &verified_release, verified_bytes);
        install_record(&root, &corrupted_release, corrupted_bytes);

        let corrupted_executable = download::runtime_directory(&root, &corrupted_release)
            .unwrap()
            .join(&corrupted_release.relative_path);
        fs::write(&corrupted_executable, b"tampered after installation").unwrap();

        let targets = load_installed_targets(&root, &ChecksumVerifier).unwrap();
        assert_eq!(
            targets.versions(&Runtime::Wayfern),
            vec![verified_release.version]
        );
        assert!(
            targets
                .target(&Runtime::Wayfern, Some(&corrupted_release.version))
                .is_none()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn completed_install_is_reused_by_later_concurrent_caller() {
        let root = root();
        let bytes = b"verified-wayfern-runtime";
        let release = release("155.0.8059.79", bytes);
        install_record(&root, &release, bytes);
        let catalog = RuntimeCatalog {
            schema_version: 1,
            releases: vec![release.clone()],
        };
        let verified = catalog.verify(&ChecksumVerifier).unwrap().remove(0);

        let reused = super::existing_install(&root, &verified, 1)
            .unwrap()
            .expect("completed signed runtime install should be reused");

        assert_eq!(reused.binary.version, release.version);
        assert_eq!(reused.binary.expected_checksum, checksum(bytes));
        fs::remove_dir_all(root).unwrap();
    }
}
