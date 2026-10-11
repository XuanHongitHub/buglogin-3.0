use std::path::PathBuf;

use buglogin_api::http::ApiState;
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{entities::Profile, errors::Problem, ids::ProfileId};
use buglogin_migration::{
  activate_package,
  cutover::{confirm_cutover, promote_staged},
  inspect::inspect_source,
  package::{build_encrypted_profile_package, PackageMetadata},
  rollback::{confirm_rollback, rollback_activation, rollback_cutover},
  signature::{SignatureMetadata, sign_package, verify_signed_package},
  transform::stage_package,
  MigrationKey,
};
use buglogin_network::proxy::{parse_lines, ParseReport};
use buglogin_storage::{
  local::{LocalRepository, Versioned},
  secrets::{OsVault, SecretKey},
  vault::NativeVault,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::State;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct V2Health {
  status: &'static str,
  service: &'static str,
  contract_version: &'static str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRevisionInput {
  profile_id: String,
  revision: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDeleteInput {
  profile_id: String,
  revision: u64,
  confirmation: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationImportInput {
  source_root: PathBuf,
  staging_parent: PathBuf,
  destination_root: PathBuf,
  run_id: String,
  confirmation: String,
  key_namespace: String,
  key_name: String,
  signing_key_namespace: String,
  signing_key_name: String,
  signing_key_id: String,
  signer: String,
}

pub fn state(root: PathBuf) -> Result<ApiState, Problem> {
  Ok(ApiState::new(ProfileService::new(LocalRepository::open(
    root,
  )?)))
}

#[tauri::command]
pub fn system_health_get() -> V2Health {
  V2Health {
    status: "ok",
    service: "buglogin-v2",
    contract_version: "1.0",
  }
}

#[tauri::command]
pub fn profile_list(state: State<'_, ApiState>) -> Result<Vec<Versioned<Profile>>, Problem> {
  state.profiles.list()
}

#[tauri::command]
pub fn profile_get(
  state: State<'_, ApiState>,
  profile_id: String,
) -> Result<Versioned<Profile>, Problem> {
  let id = parse_profile_id(&profile_id)?;
  state.profiles.get(&id)?.ok_or_else(profile_not_found)
}

#[tauri::command]
pub fn profile_create(
  state: State<'_, ApiState>,
  profile: Profile,
) -> Result<Versioned<Profile>, Problem> {
  state.profiles.create(profile)
}

#[tauri::command]
pub fn profile_archive(
  state: State<'_, ApiState>,
  input: ProfileRevisionInput,
) -> Result<Versioned<Profile>, Problem> {
  mutate_profile(&state.profiles, input, true)
}

#[tauri::command]
pub fn profile_restore(
  state: State<'_, ApiState>,
  input: ProfileRevisionInput,
) -> Result<Versioned<Profile>, Problem> {
  mutate_profile(&state.profiles, input, false)
}

#[tauri::command]
pub fn profile_delete(
  state: State<'_, ApiState>,
  input: ProfileDeleteInput,
) -> Result<Value, Problem> {
  if input.confirmation != "QUARANTINE_PROFILE_DATA" {
    return Err(Problem::new(
      "delete_not_confirmed",
      "Profile deletion is not confirmed",
      409,
      "profile",
    ));
  }
  let id = parse_profile_id(&input.profile_id)?;
  let current = state.profiles.get(&id)?.ok_or_else(profile_not_found)?;
  let path = state.profiles.delete(input.revision, &current.value)?;
  Ok(json!({ "id": id, "deleted": true, "recoverable": true, "quarantinePath": path }))
}

#[tauri::command]
pub fn proxy_parse(input: String) -> ParseReport {
  parse_lines(&input)
}

#[tauri::command]
pub fn migration_inspect(source_root: PathBuf) -> Result<Value, Problem> {
  let (report, snapshot) = inspect_source(&source_root)?;
  Ok(json!({ "report": report, "snapshot": snapshot }))
}

#[tauri::command]
pub fn migration_import(input: MigrationImportInput) -> Result<Value, Problem> {
  let (report, snapshot) = inspect_source(&input.source_root)?;
  let key_ref = SecretKey::new(input.key_namespace, input.key_name).map_err(|error| {
    Problem::new(
      error.code(),
      "Migration key reference is invalid",
      400,
      "migration",
    )
  })?;
  let vault = NativeVault::new();
  let (secret, _) = vault.read(&key_ref).map_err(|error| {
    Problem::new(
      error.code(),
      "Migration key is unavailable",
      503,
      "migration",
    )
  })?;
  let key_bytes: [u8; 32] = secret.as_bytes().try_into().map_err(|_| {
    Problem::new(
      "migration_key_invalid",
      "Migration key must be 32 bytes",
      503,
      "migration",
    )
  })?;
  let migration_key = MigrationKey::from_bytes(key_bytes);
  let signing_ref = SecretKey::new(input.signing_key_namespace, input.signing_key_name).map_err(
    |error| {
      Problem::new(
        error.code(),
        "Migration signing key reference is invalid",
        400,
        "migration",
      )
    },
  )?;
  let (signing_secret, _) = vault.read(&signing_ref).map_err(|error| {
    Problem::new(
      error.code(),
      "Migration signing key is unavailable",
      503,
      "migration",
    )
  })?;
  let metadata = PackageMetadata {
    created_at: migration_timestamp()?,
    tool_version: env!("CARGO_PKG_VERSION").to_owned(),
    tool_commit: option_env!("GIT_COMMIT").unwrap_or("unknown").to_owned(),
  };
  let package = build_encrypted_profile_package(
    &input.source_root,
    &report,
    &snapshot,
    &metadata,
    &migration_key,
  )?;
  let package = sign_package(
    &package,
    signing_secret.as_bytes(),
    SignatureMetadata {
      key_id: input.signing_key_id,
      signer: input.signer,
      signed_at: metadata.created_at.clone(),
    },
  )?;
  verify_signed_package(&package, signing_secret.as_bytes())?;
  let staged = stage_package(&package, &input.staging_parent, &input.run_id)?;
  let receipt = promote_staged(
    &staged,
    &input.destination_root,
    confirm_cutover(&input.confirmation)?,
  )?;
  let activation = match activate_package(
    &input.destination_root,
    &input.run_id,
    &package,
    Some(&migration_key),
  ) {
    Ok(value) => value,
    Err(error) => {
      let confirmation = confirm_rollback("ROLLBACK_MIGRATION_RUN")?;
      let _ = rollback_activation(&input.destination_root, &input.run_id, confirmation.clone());
      let _ = rollback_cutover(&input.destination_root, &input.run_id, confirmation);
      return Err(error);
    }
  };
  Ok(json!({
    "runId": receipt.run_id,
    "packageId": receipt.package_id,
    "destinationRoot": receipt.destination_root,
    "runRoot": receipt.run_root,
    "artifacts": receipt.artifacts,
    "createdProfiles": activation.created_profiles,
    "reusedProfiles": activation.reused_profiles
  }))
}

fn parse_profile_id(value: &str) -> Result<ProfileId, Problem> {
  ProfileId::from_str(value)
    .map_err(|_| Problem::new("invalid_profile_id", "Invalid profile ID", 400, "profile"))
}

fn profile_not_found() -> Problem {
  Problem::new("profile_not_found", "Profile not found", 404, "profile")
}

fn mutate_profile(
  profiles: &ProfileService,
  input: ProfileRevisionInput,
  archive: bool,
) -> Result<Versioned<Profile>, Problem> {
  let current = profiles
    .get(&parse_profile_id(&input.profile_id)?)?
    .ok_or_else(profile_not_found)?;
  if archive {
    profiles.archive(input.revision, current.value)
  } else {
    profiles.restore(input.revision, current.value)
  }
}

fn migration_timestamp() -> Result<String, Problem> {
  time::OffsetDateTime::now_utc()
    .format(&time::format_description::well_known::Rfc3339)
    .map_err(|_| {
      Problem::new(
        "migration_timestamp_invalid",
        "Migration timestamp is invalid",
        500,
        "migration",
      )
    })
}

#[cfg(test)]
mod tests {
  use super::*;
  use buglogin_migration::{
    activate_package,
    encrypted_data::decrypt_packaged_profile_data,
    package::{build_encrypted_profile_package, PackageMetadata},
    rollback::{confirm_rollback, rollback_activation},
    snapshot::create_snapshot,
    MigrationKey,
  };
  use buglogin_storage::{
    secrets::{OsVault, SecretKey, SecretValue},
    vault::NativeVault,
  };
  use std::fs;
  use tempfile::tempdir;

  fn fixture_source() -> (tempfile::TempDir, PathBuf) {
    let directory = tempdir().unwrap();
    let source = directory.path().join("source");
    fs::create_dir_all(source.join("profiles/00000000-0000-0000-0000-000000000001/profile"))
      .unwrap();
    fs::write(
      source.join("version.json"),
      r#"{"system":"buglogin-legacy","version":"1.1.84","instanceId":"tauri-migrate-04","os":"windows","architecture":"x86_64"}"#,
    )
    .unwrap();
    fs::write(
      source.join("profiles/00000000-0000-0000-0000-000000000001/metadata.json"),
      r#"{"id":"00000000-0000-0000-0000-000000000001","name":"Encrypted","runtime":"wayfern"}"#,
    )
    .unwrap();
    fs::write(
      source.join("profiles/00000000-0000-0000-0000-000000000001/profile/secret.txt"),
      b"migration-secret-canary",
    )
    .unwrap();
    (directory, source)
  }

  fn key_from_secret(value: &SecretValue) -> Result<MigrationKey, &'static str> {
    let bytes: [u8; 32] = value
      .as_bytes()
      .try_into()
      .map_err(|_| "invalid key length")?;
    Ok(MigrationKey::from_bytes(bytes))
  }

  #[test]
  fn v2_commands_use_real_services() {
    assert_eq!(system_health_get().status, "ok");
    assert_eq!(proxy_parse("127.0.0.1:8080".to_owned()).rows.len(), 1);
    let api = state(tempdir().unwrap().path().to_path_buf()).unwrap();
    let profile = serde_json::from_value::<Profile>(json!({
      "id": "profile-test", "name": "Test", "runtime": "wayfern",
      "runtimeVersion": null, "state": "created", "archived": false,
      "fingerprint": null, "proxyId": null, "vpnId": null, "groupIds": [],
      "tags": [], "extensionGroupId": null, "syncMode": "local",
      "createdAt": "2026-07-20T00:00:00Z", "updatedAt": "2026-07-20T00:00:00Z"
    }))
    .unwrap();
    api.profiles.create(profile).unwrap();
    assert_eq!(api.profiles.list().unwrap().len(), 1);
  }

  #[test]
  fn tauri_migrate_04_rejects_non_32_byte_vault_keys() {
    assert!(key_from_secret(&SecretValue::new([7; 31].to_vec()).unwrap()).is_err());
    assert!(key_from_secret(&SecretValue::new([7; 33].to_vec()).unwrap()).is_err());
    assert!(key_from_secret(&SecretValue::new([7; 32].to_vec()).unwrap()).is_ok());
  }

  #[cfg(any(windows, target_os = "macos"))]
  #[test]
  fn tauri_migrate_04_uses_native_vault_key_reference_without_leakage() {
    let mut vault = NativeVault::new();
    let key = SecretKey::new(
      "migration",
      format!("tauri-migrate-04-{}", std::process::id()),
    )
    .unwrap();
    let version = vault
      .create(&key, SecretValue::new([17; 32].to_vec()).unwrap())
      .unwrap();
    let (stored, stored_version) = vault.read(&key).unwrap();
    assert_eq!(stored_version, version);
    assert_eq!(format!("{stored:?}"), "SecretValue(\"<redacted>\")");
    assert_eq!(
      format!("{:?}", key),
      "SecretKey { namespace: \"migration\", name: \"tauri-migrate-04-".to_owned()
        + &std::process::id().to_string()
        + "\" }"
    );
    assert!(key_from_secret(&stored).is_ok());
    assert_eq!(vault.remove(&key, version), Ok(()));
  }

  #[cfg(not(any(windows, target_os = "macos")))]
  #[test]
  fn tauri_migrate_04_native_vault_fails_closed() {
    let mut vault = NativeVault::new();
    let key = SecretKey::new("migration", "tauri-migrate-04").unwrap();
    assert!(vault
      .create(&key, SecretValue::new([17; 32].to_vec()).unwrap())
      .is_err());
  }

  #[test]
  fn tauri_migrate_04_encrypts_rolls_back_and_preserves_source() {
    let (_directory, source) = fixture_source();
    let before = create_snapshot(&source).unwrap();
    let (report, snapshot) = inspect_source(&source).unwrap();
    let migration_key = MigrationKey::from_bytes([23; 32]);
    let package = build_encrypted_profile_package(
      &source,
      &report,
      &snapshot,
      &PackageMetadata {
        created_at: "2026-07-21T00:00:00Z".to_owned(),
        tool_version: env!("CARGO_PKG_VERSION").to_owned(),
        tool_commit: "test".to_owned(),
      },
      &migration_key,
    )
    .unwrap();
    let package_bytes = package
      .files()
      .values()
      .flat_map(|bytes| bytes.iter())
      .copied()
      .collect::<Vec<_>>();
    let package_text = String::from_utf8_lossy(&package_bytes);
    assert!(!package_text.contains("migration-secret-canary"));
    assert!(decrypt_packaged_profile_data(&package, &migration_key).is_ok());
    let wrong_key_error =
      decrypt_packaged_profile_data(&package, &MigrationKey::from_bytes([24; 32])).unwrap_err();
    let error_text = format!("{wrong_key_error:?}");
    assert!(!error_text.contains("migration-secret-canary"));
    assert!(!error_text.contains("ClientKey"));

    let destination_directory = tempdir().unwrap();
    let receipt = activate_package(
      destination_directory.path(),
      "tauri-migrate-04",
      &package,
      Some(&migration_key),
    )
    .unwrap();
    assert_eq!(receipt.created_profiles, 1);
    let rollback = rollback_activation(
      destination_directory.path(),
      "tauri-migrate-04",
      confirm_rollback("ROLLBACK_MIGRATION_RUN").unwrap(),
    )
    .unwrap();
    assert_eq!(rollback.quarantined_profiles.len(), 1);
    assert_eq!(create_snapshot(&source).unwrap(), before);
    assert!(receipt.journal_path.exists());
  }
}
