use std::{
    collections::{BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use buglogin_browser::{
    lifecycle::{LifecycleManager, LifecycleState},
    process::ProcessController,
    runtime::LaunchSpecification,
};
use buglogin_domain::{
    entities::Runtime,
    errors::Problem,
    ids::{ProfileId, ResourceId, WorkspaceId},
};
use buglogin_sync::coordination::{
    CoordinationAuth, CoordinationBackend, CoordinationScope, LeaseGrant,
    lease_release_idempotency_key,
};
use buglogin_sync::manifest::SyncManifest;
use tokio::sync::{mpsc, oneshot};

use crate::clock::unix_time_millis;
use crate::proxy_bridge::ProxyBridgeHandle;

pub const PROFILE_LEASE_TTL: Duration = Duration::from_secs(60);
const MAX_STOP_SYNC_HOLD: Duration = Duration::from_secs(5 * 60);

#[derive(Clone)]
pub struct ManagedCoordination {
    pub backend: Arc<dyn DesktopSyncBackend>,
    pub auth: CoordinationAuth,
    pub lease_ttl: Duration,
    pub heartbeat_interval: Duration,
}

#[async_trait::async_trait]
pub trait DesktopSyncBackend: Send + Sync {
    async fn acquire_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem>;
    async fn renew_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem>;
    async fn release_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
    ) -> Result<(), Problem> {
        let idempotency_key = lease_release_idempotency_key(&scope, resource_id, owner_token);
        self.release_lease_idempotent(scope, resource_id, owner_token, &idempotency_key)
            .await
    }
    async fn release_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        idempotency_key: &str,
    ) -> Result<(), Problem>;
    async fn current_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
    ) -> Result<Option<SyncManifest>, Problem>;
    async fn commit_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        expected_profile_revision: Option<u64>,
        manifest: SyncManifest,
    ) -> Result<(), Problem>;
}

#[async_trait::async_trait]
impl<T: CoordinationBackend + ?Sized> DesktopSyncBackend for T {
    async fn acquire_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        CoordinationBackend::acquire_lease_idempotent(
            self,
            scope,
            resource_id,
            ttl,
            idempotency_key,
        )
        .await
    }

    async fn renew_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
        idempotency_key: &str,
    ) -> Result<LeaseGrant, Problem> {
        CoordinationBackend::renew_lease_idempotent(
            self,
            scope,
            resource_id,
            owner_token,
            ttl,
            idempotency_key,
        )
        .await
    }

    async fn release_lease_idempotent(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        idempotency_key: &str,
    ) -> Result<(), Problem> {
        CoordinationBackend::release_lease_idempotent(
            self,
            scope,
            resource_id,
            owner_token,
            idempotency_key,
        )
        .await
    }

    async fn current_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
    ) -> Result<Option<SyncManifest>, Problem> {
        CoordinationBackend::current_manifest(self, scope, resource_id).await
    }

    async fn commit_manifest(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        _expected_profile_revision: Option<u64>,
        manifest: SyncManifest,
    ) -> Result<(), Problem> {
        CoordinationBackend::commit_manifest(self, scope, resource_id, owner_token, manifest).await
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LeaseScope {
    pub workspace_id: WorkspaceId,
    pub resource_id: ResourceId,
}

pub struct LaunchRequest {
    pub profile_id: ProfileId,
    pub idempotency_key: String,
    pub profile_path: PathBuf,
    pub specification: LaunchSpecification,
    pub lease_scope: Option<LeaseScope>,
    pub preacquired_lease: Option<LeaseGrant>,
    pub proxy_bridge: Option<ProxyBridgeHandle>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WriteAuthority {
    pub workspace_id: WorkspaceId,
    pub resource_id: ResourceId,
    pub fencing_token: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchReceipt {
    pub process_id: u32,
    pub write_authority: Option<WriteAuthority>,
    pub session_epoch: u64,
}

#[derive(Clone)]
pub struct StopReceipt {
    pub(crate) profile_path: PathBuf,
    pub(crate) lease: Option<LeaseGrant>,
    pub(crate) was_launched: bool,
    pub(crate) session_epoch: u64,
}

#[derive(Clone)]
pub struct ProfileRuntimeHandle {
    commands: mpsc::Sender<RuntimeCommand>,
}

impl ProfileRuntimeHandle {
    pub async fn launch(&self, request: LaunchRequest) -> Result<LaunchReceipt, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::Launch { request, reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn stop(&self, profile_id: ProfileId) -> Result<(), Problem> {
        match self.stop_for_sync(profile_id.clone()).await {
            Ok(_) => self.complete_stop(profile_id).await,
            Err(error) if error.code == "profile_not_running" => {
                self.complete_stop(profile_id).await
            }
            Err(error) => Err(error),
        }
    }

    pub async fn stop_for_sync(&self, profile_id: ProfileId) -> Result<StopReceipt, Problem> {
        self.send_stop_for_session(profile_id, None).await
    }

    pub async fn stop_for_session(
        &self,
        profile_id: ProfileId,
        session_epoch: u64,
    ) -> Result<StopReceipt, Problem> {
        self.send_stop_for_session(profile_id, Some(session_epoch))
            .await
    }

    async fn send_stop_for_session(
        &self,
        profile_id: ProfileId,
        session_epoch: Option<u64>,
    ) -> Result<StopReceipt, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::StopForSync {
                profile_id,
                session_epoch,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn set_sync_attempt_active(
        &self,
        profile_id: ProfileId,
        active: bool,
    ) -> Result<(), Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::SetSyncAttemptActive {
                profile_id,
                active,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn sync_attempt_active(&self, profile_id: ProfileId) -> Result<bool, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::SyncAttemptActive { profile_id, reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())
    }

    pub async fn hold_idle_profile_for_sync(
        &self,
        profile_id: ProfileId,
        profile_path: PathBuf,
        lease_scope: LeaseScope,
        lease: LeaseGrant,
    ) -> Result<StopReceipt, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::HoldIdleProfileForSync {
                profile_id,
                profile_path,
                lease_scope,
                lease,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn complete_stop(&self, profile_id: ProfileId) -> Result<(), Problem> {
        self.send_complete_stop_for_session(profile_id, None).await
    }

    pub async fn complete_stop_for_session(
        &self,
        profile_id: ProfileId,
        session_epoch: u64,
    ) -> Result<(), Problem> {
        self.send_complete_stop_for_session(profile_id, Some(session_epoch))
            .await
    }

    async fn send_complete_stop_for_session(
        &self,
        profile_id: ProfileId,
        session_epoch: Option<u64>,
    ) -> Result<(), Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::CompleteStop {
                profile_id,
                session_epoch,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn complete_stop_detached(&self, profile_id: ProfileId) -> Result<(), Problem> {
        self.send_complete_stop_detached_for_session(profile_id, None)
            .await
    }

    pub async fn complete_stop_detached_for_session(
        &self,
        profile_id: ProfileId,
        session_epoch: u64,
    ) -> Result<(), Problem> {
        self.send_complete_stop_detached_for_session(profile_id, Some(session_epoch))
            .await
    }

    async fn send_complete_stop_detached_for_session(
        &self,
        profile_id: ProfileId,
        session_epoch: Option<u64>,
    ) -> Result<(), Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::CompleteStopDetached {
                profile_id,
                session_epoch,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())
    }

    pub async fn release_stopped_for_retry(&self, profile_id: ProfileId) -> Result<(), Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::ReleaseStoppedForRetry { profile_id, reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn take_stopped_lease_for_reopen(
        &self,
        profile_id: ProfileId,
    ) -> Result<Option<LeaseGrant>, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::TakeStoppedLeaseForReopen { profile_id, reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn finish_sync_result<T>(
        &self,
        profile_id: ProfileId,
        result: Result<T, Problem>,
    ) -> Result<T, Problem> {
        if result.is_err() {
            // Preserve retry intent and local files, but release the stopped
            // runtime entry and lease so a failed cloud sync cannot block the
            // profile lifecycle.
            self.queue_sync_retry(profile_id.clone()).await;
            let _ = self.release_stopped_for_retry(profile_id.clone()).await;
        }
        result
    }

    pub async fn queue_sync_retry(&self, profile_id: ProfileId) {
        let (reply, response) = oneshot::channel();
        if self
            .commands
            .send(RuntimeCommand::QueueSyncRetry { profile_id, reply })
            .await
            .is_ok()
        {
            let _ = response.await;
        }
    }

    pub async fn write_authority(
        &self,
        profile_id: ProfileId,
    ) -> Result<Option<WriteAuthority>, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::Authority { profile_id, reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())
    }

    pub async fn running_profiles(&self) -> Result<Vec<ProfileId>, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::RunningProfiles { reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())
    }

    pub async fn active_profiles(&self) -> Result<Vec<ProfileId>, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::ActiveProfiles { reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())
    }

    pub async fn process_state_for_path(
        &self,
        profile_path: PathBuf,
    ) -> Result<LifecycleState, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::ProcessStateForPath {
                profile_path,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn unexpectedly_stopped_profiles(&self) -> Result<Vec<ProfileId>, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::UnexpectedlyStoppedProfiles { reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())
    }

    pub async fn set_coordination(
        &self,
        coordination: Option<ManagedCoordination>,
    ) -> Result<(), Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::SetCoordination {
                coordination,
                reply,
            })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn approve_executable(&self, path: PathBuf) -> Result<(), Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::ApproveExecutable { path, reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn shutdown(&self) -> Result<(), Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::Shutdown { reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }
}

enum RuntimeCommand {
    Launch {
        request: LaunchRequest,
        reply: oneshot::Sender<Result<LaunchReceipt, Problem>>,
    },
    LaunchAcquired {
        request: LaunchRequest,
        session_epoch: u64,
        lease_result: Result<Option<LeaseGrant>, Problem>,
        coordination: Option<ManagedCoordination>,
        reply: oneshot::Sender<Result<LaunchReceipt, Problem>>,
    },
    LaunchCleanupComplete {
        profile_id: ProfileId,
        session_epoch: u64,
        error: Problem,
        reply: oneshot::Sender<Result<LaunchReceipt, Problem>>,
    },
    StopForSync {
        profile_id: ProfileId,
        session_epoch: Option<u64>,
        reply: oneshot::Sender<Result<StopReceipt, Problem>>,
    },
    HoldIdleProfileForSync {
        profile_id: ProfileId,
        profile_path: PathBuf,
        lease_scope: LeaseScope,
        lease: LeaseGrant,
        reply: oneshot::Sender<Result<StopReceipt, Problem>>,
    },
    CompleteStop {
        profile_id: ProfileId,
        session_epoch: Option<u64>,
        reply: oneshot::Sender<Result<(), Problem>>,
    },
    CompleteStopDetached {
        profile_id: ProfileId,
        session_epoch: Option<u64>,
        reply: oneshot::Sender<()>,
    },
    ReleaseStoppedForRetry {
        profile_id: ProfileId,
        reply: oneshot::Sender<Result<(), Problem>>,
    },
    TakeStoppedLeaseForReopen {
        profile_id: ProfileId,
        reply: oneshot::Sender<Result<Option<LeaseGrant>, Problem>>,
    },
    SetSyncAttemptActive {
        profile_id: ProfileId,
        active: bool,
        reply: oneshot::Sender<Result<(), Problem>>,
    },
    SyncAttemptActive {
        profile_id: ProfileId,
        reply: oneshot::Sender<bool>,
    },
    Authority {
        profile_id: ProfileId,
        reply: oneshot::Sender<Option<WriteAuthority>>,
    },
    RunningProfiles {
        reply: oneshot::Sender<Vec<ProfileId>>,
    },
    ActiveProfiles {
        reply: oneshot::Sender<Vec<ProfileId>>,
    },
    ProcessStateForPath {
        profile_path: PathBuf,
        reply: oneshot::Sender<Result<LifecycleState, Problem>>,
    },
    UnexpectedlyStoppedProfiles {
        reply: oneshot::Sender<Vec<ProfileId>>,
    },
    QueueSyncRetry {
        profile_id: ProfileId,
        reply: oneshot::Sender<()>,
    },
    SetCoordination {
        coordination: Option<ManagedCoordination>,
        reply: oneshot::Sender<Result<(), Problem>>,
    },
    ApproveExecutable {
        path: PathBuf,
        reply: oneshot::Sender<Result<(), Problem>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), Problem>>,
    },
    RenewalCompleted {
        profile_id: ProfileId,
        session_epoch: u64,
        lease: LeaseGrant,
        result: Result<LeaseGrant, Problem>,
    },
}

struct RunningProfile {
    profile_path: PathBuf,
    session_epoch: u64,
    lease: Option<LeaseGrant>,
    was_launched: bool,
    process_stopped: bool,
    sync_pending: bool,
    sync_attempt_active: bool,
    sync_started_at: Option<Instant>,
    proxy_bridge: Option<ProxyBridgeHandle>,
}

pub fn spawn_profile_runtime<C>(
    controller: C,
    coordination: Option<ManagedCoordination>,
) -> ProfileRuntimeHandle
where
    C: ProcessController + 'static,
{
    let (commands, receiver) = mpsc::channel(32);
    tauri::async_runtime::spawn(run(ProfileRuntime {
        lifecycle: LifecycleManager::new(controller),
        coordination,
        running: HashMap::new(),
        unexpectedly_stopped: BTreeSet::new(),
        pending_renewals: HashMap::new(),
        launching: HashMap::new(),
        cancelled_launches: BTreeSet::new(),
        next_session_epoch: 1,
        commands: commands.clone(),
        receiver,
    }));
    ProfileRuntimeHandle { commands }
}

struct ProfileRuntime<C: ProcessController> {
    lifecycle: LifecycleManager<C>,
    coordination: Option<ManagedCoordination>,
    running: HashMap<ProfileId, RunningProfile>,
    unexpectedly_stopped: BTreeSet<ProfileId>,
    pending_renewals: HashMap<ProfileId, (u64, u64, String)>,
    launching: HashMap<ProfileId, u64>,
    cancelled_launches: BTreeSet<(ProfileId, u64)>,
    next_session_epoch: u64,
    commands: mpsc::Sender<RuntimeCommand>,
    receiver: mpsc::Receiver<RuntimeCommand>,
}

async fn run<C>(mut runtime: ProfileRuntime<C>)
where
    C: ProcessController + 'static,
{
    let mut interval = tokio::time::interval(runtime.heartbeat_interval());
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            command = runtime.receiver.recv() => match command {
                Some(RuntimeCommand::Launch { request, reply }) => {
                    let session_epoch = match runtime.reserve_launch(&request.profile_id) {
                        Ok(session_epoch) => session_epoch,
                        Err(error) => {
                            let _ = reply.send(Err(error));
                            continue;
                        }
                    };
                    let coordination = runtime.coordination.clone();
                    let sender = runtime.commands.clone();
                    tauri::async_runtime::spawn(async move {
                        let mut request = request;
                        let lease_result = match request.preacquired_lease.take() {
                            Some(lease) => validate_preacquired(
                                coordination.as_ref(),
                                request.lease_scope.as_ref(),
                                &lease,
                            )
                            .map(|()| Some(lease)),
                            None => {
                                acquire(
                                    coordination.clone(),
                                    request.lease_scope.as_ref(),
                                    &request.idempotency_key,
                                )
                                .await
                            }
                        };
                        let command = RuntimeCommand::LaunchAcquired {
                            request,
                            session_epoch,
                            lease_result,
                            coordination,
                            reply,
                        };
                        if let Err(error) = sender.send(command).await {
                            if let RuntimeCommand::LaunchAcquired {
                                mut request,
                                session_epoch: _,
                                lease_result,
                                coordination,
                                ..
                            } = error.0
                            {
                                if let Some(bridge) = request.proxy_bridge.take() {
                                    let _ = bridge.stop().await;
                                }
                                if let Ok(Some(lease)) = lease_result {
                                    let _ = release(coordination, &lease).await;
                                }
                            }
                        }
                    });
                }
                Some(RuntimeCommand::LaunchAcquired {
                    request,
                    session_epoch,
                    lease_result,
                    coordination,
                    reply,
                }) => {
                    let launch_is_current = runtime.launching.get(&request.profile_id)
                        == Some(&session_epoch);
                    let launch_profile_id = request.profile_id.clone();
                    let launch_was_cancelled = runtime
                        .cancelled_launches
                        .remove(&(request.profile_id.clone(), session_epoch));
                    let result = match (launch_is_current, launch_was_cancelled, lease_result) {
                        (true, false, Ok(lease)) => {
                            runtime.launch_process(request, lease, session_epoch)
                        }
                        (true, false, Err(error)) => Err((request, None, error)),
                        (true, true, lease_result) => Err((
                            request,
                            lease_result.ok().flatten(),
                            Problem::new(
                                "profile_launch_cancelled",
                                "Profile launch was cancelled before the browser started",
                                409,
                                "desktop",
                            ),
                        )),
                        (false, _, lease_result) => Err((
                            request,
                            lease_result.ok().flatten(),
                            Problem::new(
                                "profile_launch_superseded",
                                "Profile launch was superseded by a newer runtime session",
                                409,
                                "desktop",
                            ),
                        )),
                    };
                    match result {
                        Ok(receipt) => {
                            if runtime.launching.get(&launch_profile_id) == Some(&session_epoch) {
                                runtime.launching.remove(&launch_profile_id);
                            }
                            let _ = reply.send(Ok(receipt));
                        }
                        Err((mut request, lease, error)) => {
                            let profile_id = request.profile_id.clone();
                            let sender = runtime.commands.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Some(bridge) = request.proxy_bridge.take() {
                                    let _ = bridge.stop().await;
                                }
                                if let Some(lease) = lease.as_ref() {
                                    let _ = release(coordination, lease).await;
                                }
                                let _ = sender
                                    .send(RuntimeCommand::LaunchCleanupComplete {
                                        profile_id,
                                        session_epoch,
                                        error,
                                        reply,
                                    })
                                    .await;
                            });
                        }
                    }
                }
                Some(RuntimeCommand::LaunchCleanupComplete {
                    profile_id,
                    session_epoch,
                    error,
                    reply,
                }) => {
                    if runtime.launching.get(&profile_id) == Some(&session_epoch) {
                        runtime.launching.remove(&profile_id);
                        runtime.cancelled_launches.remove(&(profile_id, session_epoch));
                    }
                    let _ = reply.send(Err(error));
                }
                Some(RuntimeCommand::StopForSync {
                    profile_id,
                    session_epoch,
                    reply,
                }) => {
                    let _ = reply.send(
                        runtime
                            .stop_for_session(&profile_id, session_epoch)
                            .await,
                    );
                }
                Some(RuntimeCommand::HoldIdleProfileForSync {
                    profile_id,
                    profile_path,
                    lease_scope,
                    lease,
                    reply,
                }) => {
                    let _ = reply.send(runtime.hold_idle_profile_for_sync(
                        profile_id,
                        profile_path,
                        lease_scope,
                        lease,
                    ));
                }
                Some(RuntimeCommand::CompleteStop {
                    profile_id,
                    session_epoch,
                    reply,
                }) => {
                    match runtime.begin_complete_stop(&profile_id, session_epoch) {
                        Err(error) => {
                            let _ = reply.send(Err(error));
                        }
                        Ok(None) => {
                            let _ = reply.send(Ok(()));
                        }
                        Ok(Some(lease)) => {
                            let coordination = runtime.coordination.clone();
                            tauri::async_runtime::spawn(async move {
                                let result = match release(coordination, &lease).await {
                                    Err(error) if !lease_ownership_lost(&error) => Err(error),
                                    _ => Ok(()),
                                };
                                let _ = reply.send(result);
                            });
                        }
                    }
                }
                Some(RuntimeCommand::CompleteStopDetached {
                    profile_id,
                    session_epoch,
                    reply,
                }) => {
                    runtime.complete_stop_detached(&profile_id, session_epoch);
                    let _ = reply.send(());
                }
                Some(RuntimeCommand::ReleaseStoppedForRetry { profile_id, reply }) => {
                    let _ = reply.send(runtime.release_stopped_for_retry(&profile_id).await);
                }
                Some(RuntimeCommand::TakeStoppedLeaseForReopen { profile_id, reply }) => {
                    let _ = reply.send(runtime.take_stopped_lease_for_reopen(&profile_id));
                }
                Some(RuntimeCommand::SetSyncAttemptActive { profile_id, active, reply }) => {
                    let _ = reply.send(runtime.set_sync_attempt_active(&profile_id, active));
                }
                Some(RuntimeCommand::SyncAttemptActive { profile_id, reply }) => {
                    let _ = reply.send(runtime.sync_attempt_active(&profile_id));
                }
                Some(RuntimeCommand::Authority { profile_id, reply }) => {
                    let _ = reply.send(runtime.authority(&profile_id));
                }
                Some(RuntimeCommand::RunningProfiles { reply }) => {
                    // This list also guards logout/reconfiguration. Keep profiles
                    // with a pending sync lease here even after their process stops.
                    let mut profiles = runtime.running.keys().cloned().collect::<Vec<_>>();
                    profiles.sort();
                    let _ = reply.send(profiles);
                }
                Some(RuntimeCommand::ActiveProfiles { reply }) => {
                    runtime.reconcile_exited_processes();
                    let candidates = runtime
                        .running
                        .iter()
                        .map(|(profile_id, running)| {
                            (
                                profile_id.clone(),
                                running.profile_path.clone(),
                                running.process_stopped,
                            )
                        })
                        .collect::<Vec<_>>();
                    let mut profiles = candidates
                        .into_iter()
                        // `reconcile_exited_processes` above marks only a confirmed
                        // process exit as stopped. If the OS cannot verify status,
                        // keep the tracked process active; treating Unknown as
                        // stopped can make launch recovery stop a browser that is
                        // still open.
                        .filter_map(|(profile_id, _path, stopped)| {
                            (!stopped).then_some(profile_id)
                        })
                        .collect::<Vec<_>>();
                    profiles.sort();
                    let _ = reply.send(profiles);
                }
                Some(RuntimeCommand::ProcessStateForPath { profile_path, reply }) => {
                    let _ = reply.send(runtime.lifecycle.reconcile_profile(&profile_path));
                }
                Some(RuntimeCommand::UnexpectedlyStoppedProfiles { reply }) => {
                    let _ = reply.send(runtime.unexpectedly_stopped.iter().cloned().collect());
                }
                Some(RuntimeCommand::QueueSyncRetry { profile_id, reply }) => {
                    if runtime.running.get(&profile_id).is_none_or(|running| {
                        running.process_stopped && running.sync_pending
                    }) {
                        runtime.unexpectedly_stopped.insert(profile_id);
                    }
                    let _ = reply.send(());
                }
                Some(RuntimeCommand::SetCoordination { coordination, reply }) => {
                    let result = runtime.set_coordination(coordination);
                    if result.is_ok() {
                        interval = tokio::time::interval(runtime.heartbeat_interval());
                        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        interval.reset();
                    }
                    let _ = reply.send(result);
                }
                Some(RuntimeCommand::ApproveExecutable { path, reply }) => {
                    let _ = reply.send(runtime.lifecycle.approve_executable(&path));
                }
                Some(RuntimeCommand::Shutdown { reply }) => {
                    let (mut result, leases) = runtime.shutdown().await;
                    let coordination = runtime.coordination.clone();
                    tauri::async_runtime::spawn(async move {
                        for lease in leases {
                            if let Err(error) = release(coordination.clone(), &lease).await
                                && !lease_ownership_lost(&error)
                                && result.is_ok()
                            {
                                result = Err(error);
                            }
                        }
                        let _ = reply.send(result);
                    });
                    break;
                }
                Some(RuntimeCommand::RenewalCompleted {
                    profile_id,
                    session_epoch,
                    lease,
                    result,
                }) => {
                    runtime.apply_renewal(profile_id, session_epoch, lease, result);
                }
                None => break,
            },
            _ = interval.tick(), if runtime.coordination.is_some() => runtime.schedule_renewals(),
        }
    }
}

impl<C: ProcessController> ProfileRuntime<C> {
    fn schedule_renewals(&mut self) {
        // Reconcile processes locally before dispatching network work. Slow API
        // renewals must not block stop, launch, or status commands in this actor.
        let exited = self.lifecycle.reconcile();
        let mut bridges_to_stop = Vec::new();
        for (path, state) in exited {
            if state == buglogin_browser::lifecycle::LifecycleState::Ready
                && let Some((profile_id, running)) = self
                    .running
                    .iter_mut()
                    .find(|(_, running)| running.profile_path == path)
            {
                mark_process_stopped_for_sync(running);
                self.unexpectedly_stopped.insert(profile_id.clone());
                if let Some(bridge) = running.proxy_bridge.take() {
                    bridges_to_stop.push(bridge);
                }
            }
        }
        for bridge in bridges_to_stop {
            tauri::async_runtime::spawn(async move {
                let _ = bridge.stop().await;
            });
        }

        let Some(coordination) = self.coordination.clone() else {
            return;
        };
        let leases = self
            .running
            .iter()
            .filter_map(|(profile_id, running)| {
                running
                    .lease
                    .clone()
                    .map(|lease| (profile_id.clone(), running.session_epoch, lease))
            })
            .collect::<Vec<_>>();
        for (profile_id, session_epoch, lease) in leases {
            let renewal_identity = (
                session_epoch,
                lease.fencing_token,
                lease.owner_token.clone(),
            );
            if self
                .pending_renewals
                .get(&profile_id)
                .is_some_and(|pending| pending == &renewal_identity)
            {
                continue;
            }
            self.pending_renewals
                .insert(profile_id.clone(), renewal_identity);
            let sender = self.commands.clone();
            let renewal_coordination = coordination.clone();
            let release_only = self.running.get(&profile_id).is_some_and(|running| {
                (running.process_stopped && !running.sync_pending)
                    || (running.process_stopped
                        && running.sync_pending
                        && !running.sync_attempt_active
                        && sync_hold_expired(running.sync_started_at, Instant::now()))
            });
            tauri::async_runtime::spawn(async move {
                let result = if release_only {
                    Err(lease_renewal_not_required())
                } else {
                    renew_lease(&renewal_coordination, &lease).await
                };
                let _ = sender
                    .send(RuntimeCommand::RenewalCompleted {
                        profile_id,
                        session_epoch,
                        lease,
                        result,
                    })
                    .await;
            });
        }
    }

    fn apply_renewal(
        &mut self,
        profile_id: ProfileId,
        session_epoch: u64,
        lease: LeaseGrant,
        result: Result<LeaseGrant, Problem>,
    ) {
        let renewal_identity = (
            session_epoch,
            lease.fencing_token,
            lease.owner_token.clone(),
        );
        if self.pending_renewals.get(&profile_id) == Some(&renewal_identity) {
            self.pending_renewals.remove(&profile_id);
        }
        let Some(running) = self.running.get(&profile_id) else {
            return;
        };
        if running.session_epoch != session_epoch
            || running.lease.as_ref().is_none_or(|current| {
                current.fencing_token != lease.fencing_token
                    || current.owner_token != lease.owner_token
            })
        {
            return;
        }
        let process_stopped = running.process_stopped;
        let release_only = (running.process_stopped && !running.sync_pending)
            || (running.process_stopped
                && running.sync_pending
                && !running.sync_attempt_active
                && sync_hold_expired(running.sync_started_at, Instant::now()));
        let Some(coordination) = self.coordination.clone() else {
            return;
        };
        if release_only {
            self.running.remove(&profile_id);
            if result
                .as_ref()
                .err()
                .is_some_and(|error| error.code == "lease_renewal_not_required")
                && process_stopped
            {
                self.unexpectedly_stopped.insert(profile_id.clone());
            }
            tauri::async_runtime::spawn(async move {
                let _ = release(Some(coordination), &lease).await;
            });
            return;
        }
        match result {
            Ok(renewed) if renewed.fencing_token == lease.fencing_token => {
                if let Some(running) = self.running.get_mut(&profile_id) {
                    running.lease = Some(renewed);
                }
            }
            Err(error)
                if !lease_ownership_lost(&error)
                    && lease_has_renewal_safety_window(&lease, self.heartbeat_interval()) => {}
            Err(error) if process_stopped && lease_ownership_lost(&error) => {
                self.running.remove(&profile_id);
                if error.code == "lease_not_found" {
                    self.unexpectedly_stopped.insert(profile_id);
                } else {
                    self.unexpectedly_stopped.remove(&profile_id);
                }
            }
            _ if process_stopped => {}
            _ => {
                let mut bridge = None;
                if let Some(running) = self.running.get_mut(&profile_id)
                    && self.lifecycle.stop(&running.profile_path).is_ok()
                {
                    mark_process_stopped_for_sync(running);
                    bridge = running.proxy_bridge.take();
                    self.unexpectedly_stopped.insert(profile_id);
                }
                if let Some(bridge) = bridge {
                    tauri::async_runtime::spawn(async move {
                        let _ = bridge.stop().await;
                    });
                }
            }
        }
    }

    fn reconcile_exited_processes(&mut self) {
        let exited = self.lifecycle.reconcile();
        let mut bridges_to_stop = Vec::new();
        for (path, state) in exited {
            if state == buglogin_browser::lifecycle::LifecycleState::Ready
                && let Some((profile_id, running)) = self
                    .running
                    .iter_mut()
                    .find(|(_, running)| running.profile_path == path)
            {
                // A browser can exit without the Stop button or a foreground
                // status poll. Start the bounded sync hold here so heartbeat
                // renewal cannot keep an orphaned team lease alive forever.
                mark_process_stopped_for_sync(running);
                self.unexpectedly_stopped.insert(profile_id.clone());
                if let Some(bridge) = running.proxy_bridge.take() {
                    bridges_to_stop.push(bridge);
                }
            }
        }
        for bridge in bridges_to_stop {
            bridge.stop_in_background();
        }
    }

    fn heartbeat_interval(&self) -> Duration {
        self.coordination
            .as_ref()
            .map(|value| value.heartbeat_interval)
            .unwrap_or(Duration::from_secs(30))
    }

    fn set_coordination(
        &mut self,
        coordination: Option<ManagedCoordination>,
    ) -> Result<(), Problem> {
        let same_actor_refresh = (self.running.is_empty() && self.launching.is_empty())
            || self
                .coordination
                .as_ref()
                .zip(coordination.as_ref())
                .is_some_and(|(current, next)| current.auth.actor_id() == next.auth.actor_id());
        if !same_actor_refresh {
            return Err(Problem::new(
                "runtime_reconfigure_running",
                "Runtime coordination cannot change while profiles are running",
                409,
                "desktop",
            ));
        }
        self.coordination = coordination;
        Ok(())
    }

    fn reserve_launch(&mut self, profile_id: &ProfileId) -> Result<u64, Problem> {
        if self.running.contains_key(profile_id) {
            return Err(Problem::new(
                "profile_already_running",
                "Profile is already running",
                409,
                "desktop",
            ));
        }
        if self.launching.contains_key(profile_id) {
            return Err(Problem::new(
                "profile_launch_in_progress",
                "Profile launch is already in progress",
                409,
                "desktop",
            ));
        }
        let session_epoch = self.allocate_session_epoch()?;
        self.launching.insert(profile_id.clone(), session_epoch);
        Ok(session_epoch)
    }

    fn allocate_session_epoch(&mut self) -> Result<u64, Problem> {
        let session_epoch = self.next_session_epoch;
        self.next_session_epoch = session_epoch.checked_add(1).ok_or_else(|| {
            Problem::new(
                "runtime_session_epoch_exhausted",
                "Runtime session generation is exhausted",
                500,
                "desktop",
            )
        })?;
        Ok(session_epoch)
    }

    #[cfg(test)]
    async fn launch(&mut self, mut request: LaunchRequest) -> Result<LaunchReceipt, Problem> {
        let session_epoch = self.reserve_launch(&request.profile_id)?;
        let coordination = self.coordination.clone();
        let lease_result = match request.preacquired_lease.take() {
            Some(lease) => {
                validate_preacquired(coordination.as_ref(), request.lease_scope.as_ref(), &lease)
                    .map(|()| Some(lease))
            }
            None => {
                acquire(
                    coordination.clone(),
                    request.lease_scope.as_ref(),
                    &request.idempotency_key,
                )
                .await
            }
        };
        self.launching.remove(&request.profile_id);
        let result = match lease_result {
            Ok(lease) => self.launch_process(request, lease, session_epoch),
            Err(error) => Err((request, None, error)),
        };
        match result {
            Ok(receipt) => Ok(receipt),
            Err((mut request, lease, error)) => {
                if let Some(bridge) = request.proxy_bridge.take() {
                    let _ = bridge.stop().await;
                }
                if let Some(lease) = lease.as_ref() {
                    let _ = release(coordination, lease).await;
                }
                Err(error)
            }
        }
    }

    fn launch_process(
        &mut self,
        request: LaunchRequest,
        lease: Option<LeaseGrant>,
        session_epoch: u64,
    ) -> Result<LaunchReceipt, (LaunchRequest, Option<LeaseGrant>, Problem)> {
        if self.running.contains_key(&request.profile_id) {
            return Err((
                request,
                lease,
                Problem::new(
                    "profile_already_running",
                    "Profile is already running",
                    409,
                    "desktop",
                ),
            ));
        };
        if request.specification.runtime == Runtime::Wayfern {
            let no_live_process = matches!(
                self.lifecycle.reconcile_profile(&request.profile_path),
                Ok(buglogin_browser::lifecycle::LifecycleState::Ready
                    | buglogin_browser::lifecycle::LifecycleState::Stopped)
            );
            let prepared = if no_live_process {
                clear_stale_chromium_lock_links(&request.profile_path)
            } else {
                Ok(())
            }
            .and_then(|()| {
                buglogin_browser::runtime::wayfern::configure_new_tab_shortcuts(
                    &request.profile_path,
                )
            });
            if let Err(error) = prepared {
                return Err((request, lease, error));
            }
        }
        let identity = match self
            .lifecycle
            .launch(&request.profile_path, &request.specification)
        {
            Ok(identity) => identity,
            Err(error) => return Err((request, lease, error)),
        };
        let write_authority = lease.as_ref().map(authority);
        self.running.insert(
            request.profile_id,
            RunningProfile {
                profile_path: request.profile_path,
                session_epoch,
                lease,
                was_launched: true,
                process_stopped: false,
                sync_pending: false,
                sync_attempt_active: false,
                sync_started_at: None,
                proxy_bridge: request.proxy_bridge,
            },
        );
        Ok(LaunchReceipt {
            process_id: identity.pid,
            write_authority,
            session_epoch,
        })
    }

    #[cfg(test)]
    async fn stop(&mut self, profile_id: &ProfileId) -> Result<(), Problem> {
        match self.stop_for_sync(profile_id).await {
            Ok(_) => self.complete_stop(profile_id).await,
            Err(error) if error.code == "profile_not_running" => {
                self.complete_stop(profile_id).await
            }
            Err(error) => Err(error),
        }
    }

    async fn stop_for_sync(&mut self, profile_id: &ProfileId) -> Result<StopReceipt, Problem> {
        self.stop_for_session(profile_id, None).await
    }

    async fn stop_for_session(
        &mut self,
        profile_id: &ProfileId,
        expected_session_epoch: Option<u64>,
    ) -> Result<StopReceipt, Problem> {
        if !self.running.contains_key(profile_id) {
            if let Some(launch_epoch) = self.launching.get(profile_id).copied() {
                if expected_session_epoch.is_some_and(|expected| expected != launch_epoch) {
                    return Err(Problem::new(
                        "profile_session_changed",
                        "Profile runtime session changed before the stop request was handled",
                        409,
                        "desktop",
                    )
                    .retryable(true));
                }
                self.cancelled_launches
                    .insert((profile_id.clone(), launch_epoch));
            }
            return Err(Problem::new(
                "profile_not_running",
                "Profile is not running",
                409,
                "desktop",
            ));
        }
        let (profile_path, lease, process_stopped, was_launched, session_epoch) = self
            .running
            .get(profile_id)
            .map(|running| {
                (
                    running.profile_path.clone(),
                    running.lease.clone(),
                    running.process_stopped,
                    running.was_launched,
                    running.session_epoch,
                )
            })
            .ok_or_else(|| {
                Problem::new(
                    "profile_not_running",
                    "Profile is not running",
                    409,
                    "desktop",
                )
            })?;
        if expected_session_epoch.is_some_and(|expected| expected != session_epoch) {
            return Err(Problem::new(
                "profile_session_changed",
                "Profile runtime session changed before the stop request was handled",
                409,
                "desktop",
            )
            .retryable(true));
        }
        if !process_stopped {
            self.lifecycle.stop(&profile_path)?;
            if let Some(running) = self.running.get_mut(profile_id) {
                running.process_stopped = true;
            }
        }
        let proxy_bridge = self
            .running
            .get_mut(profile_id)
            .and_then(|running| running.proxy_bridge.take());
        if let Some(proxy_bridge) = proxy_bridge {
            proxy_bridge.stop_in_background();
        }
        if let Some(running) = self.running.get_mut(profile_id) {
            if !running.sync_pending {
                running.sync_started_at = Some(Instant::now());
            }
            running.sync_pending = true;
            running.sync_attempt_active = false;
        }
        Ok(StopReceipt {
            profile_path,
            lease,
            was_launched,
            session_epoch,
        })
    }

    fn hold_idle_profile_for_sync(
        &mut self,
        profile_id: ProfileId,
        profile_path: PathBuf,
        lease_scope: LeaseScope,
        lease: LeaseGrant,
    ) -> Result<StopReceipt, Problem> {
        if let Some(running) = self.running.get(&profile_id) {
            if let Some(held_lease) = running.lease.as_ref() {
                let resumable = running.process_stopped
                    && running.sync_pending
                    && !running.was_launched
                    && running.profile_path == profile_path
                    && held_lease.workspace_id == lease.workspace_id
                    && held_lease.resource_id == lease.resource_id
                    && held_lease.owner_id == lease.owner_id
                    && held_lease.owner_token == lease.owner_token
                    && held_lease.fencing_token == lease.fencing_token;
                if resumable {
                    validate_preacquired(
                        self.coordination.as_ref(),
                        Some(&lease_scope),
                        held_lease,
                    )?;
                    return Ok(StopReceipt {
                        profile_path: running.profile_path.clone(),
                        lease: Some(held_lease.clone()),
                        was_launched: false,
                        session_epoch: running.session_epoch,
                    });
                }
            }
            return Err(Problem::new(
                "profile_already_running",
                "Profile is already running or has a pending sync",
                409,
                "desktop",
            ));
        }
        validate_preacquired(self.coordination.as_ref(), Some(&lease_scope), &lease)?;
        let session_epoch = self.allocate_session_epoch()?;
        let receipt = StopReceipt {
            profile_path: profile_path.clone(),
            lease: Some(lease.clone()),
            was_launched: false,
            session_epoch,
        };
        self.running.insert(
            profile_id,
            RunningProfile {
                profile_path,
                session_epoch,
                lease: Some(lease),
                was_launched: false,
                process_stopped: true,
                sync_pending: true,
                sync_attempt_active: false,
                sync_started_at: Some(Instant::now()),
                proxy_bridge: None,
            },
        );
        Ok(receipt)
    }

    fn begin_complete_stop(
        &mut self,
        profile_id: &ProfileId,
        expected_session_epoch: Option<u64>,
    ) -> Result<Option<LeaseGrant>, Problem> {
        if let Some(running) = self.running.get(profile_id) {
            if expected_session_epoch.is_some_and(|expected| expected != running.session_epoch)
                || !running.process_stopped
                || !running.sync_pending
            {
                return Ok(None);
            }
        }
        let running = self.running.remove(profile_id);
        self.unexpectedly_stopped.remove(profile_id);
        Ok(running.and_then(|running| running.lease))
    }

    #[cfg(test)]
    async fn complete_stop(&mut self, profile_id: &ProfileId) -> Result<(), Problem> {
        let Some(lease) = self.begin_complete_stop(profile_id, None)? else {
            return Ok(());
        };
        if let Err(error) = release(self.coordination.clone(), &lease).await
            && !lease_ownership_lost(&error)
        {
            // The browser is already stopped. Keep the local files, but never
            // retain a local runtime lock just because the API could not
            // acknowledge release; the remote lease expires on its own.
            return Err(error);
        }
        Ok(())
    }

    fn set_sync_attempt_active(
        &mut self,
        profile_id: &ProfileId,
        active: bool,
    ) -> Result<(), Problem> {
        let Some(running) = self.running.get_mut(profile_id) else {
            return Err(Problem::new(
                "profile_sync_lease_missing",
                "Profile has no local sync transaction",
                409,
                "desktop",
            ));
        };
        if !running.process_stopped || !running.sync_pending || running.lease.is_none() {
            return Err(Problem::new(
                "profile_sync_lease_invalid",
                "Profile sync requires a stopped process and a held lease",
                409,
                "desktop",
            ));
        }
        if active {
            running.sync_started_at = Some(Instant::now());
        }
        running.sync_attempt_active = active;
        Ok(())
    }

    fn sync_attempt_active(&self, profile_id: &ProfileId) -> bool {
        self.running
            .get(profile_id)
            .is_some_and(|running| running.process_stopped && running.sync_attempt_active)
    }

    async fn release_stopped_for_retry(&mut self, profile_id: &ProfileId) -> Result<(), Problem> {
        if self
            .running
            .get(profile_id)
            .is_some_and(|running| !running.process_stopped || !running.sync_pending)
        {
            return Ok(());
        }
        let running = self.running.remove(profile_id);
        self.unexpectedly_stopped.insert(profile_id.clone());
        if let Some(lease) = running.and_then(|running| running.lease) {
            let coordination = self.coordination.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = release(coordination, &lease).await
                    && !lease_ownership_lost(&error)
                {
                    tauri_plugin_log::log::warn!(
                        "Stopped profile lease release deferred: code={}",
                        error.code
                    );
                }
            });
        }
        Ok(())
    }

    fn complete_stop_detached(
        &mut self,
        profile_id: &ProfileId,
        expected_session_epoch: Option<u64>,
    ) {
        let Ok(Some(lease)) = self.begin_complete_stop(profile_id, expected_session_epoch) else {
            return;
        };
        let coordination = self.coordination.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = release(coordination, &lease).await
                && !lease_ownership_lost(&error)
            {
                tauri_plugin_log::log::warn!(
                    "Stopped profile lease release deferred: code={}",
                    error.code
                );
            }
        });
    }

    fn take_stopped_lease_for_reopen(
        &mut self,
        profile_id: &ProfileId,
    ) -> Result<Option<LeaseGrant>, Problem> {
        let Some(running) = self.running.get(profile_id) else {
            return Ok(None);
        };
        if !running.process_stopped || !running.sync_pending || running.sync_attempt_active {
            return Err(Problem::new(
                "profile_sync_in_progress",
                "Profile cannot reopen while its stopped snapshot is being synchronized",
                409,
                "desktop",
            )
            .retryable(true));
        }
        let Some(running) = self.running.remove(profile_id) else {
            return Ok(None);
        };
        self.unexpectedly_stopped.remove(profile_id);
        Ok(running.lease)
    }

    fn authority(&self, profile_id: &ProfileId) -> Option<WriteAuthority> {
        self.running
            .get(profile_id)
            .and_then(|running| running.lease.as_ref())
            .map(authority)
    }

    #[cfg(test)]
    async fn renew(&mut self) {
        // Process exit detection must run from the heartbeat itself. Previously
        // it depended on the UI asking for `active_profiles`; a minimized app
        // could keep renewing a dead browser's lease indefinitely.
        self.reconcile_exited_processes();
        let Some(coordination) = self.coordination.clone() else {
            return;
        };
        let leases = self
            .running
            .iter()
            .filter_map(|(profile_id, running)| {
                running
                    .lease
                    .clone()
                    .map(|lease| (profile_id.clone(), lease))
            })
            .collect::<Vec<_>>();
        for (profile_id, lease) in leases {
            let stop_sync_expired = self.running.get(&profile_id).is_some_and(|running| {
                running.process_stopped
                    && running.sync_pending
                    && !running.sync_attempt_active
                    && sync_hold_expired(running.sync_started_at, Instant::now())
            });
            if stop_sync_expired {
                // Never keep a team lock forever after the browser is stopped.
                // Fencing makes a late upload fail once this lease is released;
                // the local profile files remain available for retry/recovery.
                let _ = release(self.coordination.clone(), &lease).await;
                self.running.remove(&profile_id);
                // The lease hold is bounded, but retry intent survives it. The
                // desktop reconciler can reacquire a lease from the dirty local
                // generation instead of abandoning the user's pending changes.
                continue;
            }
            if self
                .running
                .get(&profile_id)
                .is_some_and(|running| running.process_stopped && !running.sync_pending)
            {
                if release(self.coordination.clone(), &lease).await.is_ok() {
                    self.running.remove(&profile_id);
                }
                continue;
            }
            let renewal_identity = format!(
                "{}|{}|{}|{}",
                lease.resource_id.as_str(),
                lease.fencing_token,
                lease.expires_at_ms,
                lease.owner_token
            );
            let renewal_key = format!(
                "lease-renew-{}",
                crate::remote_api::sha256_hex(renewal_identity.as_bytes())
            );
            let renewal_timeout = lease_renewal_timeout(&lease, self.heartbeat_interval());
            let renewal_deadline = tokio::time::Instant::now() + renewal_timeout;
            let mut renewal_attempt = 0;
            let renewed = if renewal_timeout.is_zero() {
                Err(lease_renewal_deadline_elapsed())
            } else {
                loop {
                    match tokio::time::timeout_at(
                        renewal_deadline,
                        coordination.backend.renew_lease_idempotent(
                            CoordinationScope {
                                workspace_id: &lease.workspace_id,
                                auth: &coordination.auth,
                            },
                            &lease.resource_id,
                            &lease.owner_token,
                            coordination.lease_ttl,
                            &renewal_key,
                        ),
                    )
                    .await
                    {
                        Err(_) => break Err(lease_renewal_deadline_elapsed()),
                        Ok(Err(error)) if error.retryable && renewal_attempt == 0 => {
                            renewal_attempt += 1;
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                        Ok(result) => break result,
                    }
                }
            };
            let process_stopped = self
                .running
                .get(&profile_id)
                .is_some_and(|running| running.process_stopped);
            match renewed {
                Ok(renewed) if renewed.fencing_token == lease.fencing_token => {
                    if let Some(running) = self.running.get_mut(&profile_id) {
                        running.lease = Some(renewed);
                    }
                }
                // A network blip, sleep or API restart: the lease is still ours
                // until it expires, so keep the browser open and retry next tick.
                Err(error)
                    if !lease_ownership_lost(&error)
                        && lease_has_renewal_safety_window(&lease, self.heartbeat_interval()) => {}
                // Another device now owns the profile: this stopped session can
                // never upload, so stop holding it (the local copy stays on disk).
                Err(error) if process_stopped && lease_ownership_lost(&error) => {
                    self.running.remove(&profile_id);
                    if error.code == "lease_not_found" {
                        // An absent lease is not proof that another device owns
                        // the resource. Keep the local retry intent so the
                        // stopped snapshot can acquire a fresh lease and CAS
                        // against the latest manifest.
                        self.unexpectedly_stopped.insert(profile_id.clone());
                    } else {
                        self.unexpectedly_stopped.remove(&profile_id);
                    }
                }
                // Already stopped with a sync pending; the reconcile loop retries it.
                _ if process_stopped => {}
                _ => {
                    let _ = self.stop_for_sync(&profile_id).await;
                    self.unexpectedly_stopped.insert(profile_id.clone());
                }
            }
        }
    }

    async fn shutdown(&mut self) -> (Result<(), Problem>, Vec<LeaseGrant>) {
        let profiles = self.running.keys().cloned().collect::<Vec<_>>();
        let mut first_error = None;
        let mut leases = Vec::new();
        for profile_id in profiles {
            match self.stop_for_sync(&profile_id).await {
                Ok(receipt) => {
                    match self.begin_complete_stop(&profile_id, Some(receipt.session_epoch)) {
                        Ok(Some(lease)) => leases.push(lease),
                        Ok(None) => {}
                        Err(error) if first_error.is_none() => first_error = Some(error),
                        Err(_) => {}
                    }
                }
                Err(error) if first_error.is_none() => first_error = Some(error),
                Err(_) => {}
            }
        }
        (first_error.map_or(Ok(()), Err), leases)
    }
}

fn mark_process_stopped_for_sync(running: &mut RunningProfile) {
    running.process_stopped = true;
    running.sync_pending = true;
    running.sync_attempt_active = false;
    running.sync_started_at.get_or_insert_with(Instant::now);
}

fn sync_hold_expired(started_at: Option<Instant>, now: Instant) -> bool {
    started_at
        .is_none_or(|started_at| now.saturating_duration_since(started_at) >= MAX_STOP_SYNC_HOLD)
}

fn lease_ownership_lost(error: &Problem) -> bool {
    matches!(
        error.code.as_str(),
        "lease_not_owner" | "lease_not_found" | "lease_held" | "lease_tenant_mismatch"
    )
}

fn lease_has_renewal_safety_window(lease: &LeaseGrant, heartbeat_interval: Duration) -> bool {
    let margin = heartbeat_interval.max(Duration::from_secs(5));
    unix_time_millis("lease_renewal")
        .is_ok_and(|now_ms| now_ms.saturating_add(margin.as_millis() as u64) < lease.expires_at_ms)
}

fn lease_renewal_timeout(lease: &LeaseGrant, heartbeat_interval: Duration) -> Duration {
    let now_ms = unix_time_millis("lease_renewal").unwrap_or(lease.expires_at_ms);
    let remaining = Duration::from_millis(lease.expires_at_ms.saturating_sub(now_ms));
    remaining.saturating_sub(heartbeat_interval.max(Duration::from_secs(5)))
}

fn lease_renewal_deadline_elapsed() -> Problem {
    Problem::new(
        "lease_renewal_deadline_elapsed",
        "The profile lease could not be renewed before its safety deadline",
        409,
        "lease_renewal",
    )
    .retryable(true)
}

fn lease_renewal_not_required() -> Problem {
    Problem::new(
        "lease_renewal_not_required",
        "Lease no longer needs renewal",
        409,
        "desktop",
    )
}

async fn renew_lease(
    coordination: &ManagedCoordination,
    lease: &LeaseGrant,
) -> Result<LeaseGrant, Problem> {
    let renewal_identity = format!(
        "{}|{}|{}|{}",
        lease.resource_id.as_str(),
        lease.fencing_token,
        lease.expires_at_ms,
        lease.owner_token
    );
    let renewal_key = format!(
        "lease-renew-{}",
        crate::remote_api::sha256_hex(renewal_identity.as_bytes())
    );
    let renewal_timeout = lease_renewal_timeout(lease, coordination.heartbeat_interval);
    if renewal_timeout.is_zero() {
        return Err(lease_renewal_deadline_elapsed());
    }
    let renewal_deadline = tokio::time::Instant::now() + renewal_timeout;
    let mut renewal_attempt = 0;
    loop {
        match tokio::time::timeout_at(
            renewal_deadline,
            coordination.backend.renew_lease_idempotent(
                CoordinationScope {
                    workspace_id: &lease.workspace_id,
                    auth: &coordination.auth,
                },
                &lease.resource_id,
                &lease.owner_token,
                coordination.lease_ttl,
                &renewal_key,
            ),
        )
        .await
        {
            Err(_) => return Err(lease_renewal_deadline_elapsed()),
            Ok(Err(error)) if error.retryable && renewal_attempt == 0 => {
                renewal_attempt += 1;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            Ok(result) => return result,
        }
    }
}

fn validate_preacquired(
    coordination: Option<&ManagedCoordination>,
    scope: Option<&LeaseScope>,
    lease: &LeaseGrant,
) -> Result<(), Problem> {
    let coordination = coordination.ok_or_else(|| {
        Problem::new(
            "managed_coordination_required",
            "Managed coordination is required",
            503,
            "desktop",
        )
    })?;
    let scope = scope.ok_or_else(|| {
        Problem::new(
            "lease_scope_required",
            "Lease scope is required",
            409,
            "desktop",
        )
    })?;
    let now_ms = unix_time_millis("preacquired_lease")?;
    if lease.workspace_id != scope.workspace_id
        || lease.resource_id != scope.resource_id
        || lease.owner_id != coordination.auth.actor_id()
        || lease.expires_at_ms <= now_ms
    {
        return Err(Problem::new(
            "preacquired_lease_invalid",
            "Pre-acquired lease is invalid",
            409,
            "desktop",
        ));
    }
    Ok(())
}

async fn acquire(
    coordination: Option<ManagedCoordination>,
    scope: Option<&LeaseScope>,
    idempotency_key: &str,
) -> Result<Option<LeaseGrant>, Problem> {
    let Some(scope) = scope else { return Ok(None) };
    let coordination = coordination.ok_or_else(|| {
        Problem::new(
            "managed_lease_unavailable",
            "Managed lease authority is unavailable",
            503,
            "desktop",
        )
        .retryable(true)
    })?;
    coordination
        .backend
        .acquire_lease_idempotent(
            CoordinationScope {
                workspace_id: &scope.workspace_id,
                auth: &coordination.auth,
            },
            &scope.resource_id,
            coordination.lease_ttl,
            idempotency_key,
        )
        .await
        .map(Some)
}

async fn release(
    coordination: Option<ManagedCoordination>,
    lease: &LeaseGrant,
) -> Result<(), Problem> {
    let coordination = coordination.ok_or_else(unavailable)?;
    let scope = CoordinationScope {
        workspace_id: &lease.workspace_id,
        auth: &coordination.auth,
    };
    let idempotency_key =
        lease_release_idempotency_key(&scope, &lease.resource_id, &lease.owner_token);
    let mut attempt = 0;
    loop {
        match coordination
            .backend
            .release_lease_idempotent(
                scope.clone(),
                &lease.resource_id,
                &lease.owner_token,
                &idempotency_key,
            )
            .await
        {
            Err(error) if error.retryable && attempt == 0 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
            result => return result,
        }
    }
}

fn authority(lease: &LeaseGrant) -> WriteAuthority {
    WriteAuthority {
        workspace_id: lease.workspace_id.clone(),
        resource_id: lease.resource_id.clone(),
        fencing_token: lease.fencing_token,
    }
}

/// A browser stopped by signal leaves Chromium's lock links behind; a stale
/// SingletonLock naming an old hostname makes the next start refuse the
/// profile. Only called when the lifecycle reports no live process.
fn clear_stale_chromium_lock_links(profile_path: &Path) -> Result<(), Problem> {
    for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
        let path = profile_path.join(name);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                fs::remove_file(&path).map_err(|_| unavailable())?;
            }
            Ok(_) | Err(_) => {}
        }
    }
    Ok(())
}

fn unavailable() -> Problem {
    Problem::new(
        "profile_runtime_unavailable",
        "Profile runtime is unavailable",
        503,
        "desktop",
    )
    .retryable(true)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::Path,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{SystemTime, UNIX_EPOCH},
    };
    use tokio::sync::Notify;

    use buglogin_browser::process::{ManagedProcess, ProcessIdentity, ProcessStatus};
    use buglogin_domain::entities::Runtime;
    use buglogin_sync::{
        coordination::{CoordinationEvent, Presence, PublishedEvent, StreamEvent},
        manifest::SyncManifest,
    };

    use super::*;

    #[test]
    fn stopped_sync_lease_has_a_bounded_hold_window() {
        let started_at = Instant::now();
        assert!(!sync_hold_expired(
            Some(started_at),
            started_at + MAX_STOP_SYNC_HOLD - Duration::from_secs(1)
        ));
        assert!(sync_hold_expired(
            Some(started_at),
            started_at + MAX_STOP_SYNC_HOLD
        ));
        assert!(sync_hold_expired(None, started_at));
    }

    #[derive(Clone, Default)]
    struct TestController {
        fail_launch: Arc<AtomicBool>,
        process_exited: Arc<AtomicBool>,
        process_status_unknown: Arc<AtomicBool>,
        launches: Arc<AtomicUsize>,
        stop_failures: Arc<AtomicUsize>,
        stops: Arc<AtomicUsize>,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl ProcessController for TestController {
        fn launch(
            &self,
            specification: &LaunchSpecification,
        ) -> Result<Box<dyn ManagedProcess>, Problem> {
            self.log.lock().unwrap().push("launch");
            self.launches.fetch_add(1, Ordering::SeqCst);
            if self.fail_launch.load(Ordering::SeqCst) {
                return Err(unavailable());
            }
            Ok(Box::new(TestProcess {
                identity: ProcessIdentity {
                    pid: 42,
                    executable: specification.executable.clone(),
                    token: "test-process-token".into(),
                },
                stop_failures: self.stop_failures.clone(),
                process_exited: self.process_exited.clone(),
                process_status_unknown: self.process_status_unknown.clone(),
                stops: self.stops.clone(),
                log: self.log.clone(),
            }))
        }

        fn inspect(&self, _identity: &ProcessIdentity) -> ProcessStatus {
            if self.process_exited.load(Ordering::SeqCst) {
                ProcessStatus::Exited
            } else {
                ProcessStatus::Running
            }
        }

        fn terminate(&self, _identity: &ProcessIdentity) -> Result<(), Problem> {
            Ok(())
        }
    }

    struct TestProcess {
        identity: ProcessIdentity,
        process_exited: Arc<AtomicBool>,
        process_status_unknown: Arc<AtomicBool>,
        stop_failures: Arc<AtomicUsize>,
        stops: Arc<AtomicUsize>,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl ManagedProcess for TestProcess {
        fn identity(&self) -> &ProcessIdentity {
            &self.identity
        }

        fn status(&mut self) -> Result<ProcessStatus, Problem> {
            Ok(if self.process_status_unknown.load(Ordering::SeqCst) {
                ProcessStatus::Unknown
            } else if self.process_exited.load(Ordering::SeqCst) {
                ProcessStatus::Exited
            } else {
                ProcessStatus::Running
            })
        }

        fn stop(&mut self) -> Result<(), Problem> {
            self.log.lock().unwrap().push("stop");
            self.stops.fetch_add(1, Ordering::SeqCst);
            if self
                .stop_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(unavailable());
            }
            Ok(())
        }
    }

    struct TestCoordination {
        log: Arc<Mutex<Vec<&'static str>>>,
        renewals: Arc<AtomicUsize>,
        fail_acquire: AtomicBool,
        block_acquire: AtomicBool,
        acquire_started: Arc<Notify>,
        acquire_release: Arc<Notify>,
        fail_renew: AtomicBool,
        block_renew: AtomicBool,
        renew_started: Arc<Notify>,
        renew_release: Arc<Notify>,
        block_release: AtomicBool,
        release_started: Arc<Notify>,
        release_release: Arc<Notify>,
        renew_error_code: Mutex<Option<&'static str>>,
        lease_expires_at_ms: std::sync::atomic::AtomicU64,
        release_failures: AtomicUsize,
        release_error_code: Mutex<Option<&'static str>>,
        manifests: Mutex<BTreeMap<ResourceId, SyncManifest>>,
    }

    impl TestCoordination {
        fn new(log: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                log,
                renewals: Arc::new(AtomicUsize::new(0)),
                fail_acquire: AtomicBool::new(false),
                block_acquire: AtomicBool::new(false),
                acquire_started: Arc::new(Notify::new()),
                acquire_release: Arc::new(Notify::new()),
                fail_renew: AtomicBool::new(false),
                block_renew: AtomicBool::new(false),
                renew_started: Arc::new(Notify::new()),
                renew_release: Arc::new(Notify::new()),
                block_release: AtomicBool::new(false),
                release_started: Arc::new(Notify::new()),
                release_release: Arc::new(Notify::new()),
                renew_error_code: Mutex::new(None),
                lease_expires_at_ms: std::sync::atomic::AtomicU64::new(lease_valid_for(
                    Duration::from_secs(60),
                )),
                release_failures: AtomicUsize::new(0),
                release_error_code: Mutex::new(None),
                manifests: Mutex::new(BTreeMap::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl CoordinationBackend for TestCoordination {
        async fn readiness(&self) -> Result<(), Problem> {
            Ok(())
        }

        async fn acquire_lease_idempotent(
            &self,
            scope: CoordinationScope<'_>,
            resource_id: &ResourceId,
            _ttl: Duration,
            _idempotency_key: &str,
        ) -> Result<LeaseGrant, Problem> {
            self.log.lock().unwrap().push("acquire");
            if self.fail_acquire.load(Ordering::SeqCst) {
                return Err(unavailable());
            }
            if self.block_acquire.load(Ordering::SeqCst) {
                self.acquire_started.notify_one();
                self.acquire_release.notified().await;
            }
            Ok(LeaseGrant {
                resource_id: resource_id.clone(),
                workspace_id: scope.workspace_id.clone(),
                owner_id: scope.auth.actor_id().into(),
                owner_token: "private-owner-token".into(),
                fencing_token: 7,
                issued_at_ms: 1,
                expires_at_ms: self.lease_expires_at_ms.load(Ordering::SeqCst),
            })
        }

        async fn renew_lease_idempotent(
            &self,
            scope: CoordinationScope<'_>,
            resource_id: &ResourceId,
            owner_token: &str,
            _ttl: Duration,
            _idempotency_key: &str,
        ) -> Result<LeaseGrant, Problem> {
            self.renewals.fetch_add(1, Ordering::SeqCst);
            if self.block_renew.load(Ordering::SeqCst) {
                self.renew_started.notify_one();
                self.renew_release.notified().await;
            }
            if self.fail_renew.load(Ordering::SeqCst) {
                self.log.lock().unwrap().push("renew");
                return Err(match *self.renew_error_code.lock().unwrap() {
                    Some(code) => Problem::new(code, "Lease renewal rejected", 409, "test"),
                    None => unavailable(),
                });
            }
            Ok(LeaseGrant {
                resource_id: resource_id.clone(),
                workspace_id: scope.workspace_id.clone(),
                owner_id: scope.auth.actor_id().into(),
                owner_token: owner_token.into(),
                fencing_token: 7,
                issued_at_ms: 1,
                expires_at_ms: lease_valid_for(Duration::from_secs(60)),
            })
        }

        async fn release_lease_idempotent(
            &self,
            _scope: CoordinationScope<'_>,
            _resource_id: &ResourceId,
            owner_token: &str,
            _idempotency_key: &str,
        ) -> Result<(), Problem> {
            assert_eq!(owner_token, "private-owner-token");
            self.log.lock().unwrap().push("release");
            if self.block_release.load(Ordering::SeqCst) {
                self.release_started.notify_one();
                self.release_release.notified().await;
            }
            if self
                .release_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(unavailable());
            }
            if let Some(code) = *self.release_error_code.lock().unwrap() {
                return Err(Problem::new(code, "Lease release rejected", 409, "test"));
            }
            Ok(())
        }

        async fn lease_status(
            &self,
            _scope: CoordinationScope<'_>,
            _resource_id: &ResourceId,
        ) -> Result<Option<buglogin_sync::coordination::LeaseStatus>, Problem> {
            Ok(None)
        }

        async fn validate_write_authority(
            &self,
            scope: CoordinationScope<'_>,
            resource_id: &ResourceId,
            owner_token: &str,
            fencing_token: u64,
        ) -> Result<(), Problem> {
            if scope.auth.actor_id() != "actor-runtime"
                || scope.workspace_id.as_str() != "ws_runtime_1"
                || resource_id.as_str() != "res_runtime_1"
                || owner_token != "private-owner-token"
                || fencing_token != 7
            {
                return Err(Problem::new(
                    "lease_not_owner",
                    "Lease owner mismatch",
                    409,
                    "test",
                ));
            }
            Ok(())
        }

        async fn heartbeat_presence(
            &self,
            _scope: CoordinationScope<'_>,
            _session_id: &str,
            _metadata: BTreeMap<String, String>,
            _ttl: Duration,
        ) -> Result<Presence, Problem> {
            Err(unavailable())
        }

        async fn list_presence(
            &self,
            _scope: CoordinationScope<'_>,
        ) -> Result<Vec<Presence>, Problem> {
            Err(unavailable())
        }

        async fn publish_event(
            &self,
            _scope: CoordinationScope<'_>,
            _idempotency_key: &str,
            _event: CoordinationEvent,
        ) -> Result<PublishedEvent, Problem> {
            Err(unavailable())
        }

        async fn read_events(
            &self,
            _scope: CoordinationScope<'_>,
            _after_stream_id: Option<&str>,
            _limit: usize,
        ) -> Result<Vec<StreamEvent>, Problem> {
            Err(unavailable())
        }

        async fn current_manifest(
            &self,
            _scope: CoordinationScope<'_>,
            resource_id: &ResourceId,
        ) -> Result<Option<SyncManifest>, Problem> {
            Ok(self.manifests.lock().unwrap().get(resource_id).cloned())
        }

        async fn commit_manifest(
            &self,
            scope: CoordinationScope<'_>,
            resource_id: &ResourceId,
            owner_token: &str,
            manifest: SyncManifest,
        ) -> Result<(), Problem> {
            if owner_token != "private-owner-token"
                || manifest.workspace_id != *scope.workspace_id
                || manifest.resource_id != *resource_id
            {
                return Err(unavailable());
            }
            self.manifests
                .lock()
                .unwrap()
                .insert(resource_id.clone(), manifest);
            Ok(())
        }
    }

    fn profile_path(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("buglogin-runtime-{label}-{nonce}"));
        fs::create_dir_all(&path).unwrap();
        path.canonicalize().unwrap()
    }

    #[test]
    fn wayfern_launch_repairs_generated_shortcut_and_preserves_profile_preferences() {
        let path = profile_path("new-tab-preferences");
        fs::create_dir_all(path.join("Default")).unwrap();
        fs::write(
            path.join("Default/Preferences"),
            br#"{"profile":{"name":"preserve-me"},"ntp":{"shortcust_visible":true},"custom_links":{"initialized":true,"list":["https://buglogin.com/","https://example.org/"]}}"#,
        )
        .unwrap();

        buglogin_browser::runtime::wayfern::configure_new_tab_shortcuts(&path).unwrap();

        let preferences: serde_json::Value =
            serde_json::from_slice(&fs::read(path.join("Default/Preferences")).unwrap()).unwrap();
        assert_eq!(preferences["profile"]["name"], "preserve-me");
        assert_eq!(preferences["ntp"]["shortcust_visible"], true);
        assert_eq!(
            preferences["custom_links"]["list"],
            serde_json::json!(["https://example.org/"])
        );
        assert!(preferences["custom_links"].get("initialized").is_none());
        let _ = fs::remove_dir_all(path);
    }

    #[cfg(unix)]
    #[test]
    fn stale_chromium_lock_links_are_cleared_before_launch() {
        let path = profile_path("stale-lock-links");
        fs::create_dir_all(path.join("Default")).unwrap();
        for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
            std::os::unix::fs::symlink("Old-Hostname-4242", path.join(name)).unwrap();
        }
        fs::write(path.join("Default/Cookies"), b"keep").unwrap();

        clear_stale_chromium_lock_links(&path).unwrap();

        for name in ["SingletonLock", "SingletonSocket", "SingletonCookie"] {
            assert!(fs::symlink_metadata(path.join(name)).is_err(), "{name}");
        }
        assert_eq!(fs::read(path.join("Default/Cookies")).unwrap(), b"keep");
        let _ = fs::remove_dir_all(path);
    }

    fn request(path: &Path, managed: bool) -> LaunchRequest {
        request_for(path, "prf_runtime_1", managed)
    }

    fn request_for(path: &Path, profile_id: &str, managed: bool) -> LaunchRequest {
        LaunchRequest {
            profile_id: ProfileId::from_str(profile_id).unwrap(),
            idempotency_key: format!("launch-{profile_id}-idempotency"),
            profile_path: path.to_path_buf(),
            specification: LaunchSpecification {
                runtime: Runtime::Wayfern,
                version: "1.0.0".into(),
                executable: path.join("wayfern-test"),
                profile_path: path.to_path_buf(),
                args: vec![],
            },
            lease_scope: managed.then(|| LeaseScope {
                workspace_id: WorkspaceId::from_str("ws_runtime_1").unwrap(),
                resource_id: ResourceId::from_str("res_runtime_1").unwrap(),
            }),
            preacquired_lease: None,
            proxy_bridge: None,
        }
    }

    fn managed_coordination(backend: Arc<TestCoordination>) -> ManagedCoordination {
        managed_coordination_with_bearer(backend, "private-bearer")
    }

    fn managed_coordination_with_bearer(
        backend: Arc<TestCoordination>,
        bearer: &str,
    ) -> ManagedCoordination {
        ManagedCoordination {
            backend,
            auth: CoordinationAuth::new("actor-runtime", bearer).unwrap(),
            lease_ttl: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(15),
        }
    }

    fn runtime(
        controller: TestController,
        coordination: ManagedCoordination,
    ) -> ProfileRuntime<TestController> {
        let (commands, receiver) = mpsc::channel(1);
        ProfileRuntime {
            lifecycle: LifecycleManager::new(controller),
            coordination: Some(coordination),
            running: HashMap::new(),
            unexpectedly_stopped: BTreeSet::new(),
            pending_renewals: HashMap::new(),
            launching: HashMap::new(),
            cancelled_launches: BTreeSet::new(),
            next_session_epoch: 1,
            commands,
            receiver,
        }
    }

    #[tokio::test]
    async fn local_launch_and_stop_never_require_coordination() {
        let path = profile_path("local");
        let controller = TestController::default();
        let handle = spawn_profile_runtime(controller.clone(), None);

        let receipt = handle.launch(request(&path, false)).await.unwrap();
        assert_eq!(receipt.process_id, 42);
        assert_eq!(receipt.write_authority, None);
        assert_eq!(controller.launches.load(Ordering::SeqCst), 1);
        handle
            .stop(ProfileId::from_str("prf_runtime_1").unwrap())
            .await
            .unwrap();
        handle
            .stop(ProfileId::from_str("prf_runtime_1").unwrap())
            .await
            .expect("stopping an already stopped profile is idempotent");
        assert_eq!(controller.stops.load(Ordering::SeqCst), 1);
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn blocked_lease_renewal_does_not_block_local_stop_command() {
        let path = profile_path("renewal-does-not-block-stop");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(60)), Ordering::SeqCst);
        backend.block_renew.store(true, Ordering::SeqCst);
        let renew_started = backend.renew_started.clone();
        let renew_release = backend.renew_release.clone();
        let mut coordination = managed_coordination(backend);
        coordination.heartbeat_interval = Duration::from_millis(10);
        let handle = spawn_profile_runtime(controller.clone(), Some(coordination));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();

        handle.launch(request(&path, true)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), renew_started.notified())
            .await
            .expect("lease renewal starts");

        let stop = tokio::time::timeout(
            Duration::from_secs(2),
            handle.stop_for_sync(profile_id.clone()),
        )
        .await
        .expect("stop is not queued behind the blocked API request")
        .unwrap();
        assert!(stop.was_launched);
        assert_eq!(controller.stops.load(Ordering::SeqCst), 1);

        renew_release.notify_one();
        handle.complete_stop(profile_id).await.unwrap();
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn blocked_lease_acquire_does_not_block_other_runtime_commands() {
        let existing_path = profile_path("acquire-head-of-line-existing");
        let waiting_path = profile_path("acquire-head-of-line-waiting");
        let controller = TestController::default();
        let backend = Arc::new(TestCoordination::new(controller.log.clone()));
        backend.block_acquire.store(true, Ordering::SeqCst);
        let acquire_started = backend.acquire_started.clone();
        let acquire_release = backend.acquire_release.clone();
        let handle = spawn_profile_runtime(controller, Some(managed_coordination(backend)));
        let existing_id = ProfileId::from_str("prf_local_existing").unwrap();
        let waiting_id = ProfileId::from_str("prf_waiting_for_lease").unwrap();

        handle
            .launch(request_for(&existing_path, "prf_local_existing", false))
            .await
            .unwrap();
        let waiting_handle = handle.clone();
        let waiting_path_for_launch = waiting_path.clone();
        let waiting_launch = tokio::spawn(async move {
            waiting_handle
                .launch(request_for(
                    &waiting_path_for_launch,
                    "prf_waiting_for_lease",
                    true,
                ))
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), acquire_started.notified())
            .await
            .expect("lease acquire should start in its own task");

        let stop_result =
            tokio::time::timeout(Duration::from_secs(2), handle.stop(existing_id)).await;
        acquire_release.notify_one();
        waiting_launch.await.unwrap().unwrap();
        handle.stop(waiting_id).await.unwrap();
        handle.shutdown().await.unwrap();

        assert!(
            stop_result.is_ok(),
            "runtime stop was blocked by lease acquire"
        );
        fs::remove_dir_all(existing_path).unwrap();
        fs::remove_dir_all(waiting_path).unwrap();
    }

    #[tokio::test]
    async fn stop_during_lease_acquire_cancels_launch_before_process_start() {
        let path = profile_path("cancel-launch-during-acquire");
        let controller = TestController::default();
        let backend = Arc::new(TestCoordination::new(controller.log.clone()));
        backend.block_acquire.store(true, Ordering::SeqCst);
        let acquire_started = backend.acquire_started.clone();
        let acquire_release = backend.acquire_release.clone();
        let handle = spawn_profile_runtime(controller.clone(), Some(managed_coordination(backend)));
        let profile_id = ProfileId::from_str("prf_cancelled_launch").unwrap();

        let launch_handle = handle.clone();
        let launch_path = path.clone();
        let launch = tokio::spawn(async move {
            launch_handle
                .launch(request_for(&launch_path, "prf_cancelled_launch", true))
                .await
        });
        tokio::time::timeout(Duration::from_secs(1), acquire_started.notified())
            .await
            .expect("lease acquire should start in its own task");

        handle.stop(profile_id).await.unwrap();
        acquire_release.notify_one();
        let launch_error = launch.await.unwrap().unwrap_err();
        handle.shutdown().await.unwrap();

        assert_eq!(launch_error.code, "profile_launch_cancelled");
        assert_eq!(controller.launches.load(Ordering::SeqCst), 0);
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn stopped_local_lease_can_be_reused_for_reopen_without_api_reacquire() {
        let path = profile_path("reuse-stopped-lease");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(60)), Ordering::SeqCst);
        let handle = spawn_profile_runtime(controller.clone(), Some(managed_coordination(backend)));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();

        let first_launch = handle.launch(request(&path, true)).await.unwrap();
        handle.stop_for_sync(profile_id.clone()).await.unwrap();
        let lease = handle
            .take_stopped_lease_for_reopen(profile_id.clone())
            .await
            .unwrap()
            .expect("stopped profile retains its lease for a safe local reopen");
        assert!(handle.running_profiles().await.unwrap().is_empty());

        let mut reopen = request(&path, true);
        reopen.preacquired_lease = Some(lease);
        let second_launch = handle.launch(reopen).await.unwrap();
        assert_ne!(first_launch.session_epoch, second_launch.session_epoch);

        let stale_stop = match handle
            .stop_for_session(profile_id.clone(), first_launch.session_epoch)
            .await
        {
            Ok(_) => panic!("stale stop unexpectedly affected the reopened session"),
            Err(error) => error,
        };
        assert_eq!(stale_stop.code, "profile_session_changed");
        assert_eq!(
            handle.active_profiles().await.unwrap(),
            vec![profile_id.clone()]
        );

        handle
            .stop_for_session(profile_id.clone(), second_launch.session_epoch)
            .await
            .unwrap();
        handle
            .complete_stop_for_session(profile_id.clone(), first_launch.session_epoch)
            .await
            .unwrap();
        assert_eq!(
            handle.running_profiles().await.unwrap(),
            vec![profile_id.clone()],
            "a stale completion must not remove the newer stopped session"
        );
        handle
            .complete_stop_for_session(profile_id.clone(), second_launch.session_epoch)
            .await
            .unwrap();

        assert_eq!(controller.launches.load(Ordering::SeqCst), 2);
        assert_eq!(
            log.lock()
                .unwrap()
                .iter()
                .filter(|event| **event == "acquire")
                .count(),
            1,
            "reopen reuses the lease instead of waiting for another API acquire"
        );
        handle.stop(profile_id).await.unwrap();
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn blocked_remote_release_does_not_block_runtime_after_local_stop() {
        let path = profile_path("detached-release");
        let other_path = profile_path("release-does-not-block-other-profile");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(60)), Ordering::SeqCst);
        backend.block_release.store(true, Ordering::SeqCst);
        let release_started = backend.release_started.clone();
        let release_release = backend.release_release.clone();
        let handle = spawn_profile_runtime(controller.clone(), Some(managed_coordination(backend)));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        let other_id = ProfileId::from_str("prf_local_other").unwrap();

        handle.launch(request(&path, true)).await.unwrap();
        handle
            .launch(request_for(&other_path, "prf_local_other", false))
            .await
            .unwrap();
        handle.stop_for_sync(profile_id.clone()).await.unwrap();
        let completion_handle = handle.clone();
        let completion =
            tokio::spawn(async move { completion_handle.complete_stop(profile_id).await });
        tokio::time::timeout(Duration::from_secs(2), release_started.notified())
            .await
            .expect("background release was started");

        assert_eq!(
            handle.running_profiles().await.unwrap(),
            vec![other_id.clone()]
        );
        assert_eq!(controller.stops.load(Ordering::SeqCst), 1);
        let other_stop = tokio::time::timeout(Duration::from_secs(2), handle.stop(other_id)).await;
        release_release.notify_one();
        completion.await.unwrap().unwrap();
        assert!(
            other_stop.is_ok(),
            "remote release blocked another profile stop"
        );
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
        fs::remove_dir_all(other_path).unwrap();
    }

    #[tokio::test]
    async fn exited_browser_is_not_reported_as_active_but_keeps_pending_sync_entry() {
        let path = profile_path("exited-before-stop");
        let controller = TestController::default();
        let handle = spawn_profile_runtime(
            controller.clone(),
            Some(managed_coordination(Arc::new(TestCoordination::new(
                Arc::new(Mutex::new(Vec::new())),
            )))),
        );
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();

        handle
            .launch(request(&path, true))
            .await
            .expect("managed launch succeeds");
        controller.process_exited.store(true, Ordering::SeqCst);

        assert!(handle.active_profiles().await.unwrap().is_empty());
        assert_eq!(
            handle.running_profiles().await.unwrap(),
            vec![profile_id.clone()]
        );

        let stopped = handle.stop_for_sync(profile_id.clone()).await.unwrap();
        assert!(stopped.was_launched);
        handle.complete_stop(profile_id.clone()).await.unwrap();
        assert!(handle.running_profiles().await.unwrap().is_empty());

        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn unverified_tracked_browser_remains_active_until_exit_is_confirmed() {
        let path = profile_path("unverified-process-remains-active");
        let controller = TestController::default();
        let handle = spawn_profile_runtime(controller.clone(), None);
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();

        handle.launch(request(&path, false)).await.unwrap();
        controller
            .process_status_unknown
            .store(true, Ordering::SeqCst);

        assert_eq!(handle.active_profiles().await.unwrap(), vec![profile_id]);
        assert_eq!(handle.running_profiles().await.unwrap().len(), 1);
        assert_eq!(controller.stops.load(Ordering::SeqCst), 0);

        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn lost_lease_release_clears_stopped_local_runtime_lock() {
        let path = profile_path("stale-lease-stop");
        fs::write(path.join("profile-data"), b"keep local profile data").unwrap();
        let controller = TestController::default();
        let backend = Arc::new(TestCoordination::new(Arc::new(Mutex::new(Vec::new()))));
        *backend.release_error_code.lock().unwrap() = Some("lease_not_owner");
        let handle = spawn_profile_runtime(controller, Some(managed_coordination(backend)));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        handle.launch(request(&path, true)).await.unwrap();
        handle.stop_for_sync(profile_id.clone()).await.unwrap();

        handle
            .complete_stop(profile_id.clone())
            .await
            .expect("a lost remote lease must not leave the local profile locked");
        handle
            .complete_stop(profile_id.clone())
            .await
            .expect("completing cleanup again must be idempotent");
        assert!(handle.running_profiles().await.unwrap().is_empty());
        assert_eq!(
            fs::read(path.join("profile-data")).unwrap(),
            b"keep local profile data"
        );

        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn failed_lease_release_still_clears_local_lock_and_preserves_profile_data() {
        let path = profile_path("release-network-failure");
        fs::write(path.join("profile-data"), b"keep local profile data").unwrap();
        let controller = TestController::default();
        let backend = Arc::new(TestCoordination::new(Arc::new(Mutex::new(Vec::new()))));
        backend.release_failures.store(2, Ordering::SeqCst);
        let handle = spawn_profile_runtime(controller, Some(managed_coordination(backend)));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        handle.launch(request(&path, true)).await.unwrap();
        handle.stop_for_sync(profile_id.clone()).await.unwrap();

        let error = handle.complete_stop(profile_id.clone()).await.unwrap_err();
        assert_eq!(error.code, "profile_runtime_unavailable");
        assert!(handle.running_profiles().await.unwrap().is_empty());
        assert_eq!(
            fs::read(path.join("profile-data")).unwrap(),
            b"keep local profile data"
        );

        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn failed_sync_stays_queued_for_automatic_retry_and_keeps_dirty_files() {
        let path = profile_path("sync-failure-cleanup");
        fs::create_dir_all(path.join("Default")).unwrap();
        fs::write(path.join("Default/Cookies"), b"dirty cookie data").unwrap();
        let controller = TestController::default();
        let log = controller.log.clone();
        let handle = spawn_profile_runtime(
            controller,
            Some(managed_coordination(Arc::new(TestCoordination::new(
                log.clone(),
            )))),
        );
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        handle.launch(request(&path, true)).await.unwrap();
        handle.stop_for_sync(profile_id.clone()).await.unwrap();
        assert!(handle.active_profiles().await.unwrap().is_empty());
        assert_eq!(
            handle.running_profiles().await.unwrap(),
            vec![profile_id.clone()]
        );

        let injected = Problem::new(
            "injected_sync_failure",
            "Injected sync failure",
            503,
            "test",
        );
        let result: Result<(), Problem> = handle
            .finish_sync_result(profile_id.clone(), Err(injected))
            .await;
        assert_eq!(result.unwrap_err().code, "injected_sync_failure");
        handle.queue_sync_retry(profile_id.clone()).await;
        handle.queue_sync_retry(profile_id.clone()).await;
        assert!(handle.running_profiles().await.unwrap().is_empty());
        assert!(handle.active_profiles().await.unwrap().is_empty());
        assert_eq!(
            handle.unexpectedly_stopped_profiles().await.unwrap(),
            vec![profile_id.clone()]
        );
        assert_eq!(
            fs::read(path.join("Default/Cookies")).unwrap(),
            b"dirty cookie data"
        );
        assert!(log.lock().unwrap().contains(&"release"));

        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn expired_sync_lease_releases_lock_but_keeps_retry_intent() {
        let path = profile_path("expired-sync-retry-intent");
        let controller = TestController::default();
        let backend = Arc::new(TestCoordination::new(controller.log.clone()));
        let mut runtime = runtime(controller, managed_coordination(backend));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.launch(request(&path, true)).await.unwrap();
        runtime.stop_for_sync(&profile_id).await.unwrap();
        runtime.unexpectedly_stopped.insert(profile_id.clone());
        runtime
            .running
            .get_mut(&profile_id)
            .unwrap()
            .sync_started_at = Some(Instant::now() - MAX_STOP_SYNC_HOLD);
        runtime.renew().await;

        assert!(!runtime.running.contains_key(&profile_id));
        assert!(runtime.unexpectedly_stopped.contains(&profile_id));
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn active_sync_attempt_renews_past_idle_hold_then_releases_for_retry() {
        let path = profile_path("active-sync-attempt-lease");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        let mut runtime = runtime(controller, managed_coordination(backend.clone()));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.launch(request(&path, true)).await.unwrap();
        runtime.stop_for_sync(&profile_id).await.unwrap();
        runtime.set_sync_attempt_active(&profile_id, true).unwrap();
        assert!(runtime.sync_attempt_active(&profile_id));
        runtime
            .running
            .get_mut(&profile_id)
            .unwrap()
            .sync_started_at = Some(Instant::now() - MAX_STOP_SYNC_HOLD);
        runtime
            .running
            .get_mut(&profile_id)
            .unwrap()
            .lease
            .as_mut()
            .unwrap()
            .expires_at_ms = unix_time_millis("test").unwrap() + 30_000;

        runtime.renew().await;

        assert!(runtime.running.contains_key(&profile_id));
        assert_eq!(backend.renewals.load(Ordering::SeqCst), 1);

        runtime.set_sync_attempt_active(&profile_id, false).unwrap();
        assert!(!runtime.sync_attempt_active(&profile_id));
        runtime.renew().await;
        assert!(!runtime.running.contains_key(&profile_id));
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn heartbeat_detects_closed_browser_and_bounds_its_sync_lease() {
        let path = profile_path("heartbeat-reconciles-exit");
        let controller = TestController::default();
        let backend = Arc::new(TestCoordination::new(controller.log.clone()));
        let mut runtime = runtime(controller.clone(), managed_coordination(backend));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.launch(request(&path, true)).await.unwrap();
        controller.process_exited.store(true, Ordering::SeqCst);

        // No UI status poll or Stop button is needed to detect process exit.
        runtime.renew().await;

        let stopped = runtime.running.get(&profile_id).unwrap();
        assert!(stopped.process_stopped);
        assert!(stopped.sync_pending);
        assert!(stopped.sync_started_at.is_some());
        assert!(runtime.unexpectedly_stopped.contains(&profile_id));

        runtime
            .running
            .get_mut(&profile_id)
            .unwrap()
            .sync_started_at = Some(Instant::now() - MAX_STOP_SYNC_HOLD);
        runtime.renew().await;

        assert!(!runtime.running.contains_key(&profile_id));
        assert!(controller.log.lock().unwrap().contains(&"release"));
        assert!(runtime.unexpectedly_stopped.contains(&profile_id));
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn managed_launch_without_authority_fails_before_process_spawn() {
        let path = profile_path("managed-unavailable");
        let controller = TestController::default();
        let handle = spawn_profile_runtime(controller.clone(), None);

        let error = handle.launch(request(&path, true)).await.unwrap_err();
        assert_eq!(error.code, "managed_lease_unavailable");
        assert_eq!(controller.launches.load(Ordering::SeqCst), 0);
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn coordination_rebind_enables_managed_launch_after_unauthenticated_start() {
        let path = profile_path("coordination-rebind");
        let controller = TestController::default();
        let log = controller.log.clone();
        let handle = spawn_profile_runtime(controller.clone(), None);

        let unavailable = handle.launch(request(&path, true)).await.unwrap_err();
        assert_eq!(unavailable.code, "managed_lease_unavailable");
        assert_eq!(controller.launches.load(Ordering::SeqCst), 0);

        handle
            .set_coordination(Some(managed_coordination(Arc::new(TestCoordination::new(
                log.clone(),
            )))))
            .await
            .unwrap();
        let receipt = handle.launch(request(&path, true)).await.unwrap();
        assert_eq!(receipt.write_authority.unwrap().fencing_token, 7);
        assert_eq!(*log.lock().unwrap(), vec!["acquire", "launch"]);

        handle
            .stop(ProfileId::from_str("prf_runtime_1").unwrap())
            .await
            .unwrap();
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn coordination_reconfigure_is_rejected_atomically_while_profile_runs() {
        let path = profile_path("coordination-reconfigure-running");
        let controller = TestController::default();
        let log = controller.log.clone();
        let handle = spawn_profile_runtime(controller.clone(), None);
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        handle.launch(request(&path, false)).await.unwrap();

        let error = handle
            .set_coordination(Some(managed_coordination(Arc::new(TestCoordination::new(
                log,
            )))))
            .await
            .unwrap_err();
        assert_eq!(error.code, "runtime_reconfigure_running");
        assert_eq!(error.status, 409);

        handle.stop(profile_id).await.unwrap();
        let unavailable = handle.launch(request(&path, true)).await.unwrap_err();
        assert_eq!(unavailable.code, "managed_lease_unavailable");
        assert_eq!(controller.launches.load(Ordering::SeqCst), 1);
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn same_actor_coordination_refresh_keeps_running_profile_lease() {
        let path = profile_path("coordination-refresh-running");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log));
        let handle = spawn_profile_runtime(
            controller.clone(),
            Some(managed_coordination(backend.clone())),
        );
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        handle.launch(request(&path, true)).await.unwrap();

        handle
            .set_coordination(Some(managed_coordination_with_bearer(
                backend,
                "rotated-bearer",
            )))
            .await
            .expect("refreshing credentials for the same account preserves its lease");
        assert_eq!(
            handle.running_profiles().await.unwrap(),
            vec![profile_id.clone()]
        );

        handle.stop(profile_id).await.unwrap();
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn managed_launch_and_stop_preserve_lease_process_order() {
        let path = profile_path("managed-order");
        let controller = TestController::default();
        let log = controller.log.clone();
        let coordination = managed_coordination(Arc::new(TestCoordination::new(log.clone())));
        let handle = spawn_profile_runtime(controller, Some(coordination));

        let receipt = handle.launch(request(&path, true)).await.unwrap();
        assert_eq!(receipt.write_authority.unwrap().fencing_token, 7);
        assert_eq!(*log.lock().unwrap(), vec!["acquire", "launch"]);

        handle
            .stop(ProfileId::from_str("prf_runtime_1").unwrap())
            .await
            .unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            vec!["acquire", "launch", "stop", "release"]
        );
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn two_phase_stop_holds_lease_until_sync_completes() {
        let path = profile_path("two-phase-stop");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        let mut runtime = runtime(controller, managed_coordination(backend));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.launch(request(&path, true)).await.unwrap();

        let receipt = runtime.stop_for_sync(&profile_id).await.unwrap();

        assert_eq!(receipt.profile_path, path);
        assert!(receipt.lease.is_some());
        assert_eq!(*log.lock().unwrap(), vec!["acquire", "launch", "stop"]);

        runtime.complete_stop(&profile_id).await.unwrap();
        assert_eq!(
            *log.lock().unwrap(),
            vec!["acquire", "launch", "stop", "release"]
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn stopped_profile_keeps_lease_renewing_until_sync_completes() {
        let path = profile_path("sync-lease-renewal");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(60)), Ordering::SeqCst);
        let renewals = Arc::clone(&backend.renewals);
        let coordination = ManagedCoordination {
            backend: backend.clone(),
            auth: CoordinationAuth::new("actor-runtime", "private-bearer").unwrap(),
            lease_ttl: Duration::from_secs(1),
            heartbeat_interval: Duration::from_millis(10),
        };
        let handle = spawn_profile_runtime(controller, Some(coordination));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        handle.launch(request(&path, true)).await.unwrap();
        handle.stop_for_sync(profile_id.clone()).await.unwrap();

        assert_eq!(
            handle.running_profiles().await.unwrap(),
            vec![profile_id.clone()]
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while renewals.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("sync-pending lease should renew at least twice");
        let during_sync = log.lock().unwrap().clone();
        assert!(renewals.load(Ordering::SeqCst) >= 2);
        assert!(!during_sync.contains(&"release"));

        handle.complete_stop(profile_id).await.unwrap();
        assert!(log.lock().unwrap().contains(&"release"));
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn idle_profile_sync_lease_is_renewed_without_starting_a_browser() {
        let path = profile_path("idle-sync-lease-renewal");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(60)), Ordering::SeqCst);
        let renewals = Arc::clone(&backend.renewals);
        let coordination = ManagedCoordination {
            backend: backend.clone(),
            auth: CoordinationAuth::new("actor-runtime", "private-bearer").unwrap(),
            lease_ttl: Duration::from_secs(1),
            heartbeat_interval: Duration::from_millis(10),
        };
        let handle = spawn_profile_runtime(controller, Some(coordination));
        let profile_id = ProfileId::from_str("prf_idle_sync_1").unwrap();
        let workspace_id = WorkspaceId::from_str("ws_idle_sync_1").unwrap();
        let resource_id = ResourceId::from_str("res_idle_sync_1").unwrap();
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let lease = LeaseGrant {
            resource_id: resource_id.clone(),
            workspace_id: workspace_id.clone(),
            owner_id: "actor-runtime".into(),
            owner_token: "private-owner-token".into(),
            fencing_token: 7,
            issued_at_ms: now_ms,
            expires_at_ms: now_ms + 60_000,
        };

        let receipt = handle
            .hold_idle_profile_for_sync(
                profile_id.clone(),
                path.clone(),
                LeaseScope {
                    workspace_id,
                    resource_id,
                },
                lease,
            )
            .await
            .unwrap();

        assert_eq!(receipt.profile_path, path);
        assert_eq!(receipt.lease.unwrap().fencing_token, 7);
        assert_eq!(
            handle.running_profiles().await.unwrap(),
            vec![profile_id.clone()]
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while renewals.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert!(renewals.load(Ordering::SeqCst) >= 2);
        let during_sync = log.lock().unwrap().clone();
        assert!(!during_sync.contains(&"launch"));
        assert!(!during_sync.contains(&"stop"));
        assert!(!during_sync.contains(&"release"));

        handle.complete_stop(profile_id).await.unwrap();
        assert!(log.lock().unwrap().contains(&"release"));
        handle.shutdown().await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn lease_acquire_failure_does_not_launch_process() {
        let path = profile_path("acquire-failure");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend.fail_acquire.store(true, Ordering::SeqCst);
        let mut runtime = runtime(controller.clone(), managed_coordination(backend));

        runtime.launch(request(&path, true)).await.unwrap_err();

        assert_eq!(controller.launches.load(Ordering::SeqCst), 0);
        assert_eq!(*log.lock().unwrap(), vec!["acquire"]);
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn process_launch_failure_releases_acquired_lease() {
        let path = profile_path("launch-failure");
        let controller = TestController::default();
        controller.fail_launch.store(true, Ordering::SeqCst);
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        let mut runtime = runtime(controller, managed_coordination(backend));

        runtime.launch(request(&path, true)).await.unwrap_err();

        assert_eq!(*log.lock().unwrap(), vec!["acquire", "launch", "release"]);
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn process_stop_failure_does_not_release_lease() {
        let path = profile_path("stop-failure");
        let controller = TestController::default();
        controller.stop_failures.store(1, Ordering::SeqCst);
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        let mut runtime = runtime(controller, managed_coordination(backend));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.launch(request(&path, true)).await.unwrap();

        runtime.stop(&profile_id).await.unwrap_err();

        assert_eq!(*log.lock().unwrap(), vec!["acquire", "launch", "stop"]);
        runtime.stop(&profile_id).await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn lease_release_failure_clears_local_lock_after_bounded_retry() {
        let path = profile_path("release-failure");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        // release() retries one retryable failure with the same idempotency key.
        // If both bounded attempts fail, the stopped browser must still unlock
        // locally; the server lease expires and the profile files remain intact.
        backend.release_failures.store(2, Ordering::SeqCst);
        let mut runtime = runtime(controller, managed_coordination(backend));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.launch(request(&path, true)).await.unwrap();

        runtime.stop(&profile_id).await.unwrap_err();
        assert!(runtime.authority(&profile_id).is_none());
        runtime.stop(&profile_id).await.unwrap();

        assert!(runtime.authority(&profile_id).is_none());
        assert_eq!(
            *log.lock().unwrap(),
            vec!["acquire", "launch", "stop", "release", "release"]
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn renewal_failure_stops_process_and_keeps_dirty_lease() {
        let path = profile_path("renewal-failure");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(20)), Ordering::SeqCst);
        backend.fail_renew.store(true, Ordering::SeqCst);
        let mut runtime = runtime(controller, managed_coordination(backend.clone()));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.launch(request(&path, true)).await.unwrap();

        runtime.renew().await;

        assert!(runtime.authority(&profile_id).is_some());
        assert!(!runtime.running[&profile_id].process_stopped);
        runtime
            .running
            .get_mut(&profile_id)
            .unwrap()
            .lease
            .as_mut()
            .unwrap()
            .expires_at_ms = lease_valid_for(Duration::from_secs(10));
        runtime.renew().await;

        assert!(runtime.running[&profile_id].process_stopped);
        assert_eq!(
            *log.lock().unwrap(),
            vec!["acquire", "launch", "renew", "renew", "stop"]
        );
        fs::remove_dir_all(path).unwrap();
    }

    fn lease_valid_for(duration: Duration) -> u64 {
        u64::try_from(
            (SystemTime::now() + duration)
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn transient_renewal_failure_keeps_the_browser_running_while_the_lease_is_valid() {
        let path = profile_path("renewal-transient");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(45)), Ordering::SeqCst);
        backend.fail_renew.store(true, Ordering::SeqCst);
        let mut runtime = runtime(controller, managed_coordination(backend));
        runtime.launch(request(&path, true)).await.unwrap();

        runtime.renew().await;
        runtime.renew().await;

        assert!(
            !log.lock().unwrap().contains(&"stop"),
            "{:?}",
            log.lock().unwrap()
        );
        assert!(
            runtime
                .authority(&ProfileId::from_str("prf_runtime_1").unwrap())
                .is_some()
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn renewal_failure_stops_before_lease_enters_the_expiry_safety_window() {
        let path = profile_path("renewal-safety-window");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(10)), Ordering::SeqCst);
        backend.fail_renew.store(true, Ordering::SeqCst);
        let mut runtime = runtime(controller, managed_coordination(backend));
        runtime.launch(request(&path, true)).await.unwrap();

        runtime.renew().await;

        assert!(log.lock().unwrap().contains(&"stop"));
        assert!(runtime.running[&ProfileId::from_str("prf_runtime_1").unwrap()].process_stopped);
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn expired_lease_stops_the_browser_but_keeps_the_sync_pending() {
        let path = profile_path("renewal-expired");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(10)), Ordering::SeqCst);
        backend.fail_renew.store(true, Ordering::SeqCst);
        let mut runtime = runtime(controller, managed_coordination(backend));
        runtime.launch(request(&path, true)).await.unwrap();
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();

        runtime.renew().await;
        runtime.renew().await;

        let entries = log.lock().unwrap().clone();
        assert_eq!(entries.iter().filter(|entry| **entry == "stop").count(), 1);
        assert!(!entries.contains(&"release"), "{entries:?}");
        assert!(
            runtime
                .running
                .get(&profile_id)
                .is_some_and(|running| running.sync_pending)
        );
        assert!(runtime.unexpectedly_stopped.contains(&profile_id));
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn lost_lease_ownership_stops_the_browser_immediately() {
        let path = profile_path("renewal-not-owner");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(45)), Ordering::SeqCst);
        backend.fail_renew.store(true, Ordering::SeqCst);
        *backend.renew_error_code.lock().unwrap() = Some("lease_not_owner");
        let mut runtime = runtime(controller, managed_coordination(backend));
        runtime.launch(request(&path, true)).await.unwrap();

        runtime.renew().await;

        assert!(log.lock().unwrap().contains(&"stop"));
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn stale_renewal_completion_does_not_clear_current_lease_pending_state() {
        let path = profile_path("stale-renewal-completion");
        let controller = TestController::default();
        let backend = Arc::new(TestCoordination::new(controller.log.clone()));
        let mut runtime = runtime(controller, managed_coordination(backend));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.launch(request(&path, true)).await.unwrap();

        let old_session_epoch = runtime.running[&profile_id].session_epoch;
        runtime.stop_for_sync(&profile_id).await.unwrap();
        let reused_lease = runtime
            .take_stopped_lease_for_reopen(&profile_id)
            .unwrap()
            .unwrap();
        let mut reopen = request(&path, true);
        reopen.preacquired_lease = Some(reused_lease.clone());
        runtime.launch(reopen).await.unwrap();

        let current = runtime.running.get(&profile_id).unwrap();
        let current_session_epoch = current.session_epoch;
        assert_ne!(old_session_epoch, current_session_epoch);
        let pending_identity = (
            current_session_epoch,
            reused_lease.fencing_token,
            reused_lease.owner_token.clone(),
        );
        runtime
            .pending_renewals
            .insert(profile_id.clone(), pending_identity.clone());

        runtime.apply_renewal(
            profile_id.clone(),
            old_session_epoch,
            reused_lease,
            Err(Problem::new("temporary_failure", "retry", 503, "test").retryable(true)),
        );

        assert_eq!(
            runtime.pending_renewals.get(&profile_id),
            Some(&pending_identity)
        );
        assert!(!runtime.running[&profile_id].process_stopped);
        runtime.stop(&profile_id).await.unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn stopped_profile_whose_lease_was_taken_over_is_dropped() {
        let path = profile_path("renewal-taken-over");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(60)), Ordering::SeqCst);
        backend.fail_renew.store(true, Ordering::SeqCst);
        let mut runtime = runtime(controller, managed_coordination(backend.clone()));
        runtime.launch(request(&path, true)).await.unwrap();
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.stop_for_sync(&profile_id).await.unwrap();
        runtime.renew().await;
        assert!(runtime.running.contains_key(&profile_id));

        *backend.renew_error_code.lock().unwrap() = Some("lease_not_owner");
        runtime.renew().await;

        assert!(!runtime.running.contains_key(&profile_id));
        assert!(!runtime.unexpectedly_stopped.contains(&profile_id));
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn missing_stopped_sync_lease_preserves_automatic_retry_intent() {
        let path = profile_path("renewal-missing-stopped-lease");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log));
        backend.fail_renew.store(true, Ordering::SeqCst);
        backend
            .lease_expires_at_ms
            .store(lease_valid_for(Duration::from_secs(60)), Ordering::SeqCst);
        let mut runtime = runtime(controller, managed_coordination(backend.clone()));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.launch(request(&path, true)).await.unwrap();
        runtime.stop_for_sync(&profile_id).await.unwrap();

        *backend.renew_error_code.lock().unwrap() = Some("lease_not_found");
        runtime.renew().await;

        assert!(!runtime.running.contains_key(&profile_id));
        assert!(runtime.unexpectedly_stopped.contains(&profile_id));
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn write_authority_does_not_expose_secret_tokens() {
        let path = profile_path("authority-secrets");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log));
        let mut runtime = runtime(controller, managed_coordination(backend));

        let receipt = runtime.launch(request(&path, true)).await.unwrap();
        let exposed = format!("{:?}", receipt.write_authority);

        assert!(!exposed.contains("private-owner-token"));
        assert!(!exposed.contains("private-bearer"));
        runtime
            .stop(&ProfileId::from_str("prf_runtime_1").unwrap())
            .await
            .unwrap();
        fs::remove_dir_all(path).unwrap();
    }

    #[tokio::test]
    async fn shutdown_attempts_every_profile_after_one_stop_failure() {
        let first_path = profile_path("shutdown-first");
        let second_path = profile_path("shutdown-second");
        let controller = TestController::default();
        controller.stop_failures.store(1, Ordering::SeqCst);
        let handle = spawn_profile_runtime(controller.clone(), None);
        handle
            .launch(request_for(&first_path, "prf_runtime_1", false))
            .await
            .unwrap();
        handle
            .launch(request_for(&second_path, "prf_runtime_2", false))
            .await
            .unwrap();

        handle.shutdown().await.unwrap_err();

        assert_eq!(controller.stops.load(Ordering::SeqCst), 2);
        fs::remove_dir_all(first_path).unwrap();
        fs::remove_dir_all(second_path).unwrap();
    }
}
