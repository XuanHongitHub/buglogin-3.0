use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};

use buglogin_browser::{
    lifecycle::LifecycleManager, process::ProcessController, runtime::LaunchSpecification,
};
use buglogin_domain::{
    errors::Problem,
    ids::{ProfileId, ResourceId, WorkspaceId},
};
use buglogin_sync::coordination::{
    CoordinationAuth, CoordinationBackend, CoordinationScope, LeaseGrant,
};
use buglogin_sync::manifest::SyncManifest;
use tokio::sync::{mpsc, oneshot};

use crate::proxy_bridge::ProxyBridgeHandle;

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
    async fn renew_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
    ) -> Result<LeaseGrant, Problem>;
    async fn release_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
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

    async fn renew_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
        ttl: Duration,
    ) -> Result<LeaseGrant, Problem> {
        CoordinationBackend::renew_lease(self, scope, resource_id, owner_token, ttl).await
    }

    async fn release_lease(
        &self,
        scope: CoordinationScope<'_>,
        resource_id: &ResourceId,
        owner_token: &str,
    ) -> Result<(), Problem> {
        CoordinationBackend::release_lease(self, scope, resource_id, owner_token).await
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
}

#[derive(Clone)]
pub struct StopReceipt {
    pub(crate) profile_path: PathBuf,
    pub(crate) lease: Option<LeaseGrant>,
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
        let _ = self.stop_for_sync(profile_id.clone()).await?;
        self.complete_stop(profile_id).await
    }

    pub async fn stop_for_sync(&self, profile_id: ProfileId) -> Result<StopReceipt, Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::StopForSync { profile_id, reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
    }

    pub async fn complete_stop(&self, profile_id: ProfileId) -> Result<(), Problem> {
        let (reply, response) = oneshot::channel();
        self.commands
            .send(RuntimeCommand::CompleteStop { profile_id, reply })
            .await
            .map_err(|_| unavailable())?;
        response.await.map_err(|_| unavailable())?
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
    StopForSync {
        profile_id: ProfileId,
        reply: oneshot::Sender<Result<StopReceipt, Problem>>,
    },
    CompleteStop {
        profile_id: ProfileId,
        reply: oneshot::Sender<Result<(), Problem>>,
    },
    Authority {
        profile_id: ProfileId,
        reply: oneshot::Sender<Option<WriteAuthority>>,
    },
    RunningProfiles {
        reply: oneshot::Sender<Vec<ProfileId>>,
    },
    SetCoordination {
        coordination: Option<ManagedCoordination>,
        reply: oneshot::Sender<Result<(), Problem>>,
    },
    Shutdown {
        reply: oneshot::Sender<Result<(), Problem>>,
    },
}

struct RunningProfile {
    profile_path: PathBuf,
    lease: Option<LeaseGrant>,
    process_stopped: bool,
    sync_pending: bool,
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
    tokio::spawn(run(ProfileRuntime {
        lifecycle: LifecycleManager::new(controller),
        coordination,
        running: HashMap::new(),
        receiver,
    }));
    ProfileRuntimeHandle { commands }
}

struct ProfileRuntime<C: ProcessController> {
    lifecycle: LifecycleManager<C>,
    coordination: Option<ManagedCoordination>,
    running: HashMap<ProfileId, RunningProfile>,
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
                    let _ = reply.send(runtime.launch(request).await);
                }
                Some(RuntimeCommand::StopForSync { profile_id, reply }) => {
                    let _ = reply.send(runtime.stop_for_sync(&profile_id).await);
                }
                Some(RuntimeCommand::CompleteStop { profile_id, reply }) => {
                    let _ = reply.send(runtime.complete_stop(&profile_id).await);
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
                Some(RuntimeCommand::SetCoordination { coordination, reply }) => {
                    let result = runtime.set_coordination(coordination);
                    if result.is_ok() {
                        interval = tokio::time::interval(runtime.heartbeat_interval());
                        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    }
                    let _ = reply.send(result);
                }
                Some(RuntimeCommand::Shutdown { reply }) => {
                    let result = runtime.shutdown().await;
                    let _ = reply.send(result);
                    break;
                }
                None => break,
            },
            _ = interval.tick(), if runtime.coordination.is_some() => runtime.renew().await,
        }
    }
}

impl<C: ProcessController> ProfileRuntime<C> {
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
        if !self.running.is_empty() {
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

    async fn launch(&mut self, request: LaunchRequest) -> Result<LaunchReceipt, Problem> {
        if self.running.contains_key(&request.profile_id) {
            return Err(Problem::new(
                "profile_already_running",
                "Profile is already running",
                409,
                "desktop",
            ));
        }
        let lease = match request.preacquired_lease {
            Some(lease) => {
                validate_preacquired(
                    self.coordination.as_ref(),
                    request.lease_scope.as_ref(),
                    &lease,
                )?;
                Some(lease)
            }
            None => {
                acquire(
                    self.coordination.clone(),
                    request.lease_scope.as_ref(),
                    &request.idempotency_key,
                )
                .await?
            }
        };
        let mut proxy_bridge = request.proxy_bridge;
        let identity = match self
            .lifecycle
            .launch(&request.profile_path, &request.specification)
        {
            Ok(identity) => identity,
            Err(error) => {
                if let Some(bridge) = proxy_bridge.take() {
                    let _ = bridge.stop().await;
                }
                if let Some(lease) = lease.as_ref() {
                    let _ = release(self.coordination.clone(), lease).await;
                }
                return Err(error);
            }
        };
        let write_authority = lease.as_ref().map(authority);
        self.running.insert(
            request.profile_id,
            RunningProfile {
                profile_path: request.profile_path,
                lease,
                process_stopped: false,
                sync_pending: false,
                proxy_bridge,
            },
        );
        Ok(LaunchReceipt {
            process_id: identity.pid,
            write_authority,
        })
    }

    async fn stop(&mut self, profile_id: &ProfileId) -> Result<(), Problem> {
        let _ = self.stop_for_sync(profile_id).await?;
        self.complete_stop(profile_id).await
    }

    async fn stop_for_sync(&mut self, profile_id: &ProfileId) -> Result<StopReceipt, Problem> {
        let (profile_path, lease, process_stopped) = self
            .running
            .get(profile_id)
            .map(|running| {
                (
                    running.profile_path.clone(),
                    running.lease.clone(),
                    running.process_stopped,
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
            proxy_bridge.stop().await?;
        }
        if let Some(running) = self.running.get_mut(profile_id) {
            running.sync_pending = true;
        }
        Ok(StopReceipt {
            profile_path,
            lease,
        })
    }

    async fn complete_stop(&mut self, profile_id: &ProfileId) -> Result<(), Problem> {
        let lease = self
            .running
            .get(profile_id)
            .and_then(|running| running.lease.clone());
        if let Some(lease) = lease.as_ref() {
            release(self.coordination.clone(), lease).await?;
        }
        self.running.remove(profile_id);
        Ok(())
    }

    fn authority(&self, profile_id: &ProfileId) -> Option<WriteAuthority> {
        self.running
            .get(profile_id)
            .and_then(|running| running.lease.as_ref())
            .map(authority)
    }

    async fn renew(&mut self) {
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
            let renewed = coordination
                .backend
                .renew_lease(
                    CoordinationScope {
                        workspace_id: &lease.workspace_id,
                        auth: &coordination.auth,
                    },
                    &lease.resource_id,
                    &lease.owner_token,
                    coordination.lease_ttl,
                )
                .await;
            match renewed {
                Ok(renewed) if renewed.fencing_token == lease.fencing_token => {
                    if let Some(running) = self.running.get_mut(&profile_id) {
                        running.lease = Some(renewed);
                    }
                }
                _ => {
                    let _ = self.stop_for_sync(&profile_id).await;
                    if let Some(running) = self.running.get_mut(&profile_id) {
                        running.sync_pending = false;
                    }
                }
            }
        }
    }

    async fn shutdown(&mut self) -> Result<(), Problem> {
        let profiles = self.running.keys().cloned().collect::<Vec<_>>();
        let mut first_error = None;
        for profile_id in profiles {
            if let Err(error) = self.stop(&profile_id).await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
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
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
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
    coordination
        .backend
        .release_lease(
            CoordinationScope {
                workspace_id: &lease.workspace_id,
                auth: &coordination.auth,
            },
            &lease.resource_id,
            &lease.owner_token,
        )
        .await
}

fn authority(lease: &LeaseGrant) -> WriteAuthority {
    WriteAuthority {
        workspace_id: lease.workspace_id.clone(),
        resource_id: lease.resource_id.clone(),
        fencing_token: lease.fencing_token,
    }
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

    use buglogin_browser::process::{ManagedProcess, ProcessIdentity, ProcessStatus};
    use buglogin_domain::entities::Runtime;
    use buglogin_sync::{
        coordination::{CoordinationEvent, Presence, PublishedEvent, StreamEvent},
        manifest::SyncManifest,
    };

    use super::*;

    #[derive(Clone, Default)]
    struct TestController {
        fail_launch: Arc<AtomicBool>,
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
                stops: self.stops.clone(),
                log: self.log.clone(),
            }))
        }

        fn inspect(&self, _identity: &ProcessIdentity) -> ProcessStatus {
            ProcessStatus::Running
        }

        fn terminate(&self, _identity: &ProcessIdentity) -> Result<(), Problem> {
            Ok(())
        }
    }

    struct TestProcess {
        identity: ProcessIdentity,
        stop_failures: Arc<AtomicUsize>,
        stops: Arc<AtomicUsize>,
        log: Arc<Mutex<Vec<&'static str>>>,
    }

    impl ManagedProcess for TestProcess {
        fn identity(&self) -> &ProcessIdentity {
            &self.identity
        }

        fn status(&mut self) -> Result<ProcessStatus, Problem> {
            Ok(ProcessStatus::Running)
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
        fail_renew: AtomicBool,
        release_failures: AtomicUsize,
        manifests: Mutex<BTreeMap<ResourceId, SyncManifest>>,
    }

    impl TestCoordination {
        fn new(log: Arc<Mutex<Vec<&'static str>>>) -> Self {
            Self {
                log,
                renewals: Arc::new(AtomicUsize::new(0)),
                fail_acquire: AtomicBool::new(false),
                fail_renew: AtomicBool::new(false),
                release_failures: AtomicUsize::new(0),
                manifests: Mutex::new(BTreeMap::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl CoordinationBackend for TestCoordination {
        async fn acquire_lease(
            &self,
            scope: CoordinationScope<'_>,
            resource_id: &ResourceId,
            _ttl: Duration,
        ) -> Result<LeaseGrant, Problem> {
            self.log.lock().unwrap().push("acquire");
            if self.fail_acquire.load(Ordering::SeqCst) {
                return Err(unavailable());
            }
            Ok(LeaseGrant {
                resource_id: resource_id.clone(),
                workspace_id: scope.workspace_id.clone(),
                owner_id: scope.auth.actor_id().into(),
                owner_token: "private-owner-token".into(),
                fencing_token: 7,
                issued_at_ms: 1,
                expires_at_ms: 60_001,
            })
        }

        async fn renew_lease(
            &self,
            scope: CoordinationScope<'_>,
            resource_id: &ResourceId,
            owner_token: &str,
            _ttl: Duration,
        ) -> Result<LeaseGrant, Problem> {
            self.renewals.fetch_add(1, Ordering::SeqCst);
            if self.fail_renew.load(Ordering::SeqCst) {
                self.log.lock().unwrap().push("renew");
                return Err(unavailable());
            }
            Ok(LeaseGrant {
                resource_id: resource_id.clone(),
                workspace_id: scope.workspace_id.clone(),
                owner_id: scope.auth.actor_id().into(),
                owner_token: owner_token.into(),
                fencing_token: 7,
                issued_at_ms: 1,
                expires_at_ms: 60_002,
            })
        }

        async fn release_lease(
            &self,
            _scope: CoordinationScope<'_>,
            _resource_id: &ResourceId,
            owner_token: &str,
        ) -> Result<(), Problem> {
            assert_eq!(owner_token, "private-owner-token");
            self.log.lock().unwrap().push("release");
            if self
                .release_failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                    remaining.checked_sub(1)
                })
                .is_ok()
            {
                return Err(unavailable());
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
        ManagedCoordination {
            backend,
            auth: CoordinationAuth::new("actor-runtime", "private-bearer").unwrap(),
            lease_ttl: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(3_600),
        }
    }

    fn runtime(
        controller: TestController,
        coordination: ManagedCoordination,
    ) -> ProfileRuntime<TestController> {
        let (_, receiver) = mpsc::channel(1);
        ProfileRuntime {
            lifecycle: LifecycleManager::new(controller),
            coordination: Some(coordination),
            running: HashMap::new(),
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
        assert_eq!(controller.stops.load(Ordering::SeqCst), 1);
        handle.shutdown().await.unwrap();
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
        tokio::time::sleep(Duration::from_millis(60)).await;
        let during_sync = log.lock().unwrap().clone();
        assert!(renewals.load(Ordering::SeqCst) >= 2);
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
    async fn lease_release_failure_keeps_cleanup_pending_and_retries() {
        let path = profile_path("release-failure");
        let controller = TestController::default();
        let log = controller.log.clone();
        let backend = Arc::new(TestCoordination::new(log.clone()));
        backend.release_failures.store(1, Ordering::SeqCst);
        let mut runtime = runtime(controller, managed_coordination(backend));
        let profile_id = ProfileId::from_str("prf_runtime_1").unwrap();
        runtime.launch(request(&path, true)).await.unwrap();

        runtime.stop(&profile_id).await.unwrap_err();
        assert!(runtime.authority(&profile_id).is_some());
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
        backend.fail_renew.store(true, Ordering::SeqCst);
        let mut runtime = runtime(controller, managed_coordination(backend));
        runtime.launch(request(&path, true)).await.unwrap();

        runtime.renew().await;

        assert!(
            runtime
                .authority(&ProfileId::from_str("prf_runtime_1").unwrap())
                .is_some()
        );
        assert_eq!(
            *log.lock().unwrap(),
            vec!["acquire", "launch", "renew", "stop"]
        );
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
