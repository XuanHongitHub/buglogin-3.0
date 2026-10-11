use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, RwLock},
};

use buglogin_browser::{
    lifecycle::Lifecycle,
    process::{ApprovedExecutable, ProcessController, SystemProcessController},
    profile::ProfileService,
    runtime::{RuntimeAdapter, RuntimeBinary, bugmini::BugminiAdapter, wayfern::WayfernAdapter},
};
use buglogin_domain::{
    entities::{ProfileState, Runtime},
    errors::Problem,
    ids::ProfileId,
};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct ProfileLifecycleCommand {
    pub profile_id: ProfileId,
    pub profiles_root: PathBuf,
    pub extra_args: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ProfileLifecycleService;

impl ProfileLifecycleService {
    pub fn launch(
        &self,
        profiles: &ProfileService,
        lifecycles: &Arc<RwLock<HashMap<String, Lifecycle>>>,
        runtime_binaries: &Arc<RwLock<HashMap<String, RuntimeBinary>>>,
        command: ProfileLifecycleCommand,
    ) -> Result<Value, Problem> {
        let current = profiles
            .get(&command.profile_id)?
            .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
        if current.value.archived || current.value.state == ProfileState::Running {
            return Err(problem(
                "profile_not_launchable",
                "Profile cannot be launched",
                409,
            ));
        }
        let mut active_lifecycles = lifecycles.write().map_err(|_| {
            problem(
                "lifecycle_unavailable",
                "Lifecycle service unavailable",
                503,
            )
        })?;
        if active_lifecycles.contains_key(command.profile_id.as_str()) {
            return Err(problem(
                "profile_not_launchable",
                "Profile cannot be launched",
                409,
            ));
        }

        let binary = runtime_binaries
            .read()
            .map_err(|_| {
                problem(
                    "runtime_catalog_unavailable",
                    "Runtime catalog unavailable",
                    503,
                )
            })?
            .get(&runtime_key(&current.value.runtime))
            .cloned()
            .ok_or_else(|| {
                problem(
                    "runtime_not_approved",
                    "No approved runtime is configured",
                    503,
                )
            })?;
        let (validated, specification) = match current.value.runtime {
            Runtime::Wayfern => {
                let adapter = WayfernAdapter;
                let validated = adapter.validate_binary(&binary)?;
                let profile_path =
                    adapter.profile_path(&command.profiles_root, &command.profile_id)?;
                let specification =
                    adapter.launch_specification(&validated, &profile_path, &command.extra_args)?;
                (validated, specification)
            }
            Runtime::Bugmini => {
                let adapter = BugminiAdapter;
                let validated = adapter.validate_binary(&binary)?;
                let profile_path =
                    adapter.profile_path(&command.profiles_root, &command.profile_id)?;
                let specification =
                    adapter.launch_specification(&validated, &profile_path, &command.extra_args)?;
                (validated, specification)
            }
            _ => {
                return Err(problem(
                    "runtime_not_supported",
                    "Runtime is not supported",
                    422,
                ));
            }
        };
        let approved = ApprovedExecutable::new(&specification.executable)?;
        let controller = SystemProcessController::new([approved]);
        let process = controller.launch(&specification)?;
        let mut lifecycle = Lifecycle::new();
        lifecycle.attach(process)?;

        let mut updated_profile = current.value;
        updated_profile.state = ProfileState::Running;
        updated_profile.runtime_version = Some(validated.version);
        let updated = match profiles.update(current.revision, updated_profile) {
            Ok(value) => value,
            Err(error) => {
                let _ = lifecycle.stop();
                return Err(error);
            }
        };
        active_lifecycles.insert(command.profile_id.as_str().to_owned(), lifecycle);

        Ok(json!({
            "profileId": command.profile_id,
            "revision": updated.revision,
            "processId": "owned"
        }))
    }

    pub fn stop(
        &self,
        profiles: &ProfileService,
        lifecycles: &Arc<RwLock<HashMap<String, Lifecycle>>>,
        profile_id: ProfileId,
    ) -> Result<Value, Problem> {
        let current = profiles
            .get(&profile_id)?
            .ok_or_else(|| problem("profile_not_found", "Profile not found", 404))?;
        let mut lifecycle = lifecycles
            .write()
            .map_err(|_| {
                problem(
                    "lifecycle_unavailable",
                    "Lifecycle service unavailable",
                    503,
                )
            })?
            .remove(profile_id.as_str())
            .ok_or_else(|| problem("profile_not_running", "Profile is not running", 409))?;
        if let Err(error) = lifecycle.stop() {
            let _ = lifecycles
                .write()
                .map(|mut values| values.insert(profile_id.as_str().to_owned(), lifecycle));
            return Err(error);
        }
        let mut updated_profile = current.value;
        updated_profile.state = ProfileState::Ready;
        let updated = profiles.update(current.revision, updated_profile)?;
        Ok(json!({ "profileId": profile_id, "revision": updated.revision }))
    }
}

fn runtime_key(runtime: &Runtime) -> String {
    serde_json::to_string(runtime).unwrap_or_else(|_| "unknown".to_owned())
}

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "api")
}
