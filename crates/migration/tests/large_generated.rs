use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use buglogin_migration::{
    MigrationKey,
    inspect::{InspectReport, inspect_source},
    legacy::map_legacy_profiles_for_encrypted_package,
    package::{NeutralPackageManifest, PackageMetadata, build_encrypted_profile_package},
    snapshot::{SourceSnapshot, create_snapshot},
    verify::verify_package,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FixtureContract {
    seed: u64,
    profile_count: u64,
    logical_object_count: u64,
    object_shards: u64,
    budgets: Budgets,
    expected: Expected,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Budgets {
    max_generated_files: u64,
    max_source_bytes: u64,
    max_generate_millis: u64,
    max_pipeline_millis: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Expected {
    generated_files: u64,
    source_bytes: u64,
    source_tree_sha256: String,
    logical_objects_sha256: String,
    profiles_jsonl_sha256: String,
}

struct GeneratedSource {
    logical_objects_sha256: String,
    logical_object_count: u64,
    generated_files: u64,
    generation_elapsed: Duration,
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "buglogin-mig-028-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("create MIG-028 test directory");
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn mig_028_large_generated_is_deterministic_bounded_and_source_preserving() {
    let contract = fixture_contract();
    assert_eq!(contract.profile_count, 10_000);
    assert_eq!(contract.logical_object_count, 100_000);
    assert!(contract.object_shards < contract.logical_object_count);

    let test_directory = TestDirectory::new();
    let source = test_directory.0.join("source");
    let generated = generate_source(&source, &contract);
    assert_eq!(
        generated.logical_object_count,
        contract.logical_object_count
    );
    assert_eq!(generated.generated_files, contract.expected.generated_files);
    assert!(generated.generated_files <= contract.budgets.max_generated_files);
    assert!(
        generated.generation_elapsed <= Duration::from_millis(contract.budgets.max_generate_millis),
        "generation exceeded budget: {:?}",
        generated.generation_elapsed
    );

    let pipeline_started = Instant::now();
    let before = create_snapshot(&source).expect("snapshot generated source before migration");
    let (report, snapshot) = inspect_source(&source).expect("inspect generated source");
    assert_eq!(report.entity_counts["profiles"], contract.profile_count);
    assert_eq!(
        snapshot.files.len() as u64,
        contract.expected.generated_files
    );
    eprintln!(
        "MIG-028 generated={} objects={} bytes={} source={} logical={} generation_ms={}",
        generated.generated_files,
        generated.logical_object_count,
        snapshot.total_bytes,
        snapshot.source_tree_sha256,
        generated.logical_objects_sha256,
        generated.generation_elapsed.as_millis()
    );
    assert_eq!(snapshot.total_bytes, contract.expected.source_bytes);
    assert!(snapshot.total_bytes <= contract.budgets.max_source_bytes);
    assert_eq!(
        snapshot.source_tree_sha256,
        contract.expected.source_tree_sha256
    );
    assert_eq!(
        generated.logical_objects_sha256,
        contract.expected.logical_objects_sha256
    );

    verify_generated_package(
        &source,
        &contract,
        &before,
        &report,
        &snapshot,
        pipeline_started,
    );
}

fn verify_generated_package(
    source: &Path,
    contract: &FixtureContract,
    before: &SourceSnapshot,
    report: &InspectReport,
    snapshot: &SourceSnapshot,
    pipeline_started: Instant,
) {
    let profiles = map_legacy_profiles_for_encrypted_package(source, &report.source.instance_id)
        .expect("map all generated profiles");
    assert_eq!(profiles.records.len() as u64, contract.profile_count);
    assert!(profiles.warnings.is_empty());

    let package = build_encrypted_profile_package(
        source,
        report,
        snapshot,
        &PackageMetadata {
            created_at: "2026-07-22T00:00:00Z".to_owned(),
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            tool_commit: "0000000000000000000000000000000000000000".to_owned(),
        },
        &MigrationKey::from_bytes([28; 32]),
    )
    .expect("build large generated package");
    verify_package(&package).expect("verify large generated package");
    let packaged_profiles = package
        .file("entities/profiles.jsonl")
        .expect("large package profile records");
    assert_eq!(count_json_lines(packaged_profiles), contract.profile_count);
    eprintln!("MIG-028 profiles={}", sha256(packaged_profiles));
    assert_eq!(
        sha256(packaged_profiles),
        contract.expected.profiles_jsonl_sha256
    );
    let manifest: NeutralPackageManifest = serde_json::from_slice(
        package
            .file("manifest.json")
            .expect("large package manifest"),
    )
    .expect("parse large package manifest");
    assert_eq!(manifest.source.snapshot_sha256, snapshot.source_tree_sha256);

    let after = create_snapshot(source).expect("snapshot generated source after migration");
    assert_eq!(
        before, &after,
        "migration pipeline changed generated source"
    );
    let pipeline_elapsed = pipeline_started.elapsed();
    assert!(
        pipeline_elapsed <= Duration::from_millis(contract.budgets.max_pipeline_millis),
        "pipeline exceeded budget: {pipeline_elapsed:?}"
    );
    eprintln!("MIG-028 pipeline_ms={}", pipeline_elapsed.as_millis());
}

fn generate_source(source: &Path, contract: &FixtureContract) -> GeneratedSource {
    let started = Instant::now();
    fs::create_dir(source).expect("create generated source root");
    fs::write(
        source.join("version.json"),
        format!(
            "{{\"system\":\"buglogin-legacy\",\"version\":\"1.1.84\",\"instanceId\":\"mig-028-seed-{seed:016x}\",\"os\":\"windows\",\"architecture\":\"x86_64\"}}\n",
            seed = contract.seed
        ),
    )
    .expect("write generated source identity");

    let profiles_root = source.join("profiles");
    fs::create_dir(&profiles_root).expect("create generated profiles root");
    for index in 0..contract.profile_count {
        let id = profile_id(contract.seed, index);
        let profile_root = profiles_root.join(&id);
        fs::create_dir(&profile_root).expect("create generated profile directory");
        let marker = splitmix64(contract.seed ^ index);
        fs::write(
            profile_root.join("metadata.json"),
            format!(
                "{{\"id\":\"{id}\",\"name\":\"Generated {index:05}\",\"runtime\":\"wayfern\",\"tags\":[\"mig-028\",\"bucket-{bucket:02}\"],\"note\":\"seed-{marker:016x}\"}}\n",
                bucket = marker % 64
            ),
        )
        .expect("write generated profile metadata");
    }

    let (logical_objects_sha256, logical_object_count) = generate_logical_objects(source, contract);
    GeneratedSource {
        logical_objects_sha256,
        logical_object_count,
        generated_files: 1 + contract.profile_count + contract.object_shards,
        generation_elapsed: started.elapsed(),
    }
}

fn generate_logical_objects(source: &Path, contract: &FixtureContract) -> (String, u64) {
    let objects_root = source.join("generated-objects");
    fs::create_dir(&objects_root).expect("create generated object shards root");
    let mut logical_hash = Sha256::new();
    let mut logical_object_count = 0;
    for shard in 0..contract.object_shards {
        let start = shard * contract.logical_object_count / contract.object_shards;
        let end = (shard + 1) * contract.logical_object_count / contract.object_shards;
        let mut bytes = Vec::with_capacity(((end - start) * 96) as usize);
        for index in start..end {
            let value = splitmix64(contract.seed.wrapping_add(index));
            let line = format!(
                "{{\"id\":\"object-{index:06}\",\"profileIndex\":{},\"revision\":{},\"value\":\"{value:016x}\"}}\n",
                index % contract.profile_count,
                value % 17 + 1
            );
            logical_hash.update(line.as_bytes());
            bytes.extend_from_slice(line.as_bytes());
            logical_object_count += 1;
        }
        fs::write(objects_root.join(format!("shard-{shard:03}.jsonl")), bytes)
            .expect("write generated object shard");
    }
    (
        format!("{:x}", logical_hash.finalize()),
        logical_object_count,
    )
}

fn profile_id(seed: u64, index: u64) -> String {
    let high = splitmix64(seed.wrapping_add(index));
    let low = splitmix64(seed ^ index.rotate_left(17));
    let variant = 0x8000 | ((low >> 48) as u16 & 0x3fff);
    format!(
        "{:08x}-{:04x}-4{:03x}-{variant:04x}-{:012x}",
        high >> 32,
        (high >> 16) as u16,
        high as u16 & 0x0fff,
        low & 0x0000_ffff_ffff_ffff
    )
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn count_json_lines(bytes: &[u8]) -> u64 {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .count() as u64
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn fixture_contract() -> FixtureContract {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("fixtures/migration/MIG-028-large-generated/fixture.json");
    serde_json::from_slice(&fs::read(path).expect("read MIG-028 fixture contract"))
        .expect("parse MIG-028 fixture contract")
}
