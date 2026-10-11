use buglogin_domain::errors::Problem;
use buglogin_domain::state::{ImportState, ProfileLifecycle, RuntimeInstallLifecycle, SyncState};

fn assert_invalid<T>(result: Result<T, Problem>, from: &str, to: &str) {
    let error = match result {
        Ok(_) => panic!("transition must be rejected"),
        Err(error) => error,
    };
    assert_eq!(error.code, "invalid_state_transition");
    assert_eq!(error.status, 409);
    assert_eq!(error.fields["from"], from);
    assert_eq!(error.fields["to"], to);
}

#[test]
fn profile_follows_safe_lifecycle_and_recovery_branch() {
    let state = ProfileLifecycle::Created
        .transition(ProfileLifecycle::Ready)
        .expect("created to ready")
        .transition(ProfileLifecycle::Launching)
        .expect("ready to launching")
        .transition(ProfileLifecycle::Running)
        .expect("launching to running")
        .transition(ProfileLifecycle::Stopping)
        .expect("running to stopping")
        .transition(ProfileLifecycle::Ready)
        .expect("stopping to ready");
    assert_eq!(state, ProfileLifecycle::Ready);
    assert_invalid(
        ProfileLifecycle::Created.transition(ProfileLifecycle::Running),
        "created",
        "running",
    );
    assert_invalid(
        ProfileLifecycle::Ready.transition(ProfileLifecycle::Ready),
        "ready",
        "ready",
    );
    assert_eq!(
        ProfileLifecycle::Failed
            .transition(ProfileLifecycle::Recovering)
            .expect("failed to recovering")
            .transition(ProfileLifecycle::Ready)
            .expect("recovering to ready"),
        ProfileLifecycle::Ready
    );
}

#[test]
fn runtime_install_rejects_skips_and_terminal_reentry() {
    let state = RuntimeInstallLifecycle::Queued
        .transition(RuntimeInstallLifecycle::Downloading)
        .expect("queued to downloading")
        .transition(RuntimeInstallLifecycle::Verifying)
        .expect("downloading to verifying")
        .transition(RuntimeInstallLifecycle::Installing)
        .expect("verifying to installing")
        .transition(RuntimeInstallLifecycle::Ready)
        .expect("installing to ready");
    assert_eq!(state, RuntimeInstallLifecycle::Ready);
    assert_invalid(
        RuntimeInstallLifecycle::Queued.transition(RuntimeInstallLifecycle::Ready),
        "queued",
        "ready",
    );
    assert_invalid(
        RuntimeInstallLifecycle::Ready.transition(RuntimeInstallLifecycle::Downloading),
        "ready",
        "downloading",
    );
}

#[test]
fn sync_and_import_allow_recovery_without_implicit_jumps() {
    assert_eq!(
        SyncState::Idle
            .transition(SyncState::Checking)
            .expect("idle to checking")
            .transition(SyncState::Conflict)
            .expect("checking to conflict")
            .transition(SyncState::Checking)
            .expect("conflict resolution retry")
            .transition(SyncState::Downloading)
            .expect("checking to downloading")
            .transition(SyncState::Completed)
            .expect("downloading to completed"),
        SyncState::Completed
    );
    assert_invalid(
        SyncState::Idle.transition(SyncState::Completed),
        "idle",
        "completed",
    );

    assert_eq!(
        ImportState::Detected
            .transition(ImportState::Previewed)
            .expect("detected to previewed")
            .transition(ImportState::Copying)
            .expect("previewed to copying")
            .transition(ImportState::Validating)
            .expect("copying to validating")
            .transition(ImportState::Partial)
            .expect("validating to partial"),
        ImportState::Partial
    );
    assert_invalid(
        ImportState::Detected.transition(ImportState::Completed),
        "detected",
        "completed",
    );
}

#[test]
fn problem_serializes_to_canonical_shape() {
    let problem = Problem::new("profile_not_ready", "Profile not ready", 409, "cor_123")
        .with_detail("Profile must be ready before launch")
        .with_instance("req_123")
        .with_field("profileId", "prf_123")
        .retryable(false);
    let value = serde_json::to_value(problem).expect("serialize problem");
    assert_eq!(
        value["type"],
        "https://buglogin.local/problems/profile_not_ready"
    );
    assert_eq!(value["status"], 409);
    assert_eq!(value["code"], "profile_not_ready");
    assert_eq!(value["correlationId"], "cor_123");
    assert_eq!(value["fields"]["profileId"], "prf_123");
}
