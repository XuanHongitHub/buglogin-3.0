use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_domain::errors::Problem;
use s3::{bucket::Bucket, creds::Credentials, region::Region};
use serde::Deserialize;
use serde_json::json;
use unicode_normalization::UnicodeNormalization;

use super::{LegacyControlPlaneExport, LegacyControlProfile};

static STAGING_SEQUENCE: AtomicU64 = AtomicU64::new(0);
const MAX_PROFILE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_OBJECT_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LegacyS3HydrationReport {
    pub hosted_profiles: usize,
    pub manifests_found: usize,
    pub profiles_hydrated: usize,
    pub profiles_using_local_fallback: usize,
    pub profiles_preserve_only: usize,
    pub profiles_rejected: usize,
    pub files_hydrated: usize,
    pub bytes_hydrated: u64,
    pub proxies_hydrated: usize,
    pub proxies_missing: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LegacyManifest {
    version: u32,
    profile_id: String,
    #[serde(rename = "generatedAt")]
    _generated_at: String,
    #[serde(rename = "updatedAt")]
    _updated_at: String,
    #[serde(rename = "excludeGlobs")]
    _exclude_globs: Vec<String>,
    files: Vec<LegacyManifestFile>,
    #[serde(default)]
    encrypted: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyManifestFile {
    path: String,
    size: u64,
    #[serde(rename = "mtime")]
    _mtime: i64,
    hash: String,
}

struct S3Source {
    bucket: Box<Bucket>,
    root_prefix: String,
}

struct TempDirectory(PathBuf);

impl Drop for TempDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl S3Source {
    fn from_env() -> Result<Self, Problem> {
        let endpoint = required_env("BUGLOGIN_LEGACY_S3_ENDPOINT")?;
        let bucket_name = required_env("BUGLOGIN_LEGACY_S3_BUCKET")?;
        let region_name =
            env::var("BUGLOGIN_LEGACY_S3_REGION").unwrap_or_else(|_| "us-east-1".to_owned());
        let access_key = required_env("BUGLOGIN_LEGACY_S3_ACCESS_KEY")?;
        let secret_key = required_env("BUGLOGIN_LEGACY_S3_SECRET_KEY")?;
        let root_prefix = env::var("BUGLOGIN_LEGACY_S3_ROOT_PREFIX")
            .unwrap_or_default()
            .trim_matches('/')
            .to_owned();
        let path_style = env::var("BUGLOGIN_LEGACY_S3_PATH_STYLE")
            .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        if !endpoint.starts_with("https://")
            || endpoint.contains(['\r', '\n'])
            || bucket_name.is_empty()
            || region_name.is_empty()
            || (!root_prefix.is_empty()
                && root_prefix
                    .split('/')
                    .any(|segment| !safe_key_segment(segment)))
        {
            return Err(source_invalid("legacy S3 configuration"));
        }
        let credentials = Credentials::new(
            Some(&access_key),
            Some(&secret_key),
            env::var("BUGLOGIN_LEGACY_S3_SESSION_TOKEN").ok().as_deref(),
            None,
            None,
        )
        .map_err(|_| source_invalid("legacy S3 credentials"))?;
        let region = Region::Custom {
            region: region_name,
            endpoint,
        };
        let bucket = Bucket::new(&bucket_name, region, credentials)
            .map_err(|_| source_unavailable("legacy S3 connection"))?;
        let bucket = if path_style {
            bucket.with_path_style()
        } else {
            bucket
        };
        Ok(Self {
            bucket,
            root_prefix,
        })
    }

    fn key(&self, workspace_id: &str, profile_id: &str, relative: &str) -> String {
        let root = if self.root_prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", self.root_prefix)
        };
        format!("{root}workspaces/{workspace_id}/profiles/{profile_id}/{relative}")
    }

    fn proxy_key(&self, proxy_id: &str) -> String {
        let root = if self.root_prefix.is_empty() {
            String::new()
        } else {
            format!("{}/", self.root_prefix)
        };
        format!("{root}proxies/{proxy_id}.json")
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, Problem> {
        let response = self
            .bucket
            .get_object(key)
            .map_err(|_| source_unavailable("legacy S3 object read"))?;
        match response.status_code() {
            200..=299 => Ok(Some(response.bytes().to_vec())),
            404 => Ok(None),
            _ => Err(source_unavailable("legacy S3 object read")),
        }
    }
}

/// Read profile manifests and objects from the legacy bucket and hydrate the
/// prepared source tree. Every object is size- and BLAKE3-checked before it is
/// installed. A bad/incomplete cloud profile keeps its existing local copy;
/// without a local copy it remains a control-plane-only preserve-only record.
pub fn hydrate_legacy_s3_profile_payloads(
    prepared_source: &Path,
) -> Result<LegacyS3HydrationReport, Problem> {
    let prepared_source = fs::canonicalize(prepared_source)
        .map_err(|_| source_invalid("prepared migration source"))?;
    let control_path = prepared_source.join("control-plane-export.json");
    let control_bytes =
        fs::read(&control_path).map_err(|_| source_invalid("prepared control-plane export"))?;
    let export: LegacyControlPlaneExport = serde_json::from_slice(&control_bytes)
        .map_err(|_| source_invalid("prepared control-plane export"))?;
    let source = S3Source::from_env()?;
    let mut report = LegacyS3HydrationReport {
        hosted_profiles: export.profiles.len(),
        ..LegacyS3HydrationReport::default()
    };

    hydrate_referenced_proxies(&source, &prepared_source, &export, &mut report)?;

    for profile in &export.profiles {
        let profile_root = profile_local_root(&prepared_source, profile)?;
        match hydrate_profile(&source, profile, &profile_root, &mut report)? {
            HydrationStatus::Hydrated => report.profiles_hydrated += 1,
            HydrationStatus::LocalFallback => report.profiles_using_local_fallback += 1,
            HydrationStatus::PreserveOnly => report.profiles_preserve_only += 1,
            HydrationStatus::RejectedWithFallback => {
                report.profiles_rejected += 1;
                report.profiles_using_local_fallback += 1;
            }
            HydrationStatus::RejectedPreserveOnly => {
                report.profiles_rejected += 1;
                report.profiles_preserve_only += 1;
            }
        }
    }
    Ok(report)
}

fn hydrate_referenced_proxies(
    source: &S3Source,
    prepared_source: &Path,
    export: &LegacyControlPlaneExport,
    report: &mut LegacyS3HydrationReport,
) -> Result<(), Problem> {
    let proxy_ids = export
        .profiles
        .iter()
        .filter_map(|profile| profile.proxy_id.as_deref())
        .collect::<BTreeSet<_>>();
    if proxy_ids.is_empty() {
        return Ok(());
    }
    let proxy_root = prepared_source.join("proxies");
    match fs::symlink_metadata(&proxy_root) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(source_invalid("prepared proxy directory")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&proxy_root)
                .map_err(|_| source_unavailable("prepared proxy directory"))?;
        }
        Err(_) => return Err(source_unavailable("prepared proxy directory")),
    }

    for proxy_id in proxy_ids {
        if !safe_key_segment(proxy_id) {
            return Err(source_invalid("hosted proxy ID"));
        }
        let target = proxy_root.join(format!("{proxy_id}.json"));
        let Some(bytes) = source.get(&source.proxy_key(proxy_id))? else {
            report.proxies_missing += 1;
            continue;
        };
        if bytes.is_empty() || bytes.len() > 1024 * 1024 {
            return Err(source_invalid("legacy proxy object size"));
        }
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| source_invalid("legacy proxy object"))?;
        if value.get("id").and_then(serde_json::Value::as_str) != Some(proxy_id) {
            return Err(source_invalid("legacy proxy identity"));
        }
        match fs::read(&target) {
            Ok(existing) if existing == bytes => {}
            Ok(_) => return Err(source_invalid("prepared proxy source changed")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_new(&target, &bytes)
                    .map_err(|_| source_unavailable("prepared proxy object"))?;
            }
            Err(_) => return Err(source_unavailable("prepared proxy object")),
        }
        report.proxies_hydrated += 1;
    }
    Ok(())
}

enum HydrationStatus {
    Hydrated,
    LocalFallback,
    PreserveOnly,
    RejectedWithFallback,
    RejectedPreserveOnly,
}

fn hydrate_profile(
    source: &S3Source,
    profile: &LegacyControlProfile,
    profile_root: &Path,
    report: &mut LegacyS3HydrationReport,
) -> Result<HydrationStatus, Problem> {
    if !safe_key_segment(&profile.workspace_id) {
        return Ok(reject_or_fallback(profile_root));
    }
    let manifest_key = source.key(&profile.workspace_id, &profile.id, "manifest.json");
    let Some(manifest_bytes) = source.get(&manifest_key)? else {
        return Ok(local_fallback_or_preserve(profile_root));
    };
    report.manifests_found += 1;
    let manifest: LegacyManifest = match serde_json::from_slice(&manifest_bytes) {
        Ok(manifest) => manifest,
        Err(_) => return Ok(reject_or_fallback(profile_root)),
    };
    if manifest.version != 1
        || manifest.profile_id != profile.id
        || manifest.encrypted
        || manifest.files.is_empty()
    {
        return Ok(reject_or_fallback(profile_root));
    }

    let total_size = manifest.files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.size)
            .ok_or_else(|| source_invalid("legacy profile size"))
    })?;
    if total_size == 0 || total_size > MAX_PROFILE_BYTES {
        return Ok(reject_or_fallback(profile_root));
    }
    let staging = create_temp_directory()?;
    let mut seen = BTreeSet::new();
    let mut profile_bytes = 0_u64;
    for file in &manifest.files {
        if !safe_relative_path(&file.path)
            || file.size > MAX_OBJECT_BYTES
            || !valid_blake3(&file.hash)
            || !seen.insert(
                file.path
                    .nfc()
                    .flat_map(char::to_lowercase)
                    .collect::<String>(),
            )
        {
            return Ok(reject_or_fallback(profile_root));
        }
        let key = source.key(
            &profile.workspace_id,
            &profile.id,
            &format!("files/{}", file.path),
        );
        let Some(bytes) = source.get(&key)? else {
            return Ok(reject_or_fallback(profile_root));
        };
        let actual_hash = blake3::hash(&bytes).to_hex().to_string();
        if bytes.len() as u64 != file.size || actual_hash != file.hash.to_ascii_lowercase() {
            return Ok(reject_or_fallback(profile_root));
        }
        let destination = staging.0.join(&file.path);
        let parent = destination
            .parent()
            .ok_or_else(|| source_invalid("legacy profile file path"))?;
        fs::create_dir_all(parent).map_err(|_| source_unavailable("legacy profile staging"))?;
        write_new(&destination, &bytes)?;
        profile_bytes = profile_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| source_invalid("legacy profile size"))?;
    }
    if profile_bytes != total_size {
        return Ok(reject_or_fallback(profile_root));
    }

    let local_id = local_profile_id(&profile.id)?;
    let profile_data = profile_root.join("profile");
    if profile_root.exists() {
        fs::remove_dir_all(profile_root)
            .map_err(|_| source_unavailable("prepared profile replacement"))?;
    }
    let metadata = json!({
        "id": local_id,
        "name": profile.name.clone(),
        "runtime": profile.runtime.clone(),
        "version": profile.runtime_version.clone(),
        "release_type": profile.release_type.clone(),
        "proxy_id": profile.proxy_id.clone(),
        "vpn_id": profile.vpn_id.clone(),
        "group_id": profile.group_id.clone(),
        "extension_group_id": profile.extension_group_id.clone(),
        "ephemeral": profile.ephemeral,
        "created_at": profile.created_at.clone(),
        "updated_at": profile.updated_at.clone(),
        "tags": profile.tags.clone(),
        "note": profile.note.clone()
    });
    fs::create_dir_all(&profile_data)
        .map_err(|_| source_unavailable("prepared profile directory"))?;
    copy_tree(&staging.0, &profile_data)?;
    let mut metadata_bytes = serde_json::to_vec_pretty(&metadata)
        .map_err(|_| source_invalid("hosted profile metadata"))?;
    metadata_bytes.push(b'\n');
    write_new(&profile_root.join("metadata.json"), &metadata_bytes)?;
    report.files_hydrated += manifest.files.len();
    report.bytes_hydrated = report
        .bytes_hydrated
        .checked_add(profile_bytes)
        .ok_or_else(|| source_invalid("hydrated profile size"))?;
    Ok(HydrationStatus::Hydrated)
}

fn profile_local_root(
    prepared_source: &Path,
    profile: &LegacyControlProfile,
) -> Result<PathBuf, Problem> {
    Ok(prepared_source
        .join("profiles")
        .join(local_profile_id(&profile.id)?))
}

fn local_profile_id(profile_id: &str) -> Result<String, Problem> {
    let compact = profile_id
        .strip_prefix("prf_")
        .filter(|value| {
            value.len() == 32
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
        .ok_or_else(|| source_invalid("hosted profile ID"))?;
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &compact[0..8],
        &compact[8..12],
        &compact[12..16],
        &compact[16..20],
        &compact[20..32]
    ))
}

fn local_fallback_or_preserve(profile_root: &Path) -> HydrationStatus {
    if profile_root.join("metadata.json").is_file()
        && has_profile_payload(&profile_root.join("profile"))
    {
        HydrationStatus::LocalFallback
    } else {
        HydrationStatus::PreserveOnly
    }
}

fn reject_or_fallback(profile_root: &Path) -> HydrationStatus {
    if profile_root.join("metadata.json").is_file()
        && has_profile_payload(&profile_root.join("profile"))
    {
        HydrationStatus::RejectedWithFallback
    } else {
        HydrationStatus::RejectedPreserveOnly
    }
}

fn has_profile_payload(path: &Path) -> bool {
    let Ok(entries) = fs::read_dir(path) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        if kind.is_symlink() {
            return false;
        }
        (kind.is_file() && entry.metadata().is_ok_and(|metadata| metadata.len() > 0))
            || (kind.is_dir() && has_profile_payload(&entry.path()))
    })
}

fn create_temp_directory() -> Result<TempDirectory, Problem> {
    let root = env::temp_dir();
    for _ in 0..16 {
        let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!(
            "buglogin-migration-s3-{}-{sequence}",
            std::process::id()
        ));
        match fs::create_dir(&path) {
            Ok(()) => return Ok(TempDirectory(path)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(source_unavailable("legacy profile temporary directory")),
        }
    }
    Err(source_unavailable("legacy profile temporary directory"))
}

fn safe_relative_path(value: &str) -> bool {
    if value.is_empty() || value.contains(['\\', '\0']) || value.starts_with('/') {
        return false;
    }
    let components = Path::new(value)
        .components()
        .map(|component| match component {
            Component::Normal(part) => part.to_str(),
            _ => None,
        })
        .collect::<Option<Vec<_>>>();
    components.is_some_and(|parts| {
        !parts.is_empty()
            && parts.join("/") == value
            && parts.iter().all(|part| safe_file_name(part))
    })
}

fn safe_file_name(value: &str) -> bool {
    if value.is_empty()
        || value.ends_with(['.', ' '])
        || value.chars().any(|character| {
            character.is_control() || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
        })
    {
        return false;
    }
    let basename = value
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    !matches!(basename.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        && !(basename.len() == 4
            && (basename.starts_with("COM") || basename.starts_with("LPT"))
            && basename.as_bytes()[3].is_ascii_digit()
            && basename.as_bytes()[3] != b'0')
}

fn safe_key_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_blake3(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), Problem> {
    for entry in fs::read_dir(source).map_err(|_| source_unavailable("profile staging read"))? {
        let entry = entry.map_err(|_| source_unavailable("profile staging read"))?;
        let kind = entry
            .file_type()
            .map_err(|_| source_unavailable("profile staging read"))?;
        let target = destination.join(entry.file_name());
        if kind.is_symlink() {
            return Err(source_invalid("profile staging symlink"));
        }
        if kind.is_dir() {
            fs::create_dir_all(&target)
                .map_err(|_| source_unavailable("prepared profile directory"))?;
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), target)
                .map_err(|_| source_unavailable("prepared profile object"))?;
        } else {
            return Err(source_invalid("profile staging entry"));
        }
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Problem> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| source_unavailable("prepared profile metadata"))?;
    file.write_all(bytes)
        .map_err(|_| source_unavailable("prepared profile metadata"))?;
    file.sync_all()
        .map_err(|_| source_unavailable("prepared profile metadata"))
}

fn required_env(name: &'static str) -> Result<String, Problem> {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| source_invalid(name))
}

fn source_invalid(field: &'static str) -> Problem {
    Problem::new(
        "migration_legacy_s3_invalid",
        "Legacy S3 migration input is invalid",
        422,
        "migration-s3",
    )
    .with_field("field", field)
}

fn source_unavailable(operation: &'static str) -> Problem {
    Problem::new(
        "migration_legacy_s3_unavailable",
        "Legacy S3 profile data could not be read or staged",
        503,
        "migration-s3",
    )
    .with_field("operation", operation)
}

#[cfg(test)]
mod tests {
    use super::{
        has_profile_payload, safe_file_name, safe_key_segment, safe_relative_path, valid_blake3,
    };
    use std::fs;

    #[test]
    fn rejects_unsafe_manifest_paths_and_hashes() {
        for path in [
            "",
            "../Cookies",
            "/absolute",
            "..\\Cookies",
            "Default\\Cookies",
        ] {
            assert!(!safe_relative_path(path));
        }
        assert!(safe_relative_path("Default/Network/Cookies"));
        assert!(!safe_file_name("NUL"));
        assert!(!safe_file_name("Cookies."));
        assert!(!safe_key_segment("../workspace"));
        assert!(safe_key_segment("ws_abcdef-123"));
        assert!(valid_blake3(&"a".repeat(64)));
        assert!(!valid_blake3("not-a-hash"));
    }

    #[test]
    fn empty_local_profile_directory_is_not_treated_as_payload() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-s3-hydration-empty-test-{}",
            std::process::id()
        ));
        let profile = root.join("profile");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&profile).unwrap();
        assert!(!has_profile_payload(&profile));
        fs::write(profile.join("Preferences"), b"{}").unwrap();
        assert!(has_profile_payload(&profile));
        fs::remove_dir_all(root).unwrap();
    }
}
