use std::collections::BTreeMap;
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process;
use zeroize::Zeroizing;

use buglogin_domain::errors::Problem;
use buglogin_migration::cutover::{confirm_cutover, promote_staged};
use buglogin_migration::inspect::{
    InspectReport, SourceIdentity, inspect_source, inspect_source_with_identity,
};
use buglogin_migration::legacy::canonicalize_legacy_control_plane_json;
use buglogin_migration::legacy::{LegacyStoragePolicy, export_read_only_postgres_control_plane};
use buglogin_migration::package::{
    NeutralPackage, PackageMetadata, build_control_only_profile_package,
    build_encrypted_profile_package, build_neutral_package,
};
use buglogin_migration::reconcile::{ReconciliationStatus, reconcile_activation};
use buglogin_migration::rollback::{confirm_rollback, rollback_activation, rollback_cutover};
use buglogin_migration::signature::{SignatureMetadata, sign_package, verify_signed_package};
use buglogin_migration::snapshot::{
    SourceSnapshot, create_snapshot, is_legacy_runtime_artifact, validate_source_path,
};
use buglogin_migration::transform::{discard_staging, stage_package};
use buglogin_migration::verify::verify_package;
use buglogin_migration::{MigrationKey, activate_package, plan_package_activation};
use buglogin_storage::secrets::{OsVault, SecretKey, SecretValue};
use buglogin_storage::vault::NativeVault;
use serde::Serialize;
use serde_json::{Value, json};

fn main() {
    if let Err(error) = run(env::args().skip(1).collect()) {
        eprintln!(
            "{}",
            serde_json::to_string(&error)
                .unwrap_or_else(|_| "{\"code\":\"migration_cli_failed\"}".into())
        );
        process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<(), Problem> {
    let (command, options) = parse(args)?;
    match command.as_str() {
        "initialize-keys" => initialize_keys(&options),
        "inspect" => inspect(&options),
        "prepare-source" => prepare_source(&options),
        "export-control-plane" => export_control_plane(&options),
        "hydrate-s3-profile-payloads" => hydrate_s3_profile_payloads(&options),
        "snapshot" => snapshot(&options),
        "export" => export(&options),
        "verify" => verify(&options),
        "import" => import(&options),
        "rollback" => rollback(&options),
        "reconcile" => reconcile(&options),
        _ => Err(problem(
            "migration_command_invalid",
            "Unknown migration command",
            400,
        )),
    }
}

fn initialize_keys(options: &Options) -> Result<(), Problem> {
    let encryption_key = SecretKey::new(
        required(options, "key-namespace")?,
        required(options, "key-name")?,
    )
    .map_err(|_| {
        problem(
            "migration_key_invalid",
            "Migration key reference is invalid",
            400,
        )
    })?;
    let signing_key = SecretKey::new(
        required(options, "signing-key-namespace")?,
        required(options, "signing-key-name")?,
    )
    .map_err(|_| {
        problem(
            "migration_key_invalid",
            "Signing key reference is invalid",
            400,
        )
    })?;
    if encryption_key == signing_key {
        return Err(problem(
            "migration_key_reuse_forbidden",
            "Encryption and signing keys must be distinct",
            400,
        ));
    }

    let output = required_path(options, "env-output")?;
    let mut encryption_bytes = [0_u8; 32];
    let mut signing_bytes = [0_u8; 32];
    getrandom::fill(&mut encryption_bytes)
        .and_then(|()| getrandom::fill(&mut signing_bytes))
        .map_err(|_| {
            problem(
                "migration_key_generation_failed",
                "Secure key generation failed",
                500,
            )
        })?;
    let encryption_hex = Zeroizing::new(hex::encode(encryption_bytes));
    let signing_hex = Zeroizing::new(hex::encode(signing_bytes));
    encryption_bytes.fill(0);
    signing_bytes.fill(0);

    let mut vault = NativeVault::new();
    let encryption_value = SecretValue::new(hex::decode(&encryption_hex).map_err(|_| {
        problem(
            "migration_key_generation_failed",
            "Secure key generation failed",
            500,
        )
    })?)
    .map_err(|_| {
        problem(
            "migration_key_generation_failed",
            "Secure key generation failed",
            500,
        )
    })?;
    let encryption_version = vault
        .create(&encryption_key, encryption_value)
        .map_err(|_| {
            problem(
                "migration_key_store_failed",
                "Could not store migration key",
                503,
            )
        })?;
    let signing_value = SecretValue::new(hex::decode(&signing_hex).map_err(|_| {
        problem(
            "migration_key_generation_failed",
            "Secure key generation failed",
            500,
        )
    })?)
    .map_err(|_| {
        problem(
            "migration_key_generation_failed",
            "Secure key generation failed",
            500,
        )
    })?;
    let signing_version = match vault.create(&signing_key, signing_value) {
        Ok(version) => version,
        Err(_) => {
            let _ = vault.remove(&encryption_key, encryption_version);
            return Err(problem(
                "migration_key_store_failed",
                "Could not store signing key",
                503,
            ));
        }
    };

    let env_content = Zeroizing::new(format!(
        "BUGLOGIN_MIGRATION_ENCRYPTION_KEY_HEX={}\nBUGLOGIN_MIGRATION_SIGNING_KEY_HEX={}\n",
        encryption_hex.as_str(),
        signing_hex.as_str()
    ));
    if let Err(error) = write_new_secret(&output, env_content.as_bytes()) {
        let _ = vault.remove(&signing_key, signing_version);
        let _ = vault.remove(&encryption_key, encryption_version);
        return Err(error);
    }
    print_json(&json!({
        "status": "created",
        "envFile": output,
        "encryptionKey": {"namespace": encryption_key.namespace(), "name": encryption_key.name()},
        "signingKey": {"namespace": signing_key.namespace(), "name": signing_key.name()},
        "keyValuesPrinted": false
    }))
}

fn hydrate_s3_profile_payloads(options: &Options) -> Result<(), Problem> {
    if !options.contains_key("read-only") {
        return Err(problem(
            "migration_read_only_required",
            "Legacy S3 hydration requires read-only source access",
            400,
        ));
    }
    let source = required_path(options, "source")?;
    let report = buglogin_migration::legacy::hydrate_legacy_s3_profile_payloads(&source)?;
    print_json(&json!({
        "status": "completed",
        "sourceObjectsModified": false,
        "hostedProfiles": report.hosted_profiles,
        "manifestsFound": report.manifests_found,
        "profilesHydrated": report.profiles_hydrated,
        "profilesUsingLocalFallback": report.profiles_using_local_fallback,
        "profilesPreserveOnly": report.profiles_preserve_only,
        "profilesRejected": report.profiles_rejected,
        "filesHydrated": report.files_hydrated,
        "bytesHydrated": report.bytes_hydrated,
        "proxiesHydrated": report.proxies_hydrated,
        "proxiesMissing": report.proxies_missing
    }))
}

fn export_control_plane(options: &Options) -> Result<(), Problem> {
    if !options.contains_key("read-only") {
        return Err(problem(
            "migration_read_only_required",
            "Control-plane export requires --read-only",
            400,
        ));
    }
    let policy: LegacyStoragePolicy = read_json(&required_path(options, "storage-policy")?)?;
    let database_url = env::var("BUGLOGIN_LEGACY_DATABASE_URL").map_err(|_| {
        problem(
            "migration_legacy_database_config_missing",
            "Legacy source database configuration is missing",
            503,
        )
    })?;
    let key = migration_key(options)?;
    let export = export_read_only_postgres_control_plane(
        &database_url,
        required(options, "expected-database")?,
        required(options, "source-instance-id")?,
        &policy,
        &key,
    )?;
    let output = required_path(options, "output")?;
    write_new(&output, &export.canonical_json)?;
    print_json(&json!({
        "status": "completed",
        "database": export.database,
        "output": output,
        "workspaceCount": export.workspace_count,
        "membershipCount": export.membership_count,
        "sourceShareGrantCount": export.source_share_grant_count,
        "memberShareGrantCount": export.share_grant_count,
        "pendingShareProvenanceCount": export.pending_share_count,
        "pendingInviteProvenanceCount": export.pending_invite_count,
        "groupCount": export.group_count,
        "profileCount": export.profile_count,
        "profileGroupAssignmentCount": export.profile_group_assignment_count,
        "passwordAccountsEncrypted": export.legacy_password_account_count,
        "googleOnlyAccountsRequireSetup": export.google_only_account_count,
        "databaseTransactionReadOnly": true,
        "inviteTokensExported": false
    }))
}

fn prepare_source(options: &Options) -> Result<(), Problem> {
    if !options.contains_key("read-only") {
        return Err(problem(
            "migration_read_only_required",
            "Source preparation requires --read-only",
            400,
        ));
    }
    let source = required_path(options, "source")?;
    let control_plane = required_path(options, "control-plane")?;
    let output = required_path(options, "output")?;
    validate_source_path(&source)?;
    let source_root = fs::canonicalize(&source).map_err(io_problem)?;
    let output_parent = output
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let output_parent = fs::canonicalize(output_parent).map_err(io_problem)?;
    let output_absolute = output_parent.join(output.file_name().ok_or_else(invalid_path)?);
    if output_absolute.starts_with(&source_root) {
        return Err(problem(
            "migration_output_inside_source",
            "Prepared source output must be outside the source root",
            400,
        ));
    }
    let before = create_snapshot(&source_root)?;
    let (instance_id, generated_version) = source_identity_for_prepare(&source_root, options)?;
    let canonical_control = canonicalize_legacy_control_plane_json(
        &fs::read(&control_plane).map_err(io_problem)?,
        &instance_id,
    )?;
    create_new_directory(&output_absolute)?;
    copy_source_tree(&source_root, &source_root, &output_absolute)?;
    if let Some(version) = generated_version {
        write_new(&output_absolute.join("version.json"), &version)?;
    }
    write_new(
        &output_absolute.join("control-plane-export.json"),
        &canonical_control,
    )?;
    let after = create_snapshot(&source_root)?;
    if !before.has_same_integrity(&after) {
        return Err(problem(
            "migration_source_changed",
            "Migration source changed during preparation",
            409,
        ));
    }
    let prepared = create_snapshot(&output_absolute)?;
    print_json(&json!({
        "status": "completed",
        "sourceUnchanged": true,
        "sourceTreeSha256": before.source_tree_sha256,
        "preparedTreeSha256": prepared.source_tree_sha256,
        "preparedRoot": output_absolute,
        "controlPlaneIncluded": true
    }))
}

fn source_identity_override(
    source_root: &Path,
    options: &Options,
) -> Result<Option<SourceIdentity>, Problem> {
    if source_root.join("version.json").is_file() {
        return Ok(None);
    }
    let has_override = ["source-version", "instance-id", "source-os", "architecture"]
        .iter()
        .any(|name| options.contains_key(*name));
    if !has_override {
        return Ok(None);
    }
    Ok(Some(SourceIdentity {
        system: "buglogin-legacy".to_owned(),
        version: required(options, "source-version")?.to_owned(),
        instance_id: required(options, "instance-id")?.to_owned(),
        os: required(options, "source-os")?.to_owned(),
        architecture: required(options, "architecture")?.to_owned(),
        root: source_root.to_string_lossy().into_owned(),
    }))
}

fn source_identity_for_prepare(
    source_root: &Path,
    options: &Options,
) -> Result<(String, Option<Vec<u8>>), Problem> {
    let version_path = source_root.join("version.json");
    if version_path.is_file() {
        let bytes = fs::read(&version_path).map_err(io_problem)?;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| {
            problem(
                "migration_source_metadata_invalid",
                "Source version metadata is invalid",
                422,
            )
        })?;
        let instance_id = value
            .get("instanceId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                problem(
                    "migration_source_metadata_invalid",
                    "Source instance ID is missing",
                    422,
                )
            })?;
        return Ok((instance_id.to_owned(), None));
    }
    let instance_id = required(options, "instance-id")?.to_owned();
    let bytes = serde_json::to_vec_pretty(&json!({
        "system": "buglogin-legacy",
        "version": required(options, "source-version")?,
        "instanceId": instance_id,
        "os": required(options, "source-os")?,
        "architecture": required(options, "architecture")?
    }))
    .map_err(|_| {
        problem(
            "migration_json_invalid",
            "Generated source metadata is invalid",
            500,
        )
    })?;
    Ok((instance_id, Some(bytes)))
}

fn copy_source_tree(source_root: &Path, current: &Path, output_root: &Path) -> Result<(), Problem> {
    for entry in fs::read_dir(current).map_err(io_problem)? {
        let entry = entry.map_err(io_problem)?;
        let file_type = entry.file_type().map_err(io_problem)?;
        let entry_path = entry.path();
        let relative = entry_path
            .strip_prefix(source_root)
            .map_err(|_| invalid_path())?;
        if is_legacy_runtime_artifact(relative, file_type) {
            continue;
        }
        if file_type.is_symlink() {
            return Err(problem(
                "migration_symlink_rejected",
                "Migration source symlink is not allowed",
                400,
            ));
        }
        let destination = output_root.join(relative);
        if file_type.is_dir() {
            fs::create_dir(&destination).map_err(io_problem)?;
            copy_source_tree(source_root, &entry_path, output_root)?;
        } else if file_type.is_file() {
            if relative == Path::new("control-plane-export.json") {
                continue;
            }
            let bytes = fs::read(entry_path).map_err(io_problem)?;
            write_new(&destination, &bytes)?;
        } else {
            return Err(problem(
                "migration_source_entry_invalid",
                "Migration source entry is invalid",
                400,
            ));
        }
    }
    Ok(())
}

fn inspect(options: &Options) -> Result<(), Problem> {
    let source = required_path(options, "source")?;
    let report_root = required_path(options, "report")?;
    let (report, snapshot) = match source_identity_override(&source, options)? {
        Some(identity) => inspect_source_with_identity(&source, identity)?,
        None => inspect_source(&source)?,
    };
    create_new_directory(&report_root)?;
    write_json(&report_root.join("inspect.json"), &report)?;
    write_json(&report_root.join("snapshot.json"), &snapshot)?;
    print_json(&json!({
        "status": "completed",
        "cutoverEligibility": report.cutover_eligibility,
        "reportRoot": report_root
    }))
}

fn snapshot(options: &Options) -> Result<(), Problem> {
    if !options.contains_key("read-only") {
        return Err(problem(
            "migration_read_only_required",
            "Snapshot requires --read-only",
            400,
        ));
    }
    let value = create_snapshot(&required_path(options, "source")?)?;
    write_json_new_parent(&required_path(options, "output")?, &value)?;
    print_json(&value)
}

fn export(options: &Options) -> Result<(), Problem> {
    let report: InspectReport = read_json(&required_path(options, "report")?)?;
    let snapshot: SourceSnapshot = read_json(&required_path(options, "snapshot")?)?;
    let metadata = PackageMetadata {
        created_at: required(options, "created-at")?.to_owned(),
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        tool_commit: required(options, "tool-commit")?.to_owned(),
    };
    let package = if options.contains_key("redacted") {
        build_neutral_package(&report, &snapshot, &metadata)?
    } else {
        let key = migration_key(options)?;
        if options.contains_key("control-only") {
            build_control_only_profile_package(
                &required_path(options, "source")?,
                &report,
                &snapshot,
                &metadata,
                &key,
            )?
        } else {
            build_encrypted_profile_package(
                &required_path(options, "source")?,
                &report,
                &snapshot,
                &metadata,
                &key,
            )?
        }
    };
    let package = if options.contains_key("unsigned") {
        package
    } else {
        let signing_key = vault_secret(options, "signing-key-namespace", "signing-key-name")?;
        sign_package(
            &package,
            signing_key.as_bytes(),
            SignatureMetadata {
                key_id: required(options, "signing-key-id")?.to_owned(),
                signer: required(options, "signer")?.to_owned(),
                signed_at: required(options, "signed-at")?.to_owned(),
            },
        )?
    };
    let output = required_path(options, "output")?;
    write_package(&output, &package)?;
    print_json(&json!({
        "status": "completed",
        "output": output,
        "files": package.files().len()
    }))
}

fn verify(options: &Options) -> Result<(), Problem> {
    let package = read_package(&required_path(options, "package")?)?;
    let verified = if options.contains_key("checksums-only") {
        verify_package(&package)?
    } else {
        let signing_key = vault_secret(options, "signing-key-namespace", "signing-key-name")?;
        verify_signed_package(&package, signing_key.as_bytes())?
    };
    print_json(&verified)
}

fn import(options: &Options) -> Result<(), Problem> {
    let package = read_package(&required_path(options, "package")?)?;
    let signing_key = vault_secret(options, "signing-key-namespace", "signing-key-name")?;
    verify_signed_package(&package, signing_key.as_bytes())?;
    if package.file("entities/accounts.jsonl").is_some() {
        return Err(problem(
            "migration_control_authority_required",
            "Control-plane migration packages must be imported through the BugLogin control API",
            409,
        ));
    }
    let key = migration_key(options)?;
    let destination = required_path(options, "destination")?;
    if options.contains_key("dry-run") {
        let plan = plan_package_activation(&destination, &package, &key)?;
        return print_json(&json!({
            "status": "completed",
            "dryRun": true,
            "destinationChanged": false,
            "plan": plan
        }));
    }
    let run_id = required(options, "run-id")?;
    let staging = required_path(options, "staging")?;
    if options.contains_key("discard-staging") {
        discard_staging(&package, &staging, run_id)?;
        return print_json(&json!({
            "status": "discarded",
            "runId": run_id,
            "destinationChanged": false
        }));
    }
    let staged = stage_package(&package, &staging, run_id)?;
    let receipt = promote_staged(
        &staged,
        &destination,
        confirm_cutover(required(options, "confirm")?)?,
    )?;
    let activation = match activate_package(&destination, run_id, &package, Some(&key)) {
        Ok(receipt) => receipt,
        Err(error) => {
            if let Ok(confirmation) = confirm_rollback("ROLLBACK_MIGRATION_RUN") {
                let _ = rollback_activation(&destination, run_id, confirmation.clone());
                let _ = rollback_cutover(&destination, run_id, confirmation);
            }
            return Err(error);
        }
    };
    print_json(&json!({
        "status": "completed",
        "runId": receipt.run_id,
        "packageId": receipt.package_id,
        "artifacts": receipt.artifacts,
        "createdProfiles": activation.created_profiles,
        "reusedProfiles": activation.reused_profiles
    }))
}

fn reconcile(options: &Options) -> Result<(), Problem> {
    let source_report: InspectReport = read_json(&required_path(options, "source-report")?)?;
    let report = reconcile_activation(
        &source_report,
        &required_path(options, "destination")?,
        required(options, "run-id")?,
    )?;
    print_json(&report)?;
    if report.status == ReconciliationStatus::Blocked {
        return Err(problem(
            "migration_reconciliation_blocked",
            "Migration reconciliation is blocked",
            409,
        ));
    }
    Ok(())
}
fn rollback(options: &Options) -> Result<(), Problem> {
    let run_id = required(options, "cutover-id")?;
    let receipt = rollback_cutover(
        &required_path(options, "destination")?,
        run_id,
        confirm_rollback(required(options, "confirm")?)?,
    )?;
    print_json(&json!({
        "status": "completed",
        "runId": receipt.run_id,
        "removedArtifacts": receipt.removed_artifacts,
        "retainedUnownedArtifacts": receipt.retained_unowned_artifacts
    }))
}

type Options = BTreeMap<String, Option<String>>;

fn parse(args: Vec<String>) -> Result<(String, Options), Problem> {
    let command = args.first().cloned().ok_or_else(|| {
        problem(
            "migration_command_missing",
            "Migration command is required",
            400,
        )
    })?;
    let mut options = Options::new();
    let mut index = 1;
    while index < args.len() {
        let name = args[index]
            .strip_prefix("--")
            .filter(|name| !name.is_empty())
            .ok_or_else(|| {
                problem(
                    "migration_argument_invalid",
                    "Migration argument is invalid",
                    400,
                )
            })?
            .to_owned();
        if options.contains_key(&name) {
            return Err(problem(
                "migration_argument_duplicate",
                "Migration argument is duplicated",
                400,
            )
            .with_field("argument", name));
        }
        if index + 1 < args.len() && !args[index + 1].starts_with("--") {
            options.insert(name, Some(args[index + 1].clone()));
            index += 2;
        } else {
            options.insert(name, None);
            index += 1;
        }
    }
    Ok((command, options))
}

fn required<'a>(options: &'a Options, name: &str) -> Result<&'a str, Problem> {
    options
        .get(name)
        .and_then(Option::as_deref)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            problem(
                "migration_argument_missing",
                "Required migration argument is missing",
                400,
            )
            .with_field("argument", name)
        })
}

fn required_path(options: &Options, name: &str) -> Result<PathBuf, Problem> {
    Ok(PathBuf::from(required(options, name)?))
}

fn read_package(root: &Path) -> Result<NeutralPackage, Problem> {
    let metadata = fs::symlink_metadata(root).map_err(io_problem)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(problem(
            "migration_package_directory_invalid",
            "Migration package directory is invalid",
            400,
        ));
    }
    let mut files = BTreeMap::new();
    collect_package_files(root, root, &mut files)?;
    Ok(NeutralPackage::from_files(files))
}

fn collect_package_files(
    root: &Path,
    current: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> Result<(), Problem> {
    for entry in fs::read_dir(current).map_err(io_problem)? {
        let entry = entry.map_err(io_problem)?;
        let file_type = entry.file_type().map_err(io_problem)?;
        if file_type.is_symlink() {
            return Err(problem(
                "migration_symlink_rejected",
                "Migration symlink is not allowed",
                400,
            ));
        }
        if file_type.is_dir() {
            collect_package_files(root, &entry.path(), files)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(problem(
                "migration_package_entry_invalid",
                "Migration package entry is invalid",
                400,
            ));
        }
        let relative = entry
            .path()
            .strip_prefix(root)
            .map_err(|_| invalid_path())?
            .components()
            .map(|component| match component {
                Component::Normal(part) => part.to_str().ok_or_else(invalid_path),
                _ => Err(invalid_path()),
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("/");
        validate_relative_path(&relative)?;
        files.insert(relative, fs::read(entry.path()).map_err(io_problem)?);
    }
    Ok(())
}

fn write_package(root: &Path, package: &NeutralPackage) -> Result<(), Problem> {
    create_new_directory(root)?;
    for (relative, bytes) in package.files() {
        validate_relative_path(relative)?;
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(io_problem)?;
        }
        write_new(&path, bytes)?;
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), Problem> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_path());
    }
    Ok(())
}

fn create_new_directory(path: &Path) -> Result<(), Problem> {
    fs::create_dir(path).map_err(io_problem)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, Problem> {
    serde_json::from_slice(&fs::read(path).map_err(io_problem)?).map_err(|_| {
        problem(
            "migration_json_invalid",
            "Migration JSON input is invalid",
            400,
        )
    })
}

fn write_json_new_parent(path: &Path, value: &impl Serialize) -> Result<(), Problem> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(io_problem)?;
    }
    write_json(path, value)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), Problem> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| {
        problem(
            "migration_json_invalid",
            "Migration JSON output is invalid",
            500,
        )
    })?;
    write_new(path, &bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Problem> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_problem)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(io_problem)
}

/// Key material is created owner-only (0600) instead of inheriting the umask.
fn write_new_secret(path: &Path, bytes: &[u8]) -> Result<(), Problem> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(io_problem)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(io_problem)
}

fn print_json(value: &impl Serialize) -> Result<(), Problem> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(|_| {
            problem(
                "migration_json_invalid",
                "Migration JSON output is invalid",
                500,
            )
        })?
    );
    Ok(())
}

fn vault_secret(
    options: &Options,
    namespace_argument: &str,
    name_argument: &str,
) -> Result<SecretValue, Problem> {
    let reference = SecretKey::new(
        required(options, namespace_argument)?,
        required(options, name_argument)?,
    )
    .map_err(|error| problem(error.code(), "Secret key reference is invalid", 400))?;
    NativeVault::new()
        .read(&reference)
        .map(|(secret, _)| secret)
        .map_err(|error| problem(error.code(), "Secret key is unavailable", 503))
}
fn migration_key(options: &Options) -> Result<MigrationKey, Problem> {
    let secret = vault_secret(options, "key-namespace", "key-name")?;

    let bytes: [u8; 32] = secret.as_bytes().try_into().map_err(|_| {
        problem(
            "migration_key_invalid",
            "Migration key must be 32 bytes",
            503,
        )
    })?;
    Ok(MigrationKey::from_bytes(bytes))
}
fn invalid_path() -> Problem {
    problem(
        "migration_package_path_invalid",
        "Migration package path is invalid",
        400,
    )
}

fn io_problem(error: std::io::Error) -> Problem {
    problem(
        "migration_io_error",
        "Migration filesystem operation failed",
        500,
    )
    .with_detail(error.to_string())
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "migration-cli")
}

#[cfg(test)]
mod tests {
    use super::write_new_secret;

    #[cfg(unix)]
    #[test]
    fn migration_key_file_is_readable_only_by_its_owner() {
        use std::os::unix::fs::PermissionsExt;

        let directory = std::env::temp_dir().join(format!(
            "buglogin-migrate-secret-file-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("keys.env");

        write_new_secret(&path, b"BUGLOGIN_MIGRATION_ENCRYPTION_KEY_HEX=00\n").unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert!(write_new_secret(&path, b"overwrite").is_err());
        std::fs::remove_dir_all(directory).unwrap();
    }
}
