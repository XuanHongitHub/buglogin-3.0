use std::path::PathBuf;

use buglogin_api::http::ApiState;
use buglogin_browser::profile::ProfileService;
use buglogin_domain::{entities::Profile, errors::Problem, ids::ProfileId};
use buglogin_network::proxy::{parse_lines, ParseReport};
use buglogin_storage::local::{LocalRepository, Versioned};
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
