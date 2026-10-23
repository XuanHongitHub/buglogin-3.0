use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_migration::MigrationKey;
use buglogin_migration::inspect::inspect_source;
use buglogin_migration::package::{PackageMetadata, build_encrypted_profile_package};
use buglogin_migration::plan_package_activation;
use buglogin_migration::signature::{SignatureMetadata, sign_package, verify_signed_package};
use serde_json::Value;

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "buglogin-migration-cli-{label}-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create CLI test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn child(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.child(relative);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
        fs::write(path, bytes).expect("write fixture");
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn cli_inspects_exports_and_verifies_sanitized_package() {
    let source = TestDirectory::new("source");
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"cli-fixture","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "profiles/00000000-0000-0000-0000-000000000001/metadata.json",
        br#"{"id":"00000000-0000-0000-0000-000000000001","name":"One","runtime":"wayfern"}"#,
    );
    source.write(
        "profiles/00000000-0000-0000-0000-000000000001/profile/Preferences",
        b"profile-data",
    );
    let work = TestDirectory::new("work");
    let report = work.child("report");
    let package = work.child("package");
    let staging = work.child("staging");
    let destination = work.child("destination");

    assert_success(run(&[
        "inspect",
        "--source",
        text(source.path()),
        "--report",
        text(&report),
    ]));
    assert!(report.join("inspect.json").is_file());
    assert!(report.join("snapshot.json").is_file());

    assert_success(run(&[
        "export",
        "--report",
        text(&report.join("inspect.json")),
        "--snapshot",
        text(&report.join("snapshot.json")),
        "--output",
        text(&package),
        "--created-at",
        "2026-07-21T00:00:00Z",
        "--tool-commit",
        "0123456789abcdef0123456789abcdef01234567",
        "--redacted",
        "--unsigned",
    ]));
    assert_success(run(&[
        "verify",
        "--package",
        text(&package),
        "--checksums-only",
    ]));

    let dry_run = run(&[
        "import",
        "--package",
        text(&package),
        "--destination",
        text(&destination),
        "--dry-run",
    ]);
    assert!(!dry_run.status.success());
    assert_eq!(error_code(&dry_run), "migration_argument_missing");
    assert!(!destination.exists());

    let missing_key = run(&[
        "import",
        "--package",
        text(&package),
        "--destination",
        text(&destination),
        "--staging",
        text(&staging),
        "--run-id",
        "cli-cutover-1",
        "--confirm",
        "CUTOVER_AND_PROMOTE",
    ]);
    assert!(!missing_key.status.success());
    assert_eq!(error_code(&missing_key), "migration_argument_missing");
    assert!(!destination.exists());
}

#[test]
fn cli_import_failure_is_side_effect_free_when_arguments_are_incomplete() {
    let package = TestDirectory::new("import-failure-package");
    package.write("checksums.sha256", b"not-a-valid-checksum\n");
    let destination = TestDirectory::new("import-failure-destination");
    let staging = TestDirectory::new("import-failure-staging");

    let result = run(&[
        "import",
        "--package",
        text(package.path()),
        "--destination",
        text(destination.path()),
        "--staging",
        text(staging.path()),
        "--run-id",
        "cli-import-failure-001",
        "--confirm",
        "CUTOVER_AND_PROMOTE",
    ]);

    assert!(!result.status.success());
    assert_eq!(error_code(&result), "migration_argument_missing");
    assert!(!destination.path().join("migration-imports").exists());
    assert!(!staging.path().join("cli-import-failure-001").exists());
}

#[test]
fn cli_rejects_invalid_command_and_non_directory_package() {
    let missing_command = run(&[]);
    assert!(!missing_command.status.success());
    assert_eq!(error_code(&missing_command), "migration_command_missing");

    let unknown_command = run(&["not-a-command"]);
    assert!(!unknown_command.status.success());
    assert_eq!(error_code(&unknown_command), "migration_command_invalid");

    let work = TestDirectory::new("invalid-package-path");
    let package_file = work.child("package-file");
    fs::write(&package_file, b"not a package").expect("write package file");
    let result = run(&[
        "verify",
        "--package",
        text(&package_file),
        "--checksums-only",
    ]);
    assert!(!result.status.success());
    assert_eq!(error_code(&result), "migration_package_directory_invalid");
}

#[test]
fn postgres_control_export_requires_explicit_read_only_mode() {
    let work = TestDirectory::new("postgres-control-export-read-only");
    let policy = work.child("storage-policy.json");
    let output = work.child("control-plane.json");
    let result = run(&[
        "export-control-plane",
        "--expected-database",
        "buglogin3_rehearsal_fixture",
        "--source-instance-id",
        "legacy-fixture",
        "--storage-policy",
        text(&policy),
        "--output",
        text(&output),
    ]);

    assert!(!result.status.success());
    assert_eq!(error_code(&result), "migration_read_only_required");
    assert!(!output.exists());
}

#[test]
fn cli_verify_rejects_mutated_package_bytes_before_import() {
    let source = TestDirectory::new("mutation-source");
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"mutation-cli","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "profiles/one/metadata.json",
        br#"{"id":"one","name":"One","runtime":"wayfern"}"#,
    );
    source.write("profiles/one/profile/Preferences", b"profile-data");
    let work = TestDirectory::new("mutation-work");
    let report = work.child("report");
    let package = work.child("package");

    assert_success(run(&[
        "inspect",
        "--source",
        text(source.path()),
        "--report",
        text(&report),
    ]));
    assert_success(run(&[
        "export",
        "--report",
        text(&report.join("inspect.json")),
        "--snapshot",
        text(&report.join("snapshot.json")),
        "--output",
        text(&package),
        "--created-at",
        "2026-08-21T00:00:00Z",
        "--tool-commit",
        "0123456789abcdef0123456789abcdef01234567",
        "--redacted",
        "--unsigned",
    ]));

    fs::write(
        package.join("manifest.json"),
        br#"{"schema":"buglogin.export/1","tampered":true}"#,
    )
    .expect("mutate package manifest");
    let result = run(&["verify", "--package", text(&package), "--checksums-only"]);
    assert!(!result.status.success());
    assert_eq!(error_code(&result), "migration_checksum_mismatch");
}

#[test]
fn signed_package_dry_run_is_synthetic_and_does_not_write_destination() {
    let source = TestDirectory::new("signed-dry-run-source");
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"signed-dry-run","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "profiles/00000000-0000-0000-0000-000000000001/metadata.json",
        br#"{"id":"00000000-0000-0000-0000-000000000001","name":"One","runtime":"wayfern"}"#,
    );
    source.write(
        "profiles/00000000-0000-0000-0000-000000000001/profile/Preferences",
        b"profile-data",
    );

    let (report, snapshot) = inspect_source(source.path()).expect("inspect synthetic source");
    let migration_key = MigrationKey::from_bytes([17; 32]);
    let package = build_encrypted_profile_package(
        source.path(),
        &report,
        &snapshot,
        &PackageMetadata {
            created_at: "2026-08-21T00:00:00Z".into(),
            tool_version: "0.1.0".into(),
            tool_commit: "0123456789abcdef0123456789abcdef01234567".into(),
        },
        &migration_key,
    )
    .expect("build encrypted synthetic package");
    let signed = sign_package(
        &package,
        &[23; 32],
        SignatureMetadata {
            key_id: "synthetic-signing-key".into(),
            signer: "migration-test".into(),
            signed_at: "2026-08-21T00:01:00Z".into(),
        },
    )
    .expect("sign synthetic package");

    let verified = verify_signed_package(&signed, &[23; 32]).expect("verify signed package");
    assert_eq!(verified.package_id, package_id(&signed));

    let destination = TestDirectory::new("signed-dry-run-destination");
    let before = fs::read_dir(destination.path())
        .expect("read destination before dry-run")
        .count();
    let plan = plan_package_activation(destination.path(), &signed, &migration_key)
        .expect("plan signed package");
    assert_eq!(plan.create_profile_ids.len(), 1);
    assert_eq!(
        fs::read_dir(destination.path())
            .expect("read destination after dry-run")
            .count(),
        before
    );
}

#[test]
fn cli_prepare_source_is_copy_only_and_canonicalizes_control_plane() {
    let source = TestDirectory::new("prepare-source");
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"prepare-fixture","os":"windows","architecture":"x86_64"}"#,
    );
    source.write(
        "profiles/one/metadata.json",
        br#"{"id":"one","name":"One","runtime":"wayfern"}"#,
    );
    let control = TestDirectory::new("control-export");
    control.write(
        "export.json",
        br#"{
          "organizations":[{"id":"org-a","name":"Operations","slug":"operations","status":"active","createdBy":"owner-a","createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z"}],
          "workspaces":[{"id":"ws-a","organizationId":"org-a","name":"Default","mode":"team","createdAt":"2026-01-01T00:00:00Z","createdBy":"owner-a","profileLimit":10,"memberLimit":10,"maxStorageBytes":1000,"entitlementState":"active","localEntitlementState":"active"}],
          "memberships":[{"workspaceId":"ws-a","userId":"owner-a","email":"owner@example.com","role":"owner","resourceAccessMode":"full","customCapabilities":null,"accountState":"active","platformRole":null,"accessSource":"direct","createdAt":"2026-01-01T00:00:00Z"}],
          "shareGrants":[]
        }"#,
    );
    let work = TestDirectory::new("prepare-work");
    let output = work.child("prepared");
    let result = run(&[
        "prepare-source",
        "--source",
        text(source.path()),
        "--control-plane",
        text(&control.child("export.json")),
        "--output",
        text(&output),
        "--read-only",
    ]);
    assert_success(result);
    assert!(output.join("version.json").is_file());
    assert!(output.join("control-plane-export.json").is_file());
    assert!(!source.child("control-plane-export.json").exists());
}

#[cfg(unix)]
#[test]
fn cli_prepare_source_skips_chromium_runtime_links_and_binaries() {
    let source = TestDirectory::new("prepare-source-runtime-artifacts");
    source.write(
        "version.json",
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"prepare-runtime","os":"macos","architecture":"aarch64"}"#,
    );
    source.write(
        "profiles/one/metadata.json",
        br#"{"id":"one","name":"One","runtime":"wayfern"}"#,
    );
    source.write("profiles/one/profile/Preferences", b"{}");
    std::os::unix::fs::symlink(
        "MacBook-Pro-12345",
        source.child("profiles/one/profile/SingletonLock"),
    )
    .expect("create chromium singleton link");
    source.write(
        "binaries/wayfern/154.0/Wayfern.app/Contents/Info.plist",
        b"plist",
    );
    std::os::unix::fs::symlink(
        "Versions/Current/Frameworks",
        source.child("binaries/wayfern/154.0/Wayfern.app/Contents/Frameworks"),
    )
    .expect("create bundle link");
    let control = TestDirectory::new("prepare-source-runtime-control");
    control.write(
        "export.json",
        br#"{"organizations":[],"workspaces":[],"memberships":[],"shareGrants":[]}"#,
    );
    let work = TestDirectory::new("prepare-source-runtime-work");
    let output = work.child("prepared");

    let result = run(&[
        "prepare-source",
        "--source",
        text(source.path()),
        "--control-plane",
        text(&control.child("export.json")),
        "--output",
        text(&output),
        "--read-only",
    ]);

    assert_success(result);
    assert!(output.join("profiles/one/profile/Preferences").is_file());
    assert!(fs::symlink_metadata(output.join("profiles/one/profile/SingletonLock")).is_err());
    assert!(!output.join("binaries").exists());
}

#[test]
fn cli_prepare_source_rejects_symlinked_source_ancestor_before_canonicalization() {
    let root = TestDirectory::new("prepare-source-ancestor-link");
    let outside = root.child("outside");
    let linked_parent = root.child("linked-parent");
    let source = outside.join("source");
    fs::create_dir_all(&source).expect("create source");
    fs::write(
        source.join("version.json"),
        br#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"ancestor-link","os":"windows","architecture":"x86_64"}"#,
    )
    .expect("write source metadata");
    create_directory_link(&outside, &linked_parent);

    let control = TestDirectory::new("prepare-source-ancestor-control");
    control.write(
        "export.json",
        br#"{"organizations":[],"workspaces":[],"memberships":[],"shareGrants":[]}"#,
    );
    let output_root = TestDirectory::new("prepare-source-ancestor-output");
    let output = run(&[
        "prepare-source",
        "--source",
        text(&linked_parent.join("source")),
        "--control-plane",
        text(&control.child("export.json")),
        "--output",
        text(&output_root.child("prepared")),
        "--read-only",
    ]);

    assert!(!output.status.success());
    assert_eq!(error_code(&output), "migration_source_symlink_unsupported");
    assert!(!output_root.child("prepared").exists());
}

#[test]
fn cli_inspect_supports_manifestless_production_data_root_with_explicit_identity() {
    let source = TestDirectory::new("inspect-production-root");
    source.write(
        "profiles/one/metadata.json",
        br#"{"id":"one","name":"One","runtime":"wayfern"}"#,
    );
    let work = TestDirectory::new("inspect-production-work");
    let report = work.child("report");
    assert_success(run(&[
        "inspect",
        "--source",
        text(source.path()),
        "--report",
        text(&report),
        "--source-version",
        "1.1.84",
        "--instance-id",
        "production-local",
        "--source-os",
        "windows",
        "--architecture",
        "x86_64",
    ]));
    let inspect: Value =
        serde_json::from_slice(&fs::read(report.join("inspect.json")).unwrap()).unwrap();
    assert_eq!(inspect["source"]["instanceId"], "production-local");
    assert!(!source.child("version.json").exists());
}

#[test]
fn cli_prepare_source_generates_manifest_for_production_data_root() {
    let source = TestDirectory::new("prepare-production-root");
    source.write(
        "profiles/one/metadata.json",
        br#"{"id":"one","name":"One","runtime":"wayfern"}"#,
    );
    let control = TestDirectory::new("prepare-production-control");
    control.write(
        "export.json",
        br#"{"organizations":[],"workspaces":[],"memberships":[],"shareGrants":[]}"#,
    );
    let work = TestDirectory::new("prepare-production-work");
    let output = work.child("prepared");
    assert_success(run(&[
        "prepare-source",
        "--source",
        text(source.path()),
        "--control-plane",
        text(&control.child("export.json")),
        "--output",
        text(&output),
        "--read-only",
        "--source-version",
        "1.1.84",
        "--instance-id",
        "production-local",
        "--source-os",
        "windows",
        "--architecture",
        "x86_64",
    ]));
    assert!(!source.child("version.json").exists());
    let generated: Value =
        serde_json::from_slice(&fs::read(output.join("version.json")).unwrap()).unwrap();
    assert_eq!(generated["system"], "buglogin-legacy");
    assert_eq!(generated["version"], "1.1.84");
    assert_eq!(generated["instanceId"], "production-local");
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_buglogin-migrate"))
        .args(args)
        .output()
        .expect("run buglogin-migrate")
}

fn assert_success(output: Output) {
    assert!(
        output.status.success(),
        "command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice::<Value>(&output.stdout).expect("command emits JSON");
}

fn error_code(output: &Output) -> String {
    serde_json::from_slice::<Value>(&output.stderr).expect("error emits JSON")["code"]
        .as_str()
        .expect("error code")
        .to_owned()
}

fn text(path: &Path) -> &str {
    path.to_str().expect("UTF-8 test path")
}

#[cfg(unix)]
fn create_directory_link(target: &Path, link: &Path) {
    std::os::unix::fs::symlink(target, link).expect("create source directory symlink");
}

#[cfg(windows)]
fn create_directory_link(target: &Path, link: &Path) {
    let status = Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .status()
        .expect("run junction command");
    assert!(status.success(), "create source directory junction");
}

fn package_id(package: &buglogin_migration::package::NeutralPackage) -> String {
    serde_json::from_slice::<Value>(package.file("manifest.json").expect("manifest"))
        .expect("manifest JSON")["package_id"]
        .as_str()
        .expect("package ID")
        .to_owned()
}
