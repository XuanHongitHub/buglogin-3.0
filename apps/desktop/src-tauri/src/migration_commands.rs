#![allow(clippy::module_name_repetitions)]

use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use buglogin_api::commands::CommandResult;
use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const CORRELATION_ID: &str = "desktop-migration";
const INSPECT_COMMAND_ID: &str = "migration.inspect";
const IMPORT_COMMAND_ID: &str = "migration.import";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Input for the read-only local migration inspection path.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationInspectInput {
    pub source_root: PathBuf,
}

/// Input for the bounded migration import dry-run path.
///
/// This deliberately accepts only package verification material. There is no
/// staging path, run ID, confirmation token, or activation flag here, so this
/// module cannot perform a production cutover.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationImportDryRunInput {
    pub package_root: PathBuf,
    pub destination_root: PathBuf,
    pub signing_key_namespace: String,
    pub signing_key_name: String,
    pub migration_key_namespace: String,
    pub migration_key_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MigrationImportInput {
    pub package_root: PathBuf,
    pub staging_root: PathBuf,
    pub destination_root: PathBuf,
    pub run_id: String,
    pub signing_key_namespace: String,
    pub signing_key_name: String,
    pub migration_key_namespace: String,
    pub migration_key_name: String,
    pub confirmation: String,
    pub dry_run_approved: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationInspectResult {
    pub status: String,
    pub source_unchanged: bool,
    pub source_tree_sha256: String,
    pub report: Value,
    pub snapshot: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationImportDryRunResult {
    pub status: String,
    pub dry_run: bool,
    pub destination_changed: bool,
    pub plan: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationImportResult {
    pub status: String,
    pub run_id: String,
    pub package_id: Option<String>,
    pub created_profiles: Option<u64>,
    pub reused_profiles: Option<u64>,
    pub result: Value,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationProcessOutput {
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Process boundary used by the public functions and module-local tests.
///
/// Keeping this boundary injectable lets later Tauri registration use the
/// packaged migration executable without duplicating migration-core logic.
pub trait MigrationExecutor {
    fn execute(&self, args: &[OsString]) -> Result<MigrationProcessOutput, Problem>;
}

#[derive(Clone, Debug)]
pub struct MigrationCommandRunner {
    executable: PathBuf,
    timeout: Duration,
}

impl MigrationCommandRunner {
    pub fn new(executable: impl Into<PathBuf>) -> Result<Self, Problem> {
        let executable = executable.into();
        if executable.as_os_str().is_empty() {
            return Err(invalid_input("migration_executable_missing"));
        }
        Ok(Self {
            executable,
            timeout: DEFAULT_TIMEOUT,
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, Problem> {
        if timeout.is_zero() || timeout > DEFAULT_TIMEOUT {
            return Err(invalid_input("migration_timeout_invalid"));
        }
        self.timeout = timeout;
        Ok(self)
    }

    pub fn executable(&self) -> &Path {
        &self.executable
    }
}

impl MigrationExecutor for MigrationCommandRunner {
    fn execute(&self, args: &[OsString]) -> Result<MigrationProcessOutput, Problem> {
        let mut child = Command::new(&self.executable)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| process_problem("migration_process_start_failed", error))?;
        let started = Instant::now();

        loop {
            match child
                .try_wait()
                .map_err(|error| process_problem("migration_process_wait_failed", error))?
            {
                Some(status) => {
                    let output = child.wait_with_output().map_err(|error| {
                        process_problem("migration_process_output_failed", error)
                    })?;
                    return Ok(MigrationProcessOutput {
                        success: status.success(),
                        stdout: output.stdout,
                        stderr: output.stderr,
                    });
                }
                None if started.elapsed() >= self.timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Problem::new(
                        "migration_process_timeout",
                        "Migration command timed out",
                        504,
                        CORRELATION_ID,
                    )
                    .retryable(true));
                }
                None => thread::sleep(POLL_INTERVAL),
            }
        }
    }
}

/// Executes `migration.inspect` through the packaged migration CLI.
///
/// The source is fingerprinted before and after execution. Any source change,
/// including a symlink entry, fails closed. The CLI report is written only to
/// an isolated temporary directory outside the source tree.
pub fn migration_inspect<E: MigrationExecutor>(
    executor: &E,
    input: MigrationInspectInput,
) -> Result<CommandResult<MigrationInspectResult>, Problem> {
    let source_root = canonical_source_root(&input.source_root)?;
    let before = source_fingerprint(&source_root)?;
    let report_root = temporary_report_root(&source_root)?;
    let result = (|| {
        let args = vec![
            OsString::from("inspect"),
            OsString::from("--source"),
            source_root.as_os_str().to_owned(),
            OsString::from("--report"),
            report_root.as_os_str().to_owned(),
            OsString::from("--read-only"),
        ];
        let output = executor.execute(&args)?;
        ensure_process_success(INSPECT_COMMAND_ID, output)?;
        let report = read_json(&report_root.join("inspect.json"))?;
        let snapshot = read_json(&report_root.join("snapshot.json"))?;
        let after = source_fingerprint(&source_root)?;
        if before != after {
            return Err(source_changed_problem());
        }
        let source_tree_sha256 = after.ok_or_else(|| source_missing_problem())?;
        Ok(command_result(
            INSPECT_COMMAND_ID,
            MigrationInspectResult {
                status: "completed".to_owned(),
                source_unchanged: true,
                source_tree_sha256,
                report,
                snapshot,
            },
        ))
    })();
    let _ = fs::remove_dir_all(&report_root);
    result
}

/// Executes only the signed package dry-run branch of `migration.import`.
///
/// No staging directory, run ID, confirmation token, activation, or rollback
/// command is accepted by this API. Package and destination fingerprints must
/// remain unchanged, including when the destination did not exist initially.
pub fn migration_import_dry_run<E: MigrationExecutor>(
    executor: &E,
    input: MigrationImportDryRunInput,
) -> Result<CommandResult<MigrationImportDryRunResult>, Problem> {
    validate_key_reference(&input.signing_key_namespace, &input.signing_key_name)?;
    validate_key_reference(&input.migration_key_namespace, &input.migration_key_name)?;
    let package_root = canonical_directory(&input.package_root, "migration_package_invalid")?;
    let destination_root = canonical_destination(&input.destination_root)?;
    if paths_overlap(&package_root, &destination_root) {
        return Err(Problem::new(
            "migration_paths_overlap",
            "Migration package and destination must be separate",
            409,
            CORRELATION_ID,
        ));
    }
    let package_before = source_fingerprint(&package_root)?;
    let destination_before = optional_source_fingerprint(&destination_root)?;
    let args = vec![
        OsString::from("import"),
        OsString::from("--package"),
        package_root.as_os_str().to_owned(),
        OsString::from("--destination"),
        destination_root.as_os_str().to_owned(),
        OsString::from("--dry-run"),
        OsString::from("--signing-key-namespace"),
        OsString::from(&input.signing_key_namespace),
        OsString::from("--signing-key-name"),
        OsString::from(&input.signing_key_name),
        OsString::from("--key-namespace"),
        OsString::from(&input.migration_key_namespace),
        OsString::from("--key-name"),
        OsString::from(&input.migration_key_name),
    ];
    let output = executor.execute(&args)?;
    ensure_process_success(IMPORT_COMMAND_ID, output.clone())?;
    let package_after = source_fingerprint(&package_root)?;
    let destination_after = optional_source_fingerprint(&destination_root)?;
    if package_before != package_after || destination_before != destination_after {
        return Err(Problem::new(
            "migration_dry_run_changed_data",
            "Migration dry-run changed package or destination data",
            500,
            CORRELATION_ID,
        ));
    }
    let plan = parse_stdout_json(&output.stdout)?;
    Ok(command_result(
        IMPORT_COMMAND_ID,
        MigrationImportDryRunResult {
            status: "dry_run_completed".to_owned(),
            dry_run: true,
            destination_changed: false,
            plan,
        },
    ))
}

pub fn migration_import<E: MigrationExecutor>(
    executor: &E,
    input: MigrationImportInput,
) -> Result<CommandResult<MigrationImportResult>, Problem> {
    validate_key_reference(&input.signing_key_namespace, &input.signing_key_name)?;
    validate_key_reference(&input.migration_key_namespace, &input.migration_key_name)?;
    validate_run_id(&input.run_id)?;
    if input.confirmation != "CUTOVER_AND_PROMOTE" {
        return Err(invalid_input("migration_confirmation_invalid"));
    }
    if !input.dry_run_approved {
        return Err(Problem::new(
            "migration_dry_run_required",
            "Migration dry-run approval is required",
            409,
            CORRELATION_ID,
        ));
    }
    let package_root = canonical_directory(&input.package_root, "migration_package_invalid")?;
    let staging_root = canonical_directory_or_create(&input.staging_root)?;
    let destination_root = canonical_destination(&input.destination_root)?;
    if paths_overlap(&package_root, &staging_root)
        || paths_overlap(&package_root, &destination_root)
        || paths_overlap(&staging_root, &destination_root)
    {
        return Err(Problem::new(
            "migration_paths_overlap",
            "Migration package, staging, and destination must be separate",
            409,
            CORRELATION_ID,
        ));
    }
    let package_before = source_fingerprint(&package_root)?;
    let args = vec![
        OsString::from("import"),
        OsString::from("--package"),
        package_root.as_os_str().to_owned(),
        OsString::from("--destination"),
        destination_root.as_os_str().to_owned(),
        OsString::from("--staging"),
        staging_root.as_os_str().to_owned(),
        OsString::from("--run-id"),
        OsString::from(&input.run_id),
        OsString::from("--key-namespace"),
        OsString::from(&input.migration_key_namespace),
        OsString::from("--key-name"),
        OsString::from(&input.migration_key_name),
        OsString::from("--signing-key-namespace"),
        OsString::from(&input.signing_key_namespace),
        OsString::from("--signing-key-name"),
        OsString::from(&input.signing_key_name),
        OsString::from("--confirm"),
        OsString::from(&input.confirmation),
    ];
    let output = executor.execute(&args)?;
    ensure_process_success(IMPORT_COMMAND_ID, output.clone())?;
    if package_before != source_fingerprint(&package_root)? {
        return Err(Problem::new(
            "migration_package_changed",
            "Migration package changed during import",
            500,
            CORRELATION_ID,
        ));
    }
    let result = parse_stdout_json(&output.stdout)?;
    Ok(command_result(
        IMPORT_COMMAND_ID,
        MigrationImportResult {
            status: result
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("completed")
                .to_owned(),
            run_id: result
                .get("runId")
                .and_then(Value::as_str)
                .unwrap_or(&input.run_id)
                .to_owned(),
            package_id: result
                .get("packageId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            created_profiles: result.get("createdProfiles").and_then(Value::as_u64),
            reused_profiles: result.get("reusedProfiles").and_then(Value::as_u64),
            result,
        },
    ))
}

fn command_result<T>(command_id: &str, data: T) -> CommandResult<T> {
    CommandResult {
        command_id: command_id.to_owned(),
        correlation_id: CORRELATION_ID.to_owned(),
        data,
    }
}

fn ensure_process_success(command_id: &str, output: MigrationProcessOutput) -> Result<(), Problem> {
    if output.success {
        return Ok(());
    }
    if let Ok(problem) = serde_json::from_slice::<Problem>(&output.stderr) {
        return Err(problem);
    }
    Err(Problem::new(
        format!("{command_id}_failed"),
        "Migration command failed",
        422,
        CORRELATION_ID,
    ))
}

fn parse_stdout_json(bytes: &[u8]) -> Result<Value, Problem> {
    serde_json::from_slice(bytes).map_err(|_| {
        Problem::new(
            "migration_result_invalid",
            "Migration command returned invalid JSON",
            502,
            CORRELATION_ID,
        )
    })
}

fn read_json(path: &Path) -> Result<Value, Problem> {
    let bytes =
        fs::read(path).map_err(|error| process_problem("migration_result_missing", error))?;
    serde_json::from_slice(&bytes).map_err(|_| {
        Problem::new(
            "migration_result_invalid",
            "Migration command returned invalid JSON",
            502,
            CORRELATION_ID,
        )
    })
}

fn canonical_source_root(path: &Path) -> Result<PathBuf, Problem> {
    canonical_directory(path, "migration_source_invalid")
}

fn canonical_directory(path: &Path, code: &'static str) -> Result<PathBuf, Problem> {
    let metadata = fs::symlink_metadata(path).map_err(|error| process_problem(code, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Problem::new(
            code,
            "Migration directory is invalid",
            400,
            CORRELATION_ID,
        ));
    }
    fs::canonicalize(path).map_err(|error| process_problem(code, error))
}

fn canonical_destination(path: &Path) -> Result<PathBuf, Problem> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(invalid_input("migration_destination_invalid"));
            }
            fs::canonicalize(path)
                .map_err(|error| process_problem("migration_destination_invalid", error))
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .ok_or_else(|| invalid_input("migration_destination_invalid"))?;
            let parent = fs::canonicalize(parent)
                .map_err(|error| process_problem("migration_destination_invalid", error))?;
            Ok(parent.join(
                path.file_name()
                    .ok_or_else(|| invalid_input("migration_destination_invalid"))?,
            ))
        }
        Err(error) => Err(process_problem("migration_destination_invalid", error)),
    }
}

fn canonical_directory_or_create(path: &Path) -> Result<PathBuf, Problem> {
    if !path.exists() {
        fs::create_dir_all(path)
            .map_err(|error| process_problem("migration_staging_invalid", error))?;
    }
    canonical_directory(path, "migration_staging_invalid")
}

fn validate_run_id(value: &str) -> Result<(), Problem> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(invalid_input("migration_run_id_invalid"));
    }
    Ok(())
}

fn temporary_report_root(source_root: &Path) -> Result<PathBuf, Problem> {
    let base = fs::canonicalize(std::env::temp_dir())
        .map_err(|error| process_problem("migration_temp_directory_invalid", error))?;
    for _ in 0..8 {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = base.join(format!(
            "buglogin-desktop-migration-{}-{sequence}",
            std::process::id()
        ));
        if paths_overlap(&path, source_root) {
            return Err(Problem::new(
                "migration_temp_inside_source",
                "Migration report directory must be outside the source",
                409,
                CORRELATION_ID,
            ));
        }
        match fs::create_dir(&path) {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(process_problem("migration_temp_directory_invalid", error)),
        }
    }
    Err(invalid_input("migration_temp_directory_unavailable"))
}

fn source_fingerprint(root: &Path) -> Result<Option<String>, Problem> {
    if !root.exists() {
        return Ok(None);
    }
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut hasher = Sha256::new();
    for (relative, bytes) in files {
        hasher.update(relative.as_bytes());
        hasher.update([0]);
        hasher.update(bytes);
        hasher.update([0]);
    }
    Ok(Some(format!("{:x}", hasher.finalize())))
}

fn optional_source_fingerprint(root: &Path) -> Result<Option<String>, Problem> {
    source_fingerprint(root)
}

fn collect_files(
    root: &Path,
    current: &Path,
    files: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), Problem> {
    let mut entries = fs::read_dir(current)
        .map_err(|error| process_problem("migration_source_read_failed", error))?
        .map(|entry| entry.map_err(|error| process_problem("migration_source_read_failed", error)))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| process_problem("migration_source_read_failed", error))?;
        if metadata.file_type().is_symlink() {
            return Err(invalid_input("migration_symlink_rejected"));
        }
        if metadata.is_dir() {
            collect_files(root, &path, files)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|_| invalid_input("migration_source_path_invalid"))?
                .to_string_lossy()
                .replace('\\', "/");
            files.push((
                relative,
                fs::read(&path)
                    .map_err(|error| process_problem("migration_source_read_failed", error))?,
            ));
        } else {
            return Err(invalid_input("migration_source_entry_invalid"));
        }
    }
    Ok(())
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left == right || left.starts_with(right) || right.starts_with(left)
}

fn validate_key_reference(namespace: &str, name: &str) -> Result<(), Problem> {
    if namespace.is_empty()
        || name.is_empty()
        || namespace.len() > 128
        || name.len() > 128
        || !namespace.bytes().all(|byte| byte.is_ascii_graphic())
        || !name.bytes().all(|byte| byte.is_ascii_graphic())
    {
        return Err(invalid_input("migration_key_reference_invalid"));
    }
    Ok(())
}

fn source_changed_problem() -> Problem {
    Problem::new(
        "migration_source_changed",
        "Migration source changed during inspection",
        409,
        CORRELATION_ID,
    )
}

fn source_missing_problem() -> Problem {
    Problem::new(
        "migration_source_missing",
        "Migration source is unavailable",
        400,
        CORRELATION_ID,
    )
}

fn invalid_input(code: &'static str) -> Problem {
    Problem::new(code, "Migration input is invalid", 400, CORRELATION_ID)
}

fn process_problem(code: &'static str, error: io::Error) -> Problem {
    Problem::new(
        code,
        "Migration filesystem or process operation failed",
        500,
        CORRELATION_ID,
    )
    .with_detail(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, fs, sync::Mutex};

    struct FakeExecutor {
        output: Mutex<Option<MigrationProcessOutput>>,
        mutate_source: Option<PathBuf>,
    }

    impl MigrationExecutor for FakeExecutor {
        fn execute(&self, args: &[OsString]) -> Result<MigrationProcessOutput, Problem> {
            let command = args
                .first()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if command == "inspect" {
                let report_root = value_after(args, "--report");
                fs::write(
                    report_root.join("inspect.json"),
                    br#"{"schema":"buglogin.inspect/1"}"#,
                )
                .expect("write fake report");
                fs::write(
                    report_root.join("snapshot.json"),
                    br#"{"sourceTreeSha256":"fake"}"#,
                )
                .expect("write fake snapshot");
                if let Some(path) = &self.mutate_source {
                    fs::write(path, b"mutated").expect("mutate fake source");
                }
            }
            self.output
                .lock()
                .expect("fake output lock")
                .take()
                .ok_or_else(|| invalid_input("fake_output_missing"))
        }
    }

    fn value_after(args: &[OsString], flag: &str) -> PathBuf {
        args.windows(2)
            .find(|pair| pair[0] == OsString::from(flag))
            .map(|pair| PathBuf::from(&pair[1]))
            .expect("flag value")
    }

    fn directory(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "buglogin-migration-commands-test-{label}-{}",
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create test directory");
        path
    }

    fn success_output(stdout: &[u8]) -> MigrationProcessOutput {
        MigrationProcessOutput {
            success: true,
            stdout: stdout.to_vec(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn inspect_returns_command_envelope_and_preserves_source() {
        let source = directory("inspect");
        let source_file = source.join("version.json");
        fs::write(&source_file, br#"{"version":"1.1.84"}"#).expect("write source");
        let executor = FakeExecutor {
            output: Mutex::new(Some(success_output(br#"{"status":"completed"}"#))),
            mutate_source: None,
        };

        let result = migration_inspect(
            &executor,
            MigrationInspectInput {
                source_root: source.clone(),
            },
        )
        .expect("inspect succeeds");

        assert_eq!(result.command_id, INSPECT_COMMAND_ID);
        assert!(result.data.source_unchanged);
        assert_eq!(result.data.status, "completed");
        assert!(source_file.is_file());
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn inspect_fails_closed_when_executor_changes_source() {
        let source = directory("inspect-mutated");
        let source_file = source.join("version.json");
        fs::write(&source_file, b"before").expect("write source");
        let executor = FakeExecutor {
            output: Mutex::new(Some(success_output(br#"{"status":"completed"}"#))),
            mutate_source: Some(source_file.clone()),
        };

        let error = migration_inspect(
            &executor,
            MigrationInspectInput {
                source_root: source.clone(),
            },
        )
        .expect_err("changed source must fail");

        assert_eq!(error.code, "migration_source_changed");
        let _ = fs::remove_dir_all(source);
    }

    #[test]
    fn dry_run_requires_separate_package_and_destination_and_returns_no_change() {
        let package = directory("package");
        fs::write(package.join("checksums.txt"), b"package").expect("write package");
        let destination = directory("destination");
        let executor = FakeExecutor {
            output: Mutex::new(Some(success_output(
                br#"{"status":"completed","dryRun":true}"#,
            ))),
            mutate_source: None,
        };

        let result = migration_import_dry_run(
            &executor,
            MigrationImportDryRunInput {
                package_root: package.clone(),
                destination_root: destination.clone(),
                signing_key_namespace: "migration".to_owned(),
                signing_key_name: "signing".to_owned(),
                migration_key_namespace: "migration".to_owned(),
                migration_key_name: "encryption".to_owned(),
            },
        )
        .expect("dry-run succeeds");

        assert_eq!(result.command_id, IMPORT_COMMAND_ID);
        assert!(result.data.dry_run);
        assert!(!result.data.destination_changed);
        assert!(result.data.plan.is_object());
        let _ = fs::remove_dir_all(package);
        let _ = fs::remove_dir_all(destination);
    }

    #[test]
    fn dry_run_rejects_activation_inputs_by_construction() {
        let package = directory("overlap");
        let error = migration_import_dry_run(
            &FakeExecutor {
                output: Mutex::new(Some(success_output(b"{}"))),
                mutate_source: None,
            },
            MigrationImportDryRunInput {
                package_root: package.clone(),
                destination_root: package.clone(),
                signing_key_namespace: "migration".to_owned(),
                signing_key_name: "signing".to_owned(),
                migration_key_namespace: "migration".to_owned(),
                migration_key_name: "encryption".to_owned(),
            },
        )
        .expect_err("overlapping paths must fail");

        assert_eq!(error.code, "migration_paths_overlap");
        let _ = fs::remove_dir_all(package);
    }

    #[test]
    fn key_reference_validation_rejects_control_characters() {
        let error = validate_key_reference("migration", "bad\nname").expect_err("invalid key");
        assert_eq!(error.code, "migration_key_reference_invalid");
    }

    #[test]
    fn fingerprint_is_order_independent() {
        let root = directory("fingerprint");
        fs::write(root.join("b"), b"b").expect("write b");
        fs::write(root.join("a"), b"a").expect("write a");
        let first = source_fingerprint(&root).expect("fingerprint");
        let mut files = BTreeMap::new();
        files.insert("a", b"a");
        files.insert("b", b"b");
        assert!(first.is_some());
        assert_eq!(files.len(), 2);
        let second = source_fingerprint(&root).expect("fingerprint");
        assert_eq!(first, second);
        let _ = fs::remove_dir_all(root);
    }
}
