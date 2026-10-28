use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    clock::{checked_add_millis, unix_time_millis},
    profile_transfer_budget,
};
use buglogin_domain::{errors::Problem, ids::ProfileId};
use buglogin_storage::{
    blob::{checksum, read_verified, write_verified},
    paths::confined_path,
    secrets::SecretValue,
};
use buglogin_sync::{
    coordination::{CoordinationAuth, CoordinationScope},
    crypto::{
        AUTHENTICATED_CHUNK_OVERHEAD_BYTES, AuthenticatedChunk, ClientKey, ProxyCredentialEnvelope,
        RecoveryKey, ResourceKeyEnvelope,
    },
    manifest::{
        MAX_CHUNKED_PROFILE_FILE_BYTES, MAX_PROFILE_SYNC_OBJECT_BYTES, ManifestDependency,
        ManifestFileChunk, ManifestFileChunks, ManifestObject, ManifestObjectReference,
        ManifestPayloadEncryption, ManifestTombstone, PROFILE_FILE_CHUNK_BYTES,
        PayloadEncryptionAlgorithm, RESOURCE_KEY_ENVELOPE_KEY, SyncManifest,
        is_ephemeral_profile_path,
    },
    payload::{PAYLOAD_SCHEMA_VERSION, PayloadDescriptor, PayloadObject},
};
use fs2::FileExt;
use futures_util::{StreamExt, TryStreamExt, stream};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;

use crate::remote_api::{RemoteApiClient, RemoteProxySyncSnapshot};

const MARKER: &str = ".buglogin-projection.json";
const ACTIVE_SESSION_MARKER: &str = ".buglogin-active-session.json";
const SYNC_COMMIT_MARKER: &str = ".buglogin-sync-commit.json";
const HYDRATION_FENCE_MARKER: &str = ".buglogin-hydration-fence.json";
const MAX_ENCRYPTED_OBJECT_BYTES: usize = MAX_PROFILE_SYNC_OBJECT_BYTES as usize;
const MAX_PROFILE_PLAINTEXT_BYTES: u64 =
    MAX_PROFILE_SYNC_OBJECT_BYTES - AUTHENTICATED_CHUNK_OVERHEAD_BYTES as u64;
const MAX_IN_MEMORY_SNAPSHOT_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CONCURRENT_PROFILE_DOWNLOADS: usize = 4;
static PROFILE_DOWNLOAD_SLOTS: OnceLock<Arc<Semaphore>> = OnceLock::new();

fn profile_download_slots() -> Arc<Semaphore> {
    Arc::clone(
        PROFILE_DOWNLOAD_SLOTS
            .get_or_init(|| Arc::new(Semaphore::new(MAX_CONCURRENT_PROFILE_DOWNLOADS))),
    )
}

pub struct HydratedProxyCredential {
    pub proxy_id: buglogin_domain::ids::ProxyId,
    pub revision: u64,
    pub secret: SecretValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectionReceipt {
    pub profile_id: ProfileId,
    pub generation: u64,
    pub root_checksum: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotBlob {
    pub key: String,
    pub path: PathBuf,
    pub size: u64,
    pub checksum: String,
}

#[derive(Default)]
struct ProfileDownloadMetrics {
    objects: u64,
    bytes: u64,
    outcome: &'static str,
}

impl ProfileDownloadMetrics {
    fn record(&mut self, bytes: usize) {
        self.objects = self.objects.saturating_add(1);
        self.bytes = self.bytes.saturating_add(bytes as u64);
    }
}

impl Drop for ProfileDownloadMetrics {
    fn drop(&mut self) {
        tauri_plugin_log::log::info!(
            "Profile open transfer summary: outcome={} download_objects={} download_bytes={}",
            if self.outcome.is_empty() {
                "failed"
            } else {
                self.outcome
            },
            self.objects,
            self.bytes
        );
    }
}

impl SnapshotBlob {
    pub fn read_bytes(&self) -> Result<Vec<u8>, Problem> {
        let metadata = fs::symlink_metadata(&self.path).map_err(snapshot_cache_read_problem)?;
        if !metadata.is_file()
            || metadata.file_type().is_symlink()
            || self.size > MAX_ENCRYPTED_OBJECT_BYTES as u64
            || metadata.len() != self.size
        {
            return Err(snapshot_blob_corrupt());
        }
        let file = fs::File::open(&self.path).map_err(snapshot_cache_read_problem)?;
        let opened = file.metadata().map_err(snapshot_cache_read_problem)?;
        if !opened.is_file() || opened.len() != self.size {
            return Err(snapshot_blob_corrupt());
        }
        let mut bytes = Vec::with_capacity(self.size as usize);
        file.take(self.size.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(snapshot_cache_read_problem)?;
        if bytes.len() as u64 != self.size || checksum(&bytes) != self.checksum {
            return Err(snapshot_blob_corrupt());
        }
        Ok(bytes)
    }
}

fn snapshot_blob_corrupt() -> Problem {
    Problem::new(
        "profile_snapshot_blob_corrupt",
        "Staged profile snapshot object failed its checksum and must be rebuilt",
        503,
        "desktop-projection",
    )
    .retryable(true)
}

fn snapshot_cache_read_problem(error: std::io::Error) -> Problem {
    match error.kind() {
        std::io::ErrorKind::NotFound => snapshot_blob_corrupt(),
        std::io::ErrorKind::PermissionDenied => Problem::new(
            "blob_storage_not_readable",
            "The staged profile snapshot cannot be read; check its permissions",
            500,
            "desktop-projection",
        ),
        _ => unavailable(),
    }
}

#[derive(Clone)]
pub struct ProfileProjection {
    root: PathBuf,
    _root_lock: Arc<fs::File>,
    commit_lock: Arc<std::sync::Mutex<()>>,
}

impl ProfileProjection {
    pub fn snapshot_memory_charge_bytes(&self, profile_path: &Path) -> Result<u64, Problem> {
        validate_profile_source_root(profile_path)?;
        let mut peak = 0;
        visit_profile_file_paths(profile_path, profile_path, &mut |_, _, size| {
            let working_bytes = if size > MAX_IN_MEMORY_SNAPSHOT_FILE_BYTES {
                size.min(PROFILE_FILE_CHUNK_BYTES)
            } else {
                size
            };
            peak = peak.max(working_bytes.saturating_mul(3));
            Ok(())
        })?;
        Ok(peak.max(1))
    }

    pub fn pending_session_paths(&self) -> Result<Vec<(ProfileId, PathBuf)>, Problem> {
        let entries = fs::read_dir(&self.root).map_err(projection_root_io_problem)?;
        let mut pending = Vec::new();
        for entry in entries {
            let entry = entry.map_err(projection_root_io_problem)?;
            let metadata = entry.file_type().map_err(projection_root_io_problem)?;
            if !metadata.is_dir() || metadata.is_symlink() {
                continue;
            }
            let profile_id = match ProfileId::new(entry.file_name().to_string_lossy()) {
                Ok(profile_id) => profile_id,
                Err(_) => continue,
            };
            let profile_root = entry.path();
            let commit_marker = profile_root.join(SYNC_COMMIT_MARKER);
            if let Some(bytes) =
                read_projection_marker(&commit_marker, "profile_sync_commit_marker_invalid")?
            {
                let marker: SyncCommitMarker = serde_json::from_slice(&bytes).map_err(|_| {
                    projection_state_problem(
                        "profile_sync_commit_marker_invalid",
                        "The local sync commit record is corrupt; preserve profile data",
                        422,
                    )
                })?;
                if marker.profile_id != profile_id
                    || marker.base_generation.checked_add(1) != Some(marker.target_generation)
                    || !valid_manifest_checksum(&marker.base_root_checksum)
                    || !valid_manifest_checksum(&marker.target_root_checksum)
                {
                    return Err(projection_state_problem(
                        "profile_sync_commit_marker_invalid",
                        "The local sync commit record is invalid; preserve profile data",
                        422,
                    ));
                }
                pending.push((
                    profile_id.clone(),
                    decrypted_generation_path_parts(
                        &self.root,
                        &profile_id,
                        marker.base_generation,
                        &marker.base_root_checksum,
                    )?,
                ));
                continue;
            }

            let active_marker = profile_root.join(ACTIVE_SESSION_MARKER);
            if let Some(bytes) =
                read_projection_marker(&active_marker, "profile_session_marker_invalid")?
            {
                let marker: ActiveSessionMarker = serde_json::from_slice(&bytes).map_err(|_| {
                    projection_state_problem(
                        "profile_session_marker_invalid",
                        "The local profile session marker is corrupt; preserve profile data",
                        422,
                    )
                })?;
                if marker.state != "active"
                    || marker.profile_id != profile_id
                    || marker.generation == 0
                    || !valid_manifest_checksum(&marker.root_checksum)
                {
                    return Err(projection_state_problem(
                        "profile_session_marker_invalid",
                        "The local profile session marker is invalid; preserve profile data",
                        422,
                    ));
                }
                pending.push((
                    profile_id.clone(),
                    decrypted_generation_path_parts(
                        &self.root,
                        &profile_id,
                        marker.generation,
                        &marker.root_checksum,
                    )?,
                ));
            }
        }
        pending.sort_by(|left, right| left.0.cmp(&right.0));
        pending.dedup_by(|left, right| left.0 == right.0);
        Ok(pending)
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self, Problem> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(projection_staging_io_problem)?;
        let root = root.canonicalize().map_err(projection_staging_io_problem)?;
        let lock_path = confined_path(&root, Path::new(".projection.lock"))?;
        let root_lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(projection_staging_io_problem)?;
        root_lock.try_lock_exclusive().map_err(|error| {
            if is_lock_contention(&error) {
                Problem::new(
                    "profile_projection_in_use",
                    "Profile data is already open by another BugLogin process",
                    409,
                    "desktop-projection",
                )
            } else {
                projection_staging_io_problem(error)
            }
        })?;
        cleanup_abandoned_staging(&root)?;
        Ok(Self {
            root,
            _root_lock: Arc::new(root_lock),
            commit_lock: Arc::new(std::sync::Mutex::new(())),
        })
    }

    pub fn dirty_local_generation(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
    ) -> Result<Option<PathBuf>, Problem> {
        manifest.validate()?;
        descriptor_from_manifest(manifest)?;
        let path = decrypted_generation_path(&self.root, profile_id, manifest)?;
        if !local_generation_directory_exists(&path)? {
            return Ok(None);
        }
        match verify_decrypted_generation(profile_id, manifest, &path) {
            Ok(_) => Ok(None),
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "profile_projection_file_invalid" | "profile_projection_incomplete"
                ) =>
            {
                Ok(Some(path))
            }
            Err(error) => Err(error),
        }
    }

    pub fn existing_local_generation(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
    ) -> Result<Option<PathBuf>, Problem> {
        manifest.validate()?;
        descriptor_from_manifest(manifest)?;
        let path = decrypted_generation_path(&self.root, profile_id, manifest)?;
        Ok(local_generation_directory_exists(&path)?.then_some(path))
    }

    pub fn active_session_recovery_path(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
    ) -> Result<Option<PathBuf>, Problem> {
        manifest.validate()?;
        descriptor_from_manifest(manifest)?;
        let _commit_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let commit_relative = PathBuf::from(profile_id.as_str()).join(SYNC_COMMIT_MARKER);
        let commit_path = confined_path(&self.root, &commit_relative)?;
        let commit_bytes =
            read_projection_marker(&commit_path, "profile_sync_commit_marker_invalid")?;
        if let Some(bytes) = commit_bytes {
            let marker: SyncCommitMarker = serde_json::from_slice(&bytes).map_err(|_| {
                problem(
                    "profile_sync_commit_marker_invalid",
                    "The local sync commit record is invalid; local profile data has been preserved",
                )
            })?;
            if marker.profile_id != *profile_id {
                return Err(problem(
                    "profile_sync_commit_marker_invalid",
                    "The local sync commit record does not match this profile",
                ));
            }
            if marker.base_generation.checked_add(1) != Some(marker.target_generation)
                || !valid_manifest_checksum(&marker.base_root_checksum)
                || !valid_manifest_checksum(&marker.target_root_checksum)
            {
                return Err(problem(
                    "profile_sync_commit_marker_invalid",
                    "The local sync commit record contains invalid generation metadata",
                ));
            }
            if marker.target_generation <= marker.base_generation
                || !valid_manifest_checksum(&marker.base_root_checksum)
                || !valid_manifest_checksum(&marker.target_root_checksum)
            {
                return Err(problem(
                    "profile_sync_commit_marker_invalid",
                    "The local sync commit record contains invalid generation metadata",
                ));
            }
            if manifest.generation == marker.target_generation
                && manifest.root_checksum == marker.target_root_checksum
            {
                let base_path = decrypted_generation_path_parts(
                    &self.root,
                    profile_id,
                    marker.base_generation,
                    &marker.base_root_checksum,
                )?;
                if local_generation_directory_exists(&base_path)?
                    && local_generation_matches_projection_marker(
                        profile_id,
                        marker.base_generation,
                        &marker.base_root_checksum,
                        &base_path,
                    )?
                {
                    self.advance_session_hydration_fence(
                        profile_id,
                        marker.target_generation,
                        &marker.target_root_checksum,
                    )?;
                    self.clear_active_session_locked(profile_id)?;
                    return Ok(None);
                }
                if local_generation_is_recoverable(
                    profile_id,
                    marker.base_generation,
                    &marker.base_root_checksum,
                    &base_path,
                )? {
                    self.advance_session_hydration_fence(
                        profile_id,
                        marker.target_generation,
                        &marker.target_root_checksum,
                    )?;
                    self.clear_active_session_marker_locked(profile_id)?;
                    return Ok(Some(base_path));
                }
                return Err(problem(
                    "profile_local_generation_missing",
                    "The committed profile changed locally, but its preserved base generation is missing or invalid",
                ));
            }
            if manifest.generation != marker.base_generation
                || manifest.root_checksum != marker.base_root_checksum
            {
                let base_path = decrypted_generation_path_parts(
                    &self.root,
                    profile_id,
                    marker.base_generation,
                    &marker.base_root_checksum,
                )?;
                if local_generation_directory_exists(&base_path)?
                    && local_generation_matches_projection_marker(
                        profile_id,
                        marker.base_generation,
                        &marker.base_root_checksum,
                        &base_path,
                    )?
                {
                    self.advance_session_hydration_fence(
                        profile_id,
                        manifest.generation,
                        &manifest.root_checksum,
                    )?;
                    self.clear_active_session_locked(profile_id)?;
                    return Ok(None);
                }
                if local_generation_is_recoverable(
                    profile_id,
                    marker.base_generation,
                    &marker.base_root_checksum,
                    &base_path,
                )? {
                    return Ok(Some(base_path));
                }
                return Err(problem(
                    "profile_local_sync_conflict",
                    "This profile changed on another device while local browser changes were pending. Local data is preserved; resolve the sync conflict before launching",
                ));
            }
            let generation_path = decrypted_generation_path_parts(
                &self.root,
                profile_id,
                marker.base_generation,
                &marker.base_root_checksum,
            )?;
            if !local_generation_directory_exists(&generation_path)? {
                return Err(problem(
                    "profile_local_generation_missing",
                    "A pending local profile sync was recorded, but its profile data is missing",
                ));
            }
            if local_generation_matches_projection_marker(
                profile_id,
                marker.base_generation,
                &marker.base_root_checksum,
                &generation_path,
            )? {
                self.clear_active_session_locked(profile_id)?;
                return Ok(None);
            }
            if local_generation_is_recoverable(
                profile_id,
                marker.base_generation,
                &marker.base_root_checksum,
                &generation_path,
            )? {
                return Ok(Some(generation_path));
            }
            return Err(problem(
                "profile_local_generation_missing",
                "A pending local profile sync has incomplete recovery data; preserve the generation for repair",
            ));
        }
        let relative = PathBuf::from(profile_id.as_str()).join(ACTIVE_SESSION_MARKER);
        let marker_path = confined_path(&self.root, &relative)?;
        let Some(bytes) = read_projection_marker(&marker_path, "profile_session_marker_invalid")?
        else {
            return Ok(None);
        };
        let marker: ActiveSessionMarker = serde_json::from_slice(&bytes).map_err(|_| {
            problem(
                "profile_session_marker_invalid",
                "The local profile session marker is invalid; preserve the profile data and contact support",
            )
        })?;
        if marker.state != "active"
            || marker.profile_id != *profile_id
            || marker.generation == 0
            || !valid_manifest_checksum(&marker.root_checksum)
        {
            return Err(problem(
                "profile_session_marker_invalid",
                "The local profile session marker does not match this profile",
            ));
        }
        if marker.generation != manifest.generation
            || marker.root_checksum != manifest.root_checksum
        {
            let local_path = decrypted_generation_path_parts(
                &self.root,
                profile_id,
                marker.generation,
                &marker.root_checksum,
            )?;
            if local_generation_directory_exists(&local_path)?
                && local_generation_matches_projection_marker(
                    profile_id,
                    marker.generation,
                    &marker.root_checksum,
                    &local_path,
                )?
            {
                self.advance_session_hydration_fence(
                    profile_id,
                    manifest.generation,
                    &manifest.root_checksum,
                )?;
                self.clear_active_session_locked(profile_id)?;
                return Ok(None);
            }
            if local_generation_is_recoverable(
                profile_id,
                marker.generation,
                &marker.root_checksum,
                &local_path,
            )? {
                return Ok(Some(local_path));
            }
            return Err(problem(
                "profile_local_sync_conflict",
                "This profile changed on another device while local browser changes were pending. Local data is preserved; resolve the sync conflict before launching",
            ));
        }
        let generation_path = decrypted_generation_path(&self.root, profile_id, manifest)?;
        if !local_generation_directory_exists(&generation_path)? {
            return Err(problem(
                "profile_local_generation_missing",
                "A pending local profile session was recorded, but its profile data is missing",
            ));
        }
        if local_generation_matches_projection_marker(
            profile_id,
            manifest.generation,
            &manifest.root_checksum,
            &generation_path,
        )? {
            self.advance_session_hydration_fence(
                profile_id,
                manifest.generation,
                &manifest.root_checksum,
            )?;
            self.clear_active_session_locked(profile_id)?;
            Ok(None)
        } else if local_generation_is_recoverable(
            profile_id,
            manifest.generation,
            &manifest.root_checksum,
            &generation_path,
        )? {
            Ok(Some(generation_path))
        } else {
            Err(problem(
                "profile_local_generation_missing",
                "A pending local profile session has incomplete recovery data; preserve the generation for repair",
            ))
        }
    }

    pub fn active_session_conflicts_with_manifest(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
    ) -> Result<bool, Problem> {
        manifest.validate()?;
        descriptor_from_manifest(manifest)?;
        let _commit_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let commit_path = confined_path(
            &self.root,
            &PathBuf::from(profile_id.as_str()).join(SYNC_COMMIT_MARKER),
        )?;
        if let Some(bytes) =
            read_projection_marker(&commit_path, "profile_sync_commit_marker_invalid")?
        {
            let marker: SyncCommitMarker = serde_json::from_slice(&bytes).map_err(|_| {
                problem(
                    "profile_sync_commit_marker_invalid",
                    "The local sync commit record is invalid; local profile data has been preserved",
                )
            })?;
            if marker.profile_id != *profile_id {
                return Err(problem(
                    "profile_sync_commit_marker_invalid",
                    "The local sync commit record does not match this profile",
                ));
            }
            if marker.base_generation.checked_add(1) != Some(marker.target_generation)
                || !valid_manifest_checksum(&marker.base_root_checksum)
                || !valid_manifest_checksum(&marker.target_root_checksum)
            {
                return Err(problem(
                    "profile_sync_commit_marker_invalid",
                    "The local sync commit record contains invalid generation metadata",
                ));
            }
            return Ok((manifest.generation, manifest.root_checksum.as_str())
                != (marker.base_generation, marker.base_root_checksum.as_str())
                && (manifest.generation, manifest.root_checksum.as_str())
                    != (
                        marker.target_generation,
                        marker.target_root_checksum.as_str(),
                    ));
        }
        let marker_path = confined_path(
            &self.root,
            &PathBuf::from(profile_id.as_str()).join(ACTIVE_SESSION_MARKER),
        )?;
        let Some(bytes) = read_projection_marker(&marker_path, "profile_session_marker_invalid")?
        else {
            return Ok(false);
        };
        let marker: ActiveSessionMarker = serde_json::from_slice(&bytes).map_err(|_| {
            problem(
                "profile_session_marker_invalid",
                "The local profile session marker is invalid; preserve the profile data and contact support",
            )
        })?;
        if marker.state != "active"
            || marker.profile_id != *profile_id
            || marker.generation == 0
            || !valid_manifest_checksum(&manifest.root_checksum)
            || !valid_manifest_checksum(&marker.root_checksum)
        {
            return Err(problem(
                "profile_session_marker_invalid",
                "The local profile session marker does not match this profile",
            ));
        }
        Ok((manifest.generation, manifest.root_checksum.as_str())
            != (marker.generation, marker.root_checksum.as_str()))
    }

    /// Reopens a locally retained generation after validating its projection
    /// identity and filesystem shape. A newer cloud manifest is handled as a
    /// sync conflict by stop; it must never be overwritten from this snapshot.
    pub fn verified_local_recovery_receipt(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
    ) -> Result<Option<ProjectionReceipt>, Problem> {
        manifest.validate()?;
        descriptor_from_manifest(manifest)?;
        let _commit_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let commit_path = confined_path(
            &self.root,
            &PathBuf::from(profile_id.as_str()).join(SYNC_COMMIT_MARKER),
        )?;
        let marker_path = confined_path(
            &self.root,
            &PathBuf::from(profile_id.as_str()).join(ACTIVE_SESSION_MARKER),
        )?;
        let mut committed_target = false;
        let commit_bytes =
            read_projection_marker(&commit_path, "profile_sync_commit_marker_invalid")?;
        let generation_path = if let Some(bytes) = commit_bytes {
            let marker: SyncCommitMarker = serde_json::from_slice(&bytes).map_err(|_| {
                problem(
                    "profile_sync_commit_marker_invalid",
                    "The local sync commit record is invalid; local profile data has been preserved",
                )
            })?;
            if marker.profile_id != *profile_id
                || marker.base_generation.checked_add(1) != Some(marker.target_generation)
                || !valid_manifest_checksum(&marker.base_root_checksum)
                || !valid_manifest_checksum(&marker.target_root_checksum)
            {
                return Err(problem(
                    "profile_sync_commit_marker_invalid",
                    "The local sync commit record has invalid identity; local profile data was preserved",
                ));
            }
            committed_target = manifest.generation == marker.target_generation
                && manifest.root_checksum == marker.target_root_checksum;
            decrypted_generation_path_parts(
                &self.root,
                profile_id,
                marker.base_generation,
                &marker.base_root_checksum,
            )?
        } else {
            let Some(bytes) =
                read_projection_marker(&marker_path, "profile_session_marker_invalid")?
            else {
                return Ok(None);
            };
            let marker: ActiveSessionMarker = serde_json::from_slice(&bytes).map_err(|_| {
                problem(
                    "profile_session_marker_invalid",
                    "The local profile session marker is invalid; preserve the profile data and contact support",
                )
            })?;
            if marker.state != "active"
                || marker.profile_id != *profile_id
                || marker.generation == 0
                || !valid_manifest_checksum(&marker.root_checksum)
            {
                return Err(problem(
                    "profile_session_marker_invalid",
                    "The local profile session marker has invalid identity; preserve local profile data",
                ));
            }
            decrypted_generation_path_parts(
                &self.root,
                profile_id,
                marker.generation,
                &marker.root_checksum,
            )?
        };
        let generation_metadata = match fs::symlink_metadata(&generation_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(problem(
                    "profile_local_generation_missing",
                    "A recovery marker references missing local profile data; do not hydrate over it",
                ));
            }
            Err(error) => return Err(projection_marker_io_problem(error)),
        };
        if !generation_metadata.is_dir() || generation_metadata.file_type().is_symlink() {
            return Err(problem(
                "profile_local_generation_missing",
                "A recovery marker references invalid local profile data; preserve it for recovery",
            ));
        }
        let projection_marker_path = generation_path.join(MARKER);
        let marker_bytes = read_projection_marker(
            &projection_marker_path,
            "profile_projection_marker_invalid",
        )?
        .ok_or_else(|| {
            problem(
                "profile_projection_marker_missing",
                "A recovery generation is missing its integrity marker; preserve local profile data",
            )
        })?;
        let projection_marker: EncryptedProjectionMarker = serde_json::from_slice(&marker_bytes)
            .map_err(|_| {
                problem(
                    "profile_projection_marker_invalid",
                    "Profile projection marker is invalid; preserve local profile data",
                )
            })?;
        if projection_marker.state != "complete" || projection_marker.profile_id != *profile_id {
            return Err(problem(
                "profile_projection_marker_invalid",
                "A recovery generation has a mismatched integrity marker; preserve local profile data",
            ));
        }
        let (generation, root_checksum) = match projection_marker.generation {
            generation
                if generation == manifest.generation
                    && projection_marker.remote_root_checksum == manifest.root_checksum =>
            {
                (generation, projection_marker.remote_root_checksum.clone())
            }
            _ => {
                let commit_bytes =
                    read_projection_marker(&commit_path, "profile_sync_commit_marker_invalid")?;
                if let Some(bytes) = commit_bytes {
                    let commit: SyncCommitMarker =
                        serde_json::from_slice(&bytes).map_err(|_| {
                            problem(
                                "profile_sync_commit_marker_invalid",
                                "The local sync commit record is invalid; preserve local profile data",
                            )
                        })?;
                    if commit.profile_id != *profile_id
                        || commit.base_generation.checked_add(1) != Some(commit.target_generation)
                        || !valid_manifest_checksum(&commit.base_root_checksum)
                        || !valid_manifest_checksum(&commit.target_root_checksum)
                        || projection_marker.generation != commit.base_generation
                        || projection_marker.remote_root_checksum != commit.base_root_checksum
                    {
                        return Err(problem(
                            "profile_sync_commit_marker_invalid",
                            "The local sync commit record does not match its preserved generation",
                        ));
                    }
                    (commit.base_generation, commit.base_root_checksum)
                } else {
                    let bytes =
                        read_projection_marker(&marker_path, "profile_session_marker_invalid")?
                            .ok_or_else(|| {
                                problem(
                                    "profile_session_marker_invalid",
                                    "The active session marker disappeared during recovery",
                                )
                            })?;
                    let active: ActiveSessionMarker =
                        serde_json::from_slice(&bytes).map_err(|_| {
                            problem(
                                "profile_session_marker_invalid",
                                "The local profile session marker is invalid; preserve local profile data",
                            )
                        })?;
                    if active.state != "active"
                        || active.profile_id != *profile_id
                        || active.generation == 0
                        || projection_marker.generation != active.generation
                        || projection_marker.remote_root_checksum != active.root_checksum
                    {
                        return Err(problem(
                            "profile_session_marker_invalid",
                            "The local profile session marker does not match its preserved generation",
                        ));
                    }
                    (active.generation, active.root_checksum)
                }
            }
        };
        if !local_generation_is_recoverable(
            profile_id,
            generation,
            &root_checksum,
            &generation_path,
        )? {
            return Err(problem(
                "profile_local_generation_missing",
                "A recovery marker references incomplete local profile data; preserve it for recovery",
            ));
        }
        Ok(Some(ProjectionReceipt {
            profile_id: profile_id.clone(),
            generation: if committed_target {
                manifest.generation
            } else {
                generation
            },
            root_checksum: if committed_target {
                manifest.root_checksum.clone()
            } else {
                root_checksum
            },
            path: generation_path,
        }))
    }

    #[cfg(test)]
    pub fn clear_active_session_marker(&self, profile_id: &ProfileId) -> Result<(), Problem> {
        let _commit_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.clear_active_session_marker_locked(profile_id)
    }

    fn clear_active_session_marker_locked(&self, profile_id: &ProfileId) -> Result<(), Problem> {
        let relative = PathBuf::from(profile_id.as_str()).join(ACTIVE_SESSION_MARKER);
        let marker_path = confined_path(&self.root, &relative)?;
        match fs::remove_file(marker_path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(projection_marker_io_problem(error)),
        }
    }

    fn record_hydration_fence(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
    ) -> Result<(), Problem> {
        let _commit_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let profile_root = confined_path(&self.root, Path::new(profile_id.as_str()))?;
        let marker_path = profile_root.join(HYDRATION_FENCE_MARKER);
        if let Some(bytes) =
            read_projection_marker(&marker_path, "profile_hydration_fence_invalid")?
        {
            let existing: HydrationFenceMarker = serde_json::from_slice(&bytes).map_err(|_| {
                problem(
                    "profile_hydration_fence_invalid",
                    "The local hydration fence is invalid; preserve local profile data",
                )
            })?;
            if existing.profile_id != *profile_id
                || existing.generation == 0
                || !valid_manifest_checksum(&existing.root_checksum)
            {
                return Err(problem(
                    "profile_hydration_fence_invalid",
                    "The local hydration fence has invalid identity; preserve local profile data",
                ));
            }
            if existing.generation > manifest.generation
                || (existing.generation == manifest.generation
                    && existing.root_checksum != manifest.root_checksum)
            {
                return Err(projection_state_problem(
                    "profile_hydration_stale",
                    "A newer profile generation already superseded this hydration",
                    409,
                ));
            }
            if existing.generation == manifest.generation {
                return Ok(());
            }
        }
        write_hydration_fence(
            &self.root,
            profile_id,
            manifest.generation,
            &manifest.root_checksum,
        )
    }

    fn advance_session_hydration_fence(
        &self,
        profile_id: &ProfileId,
        generation: u64,
        root_checksum: &str,
    ) -> Result<(), Problem> {
        let profile_root = confined_path(&self.root, Path::new(profile_id.as_str()))?;
        let marker_path = profile_root.join(HYDRATION_FENCE_MARKER);
        if let Some(bytes) =
            read_projection_marker(&marker_path, "profile_hydration_fence_invalid")?
        {
            let fence: HydrationFenceMarker = serde_json::from_slice(&bytes).map_err(|_| {
                problem(
                    "profile_hydration_fence_invalid",
                    "The local hydration fence is invalid; preserve local profile data",
                )
            })?;
            if fence.profile_id != *profile_id
                || fence.generation == 0
                || !valid_manifest_checksum(&fence.root_checksum)
            {
                return Err(problem(
                    "profile_hydration_fence_invalid",
                    "The local hydration fence has invalid identity; preserve local profile data",
                ));
            }
            if fence.generation > generation
                || (fence.generation == generation && fence.root_checksum != root_checksum)
            {
                return Err(projection_state_problem(
                    "profile_hydration_stale",
                    "A newer profile generation superseded this session start",
                    409,
                ));
            }
        }
        write_hydration_fence(&self.root, profile_id, generation, root_checksum)
    }

    #[cfg(test)]
    pub fn begin_active_session(
        &self,
        profile_id: &ProfileId,
        generation: u64,
        root_checksum: &str,
    ) -> Result<(), Problem> {
        self.begin_active_session_inner(profile_id, generation, root_checksum, None)
    }

    pub fn begin_active_session_for_epoch(
        &self,
        profile_id: &ProfileId,
        generation: u64,
        root_checksum: &str,
        session_epoch: &str,
    ) -> Result<(), Problem> {
        validate_session_epoch(session_epoch)?;
        self.begin_active_session_inner(profile_id, generation, root_checksum, Some(session_epoch))
    }

    fn begin_active_session_inner(
        &self,
        profile_id: &ProfileId,
        generation: u64,
        root_checksum: &str,
        session_epoch: Option<&str>,
    ) -> Result<(), Problem> {
        let _commit_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if generation == 0 || !valid_manifest_checksum(root_checksum) {
            return Err(problem(
                "profile_session_marker_invalid",
                "The active profile session has invalid generation metadata",
            ));
        }
        let profile_root = confined_path(&self.root, Path::new(profile_id.as_str()))?;
        fs::create_dir_all(&profile_root).map_err(projection_staging_io_problem)?;
        let relative = PathBuf::from(profile_id.as_str()).join(ACTIVE_SESSION_MARKER);
        let marker_path = confined_path(&self.root, &relative)?;
        match fs::symlink_metadata(&marker_path) {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() <= 64 * 1024 =>
            {
                let bytes = read_projection_marker(&marker_path, "profile_session_marker_invalid")?
                    .ok_or_else(|| {
                        projection_state_problem(
                            "profile_session_marker_invalid",
                            "The local profile session marker disappeared during validation",
                            422,
                        )
                    })?;
                let existing: ActiveSessionMarker = serde_json::from_slice(&bytes).map_err(|_| {
                    projection_state_problem(
                        "profile_session_marker_invalid",
                        "The local profile session marker is invalid; preserve the profile data and contact support",
                        422,
                    )
                })?;
                if existing.state == "active"
                    && existing.profile_id == *profile_id
                    && existing.generation == generation
                    && existing.root_checksum == root_checksum
                    && existing.session_epoch.as_deref() == session_epoch
                {
                    self.advance_session_hydration_fence(profile_id, generation, root_checksum)?;
                    return Ok(());
                }
                return Err(projection_state_problem(
                    "profile_session_already_active",
                    "A local profile session is already marked active",
                    409,
                ));
            }
            Ok(_) => {
                return Err(projection_state_problem(
                    "profile_session_marker_invalid",
                    "The local profile session marker is not a valid regular file",
                    422,
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(projection_marker_io_problem(error)),
        }
        self.advance_session_hydration_fence(profile_id, generation, root_checksum)?;
        let temporary_relative = PathBuf::from(profile_id.as_str())
            .join(format!("{ACTIVE_SESSION_MARKER}.{}", unique_suffix()));
        let temporary = confined_path(&self.root, &temporary_relative)?;
        let marker = ActiveSessionMarker {
            state: "active".to_owned(),
            profile_id: profile_id.clone(),
            generation,
            root_checksum: root_checksum.to_owned(),
            session_epoch: session_epoch.map(str::to_owned),
        };
        let bytes = serde_json::to_vec(&marker).map_err(|_| unavailable())?;
        let write_result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(projection_staging_io_problem)?;
            file.write_all(&bytes)
                .map_err(projection_staging_io_problem)?;
            file.sync_all().map_err(projection_staging_io_problem)?;
            fs::rename(&temporary, &marker_path).map_err(projection_staging_io_problem)
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn clear_active_session(&self, profile_id: &ProfileId) -> Result<(), Problem> {
        let _commit_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.advance_committed_generation_fence_locked(profile_id, None)?;
        self.clear_active_session_locked(profile_id)
    }

    pub fn clear_active_session_for_epoch(
        &self,
        profile_id: &ProfileId,
        session_epoch: &str,
    ) -> Result<(), Problem> {
        validate_session_epoch(session_epoch)?;
        let _commit_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let marker_path = confined_path(
            &self.root,
            &PathBuf::from(profile_id.as_str()).join(ACTIVE_SESSION_MARKER),
        )?;
        let bytes = read_projection_marker(&marker_path, "profile_session_marker_invalid")?
            .ok_or_else(|| {
                projection_state_problem(
                    "profile_session_stale",
                    "The profile session ended before this stop could be applied",
                    409,
                )
            })?;
        let marker: ActiveSessionMarker = serde_json::from_slice(&bytes).map_err(|_| {
            projection_state_problem(
                "profile_session_marker_invalid",
                "The local profile session marker is invalid; preserve local profile data",
                422,
            )
        })?;
        if marker.state != "active"
            || marker.profile_id != *profile_id
            || marker.generation == 0
            || !valid_manifest_checksum(&marker.root_checksum)
        {
            return Err(projection_state_problem(
                "profile_session_marker_invalid",
                "The local profile session marker has invalid identity; preserve local profile data",
                422,
            ));
        }
        if marker.session_epoch.as_deref() != Some(session_epoch) {
            return Err(projection_state_problem(
                "profile_session_stale",
                "A newer profile session superseded this stop request",
                409,
            ));
        }
        self.advance_committed_generation_fence_locked(
            profile_id,
            Some((
                session_epoch,
                marker.generation,
                marker.root_checksum.as_str(),
            )),
        )?;
        self.clear_active_session_locked(profile_id)
    }

    fn advance_committed_generation_fence_locked(
        &self,
        profile_id: &ProfileId,
        active_session: Option<(&str, u64, &str)>,
    ) -> Result<(), Problem> {
        let marker_path = confined_path(
            &self.root,
            &PathBuf::from(profile_id.as_str()).join(SYNC_COMMIT_MARKER),
        )?;
        let Some(bytes) =
            read_projection_marker(&marker_path, "profile_sync_commit_marker_invalid")?
        else {
            return Ok(());
        };
        let marker: SyncCommitMarker = serde_json::from_slice(&bytes).map_err(|_| {
            problem(
                "profile_sync_commit_marker_invalid",
                "The local sync commit record is invalid; local profile data was preserved",
            )
        })?;
        if marker.profile_id != *profile_id
            || marker.base_generation.checked_add(1) != Some(marker.target_generation)
            || !valid_manifest_checksum(&marker.base_root_checksum)
            || !valid_manifest_checksum(&marker.target_root_checksum)
        {
            return Err(problem(
                "profile_sync_commit_marker_invalid",
                "The local sync commit record has invalid identity; local profile data was preserved",
            ));
        }
        if marker.session_epoch.as_deref() != active_session.map(|(epoch, _, _)| epoch) {
            return Err(projection_state_problem(
                "profile_sync_commit_stale",
                "A different profile session owns the pending sync commit; preserve local profile data",
                409,
            ));
        }
        if active_session.is_some_and(|(_, generation, root_checksum)| {
            (marker.base_generation, marker.base_root_checksum.as_str())
                != (generation, root_checksum)
        }) {
            return Err(projection_state_problem(
                "profile_sync_commit_stale",
                "The pending sync commit does not match this profile session base; preserve local profile data",
                409,
            ));
        }
        self.advance_session_hydration_fence(
            profile_id,
            marker.target_generation,
            &marker.target_root_checksum,
        )
    }

    fn clear_active_session_locked(&self, profile_id: &ProfileId) -> Result<(), Problem> {
        for name in [ACTIVE_SESSION_MARKER, SYNC_COMMIT_MARKER] {
            let relative = PathBuf::from(profile_id.as_str()).join(name);
            let marker_path = confined_path(&self.root, &relative)?;
            match fs::remove_file(marker_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(projection_marker_io_problem(error)),
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn prepare_sync_commit(
        &self,
        profile_id: &ProfileId,
        base_manifest: &SyncManifest,
        target_manifest: &SyncManifest,
    ) -> Result<(), Problem> {
        self.prepare_sync_commit_inner(profile_id, base_manifest, target_manifest, None)
    }

    pub fn prepare_sync_commit_for_epoch(
        &self,
        profile_id: &ProfileId,
        base_manifest: &SyncManifest,
        target_manifest: &SyncManifest,
        session_epoch: &str,
    ) -> Result<(), Problem> {
        validate_session_epoch(session_epoch)?;
        self.prepare_sync_commit_inner(
            profile_id,
            base_manifest,
            target_manifest,
            Some(session_epoch),
        )
    }

    fn prepare_sync_commit_inner(
        &self,
        profile_id: &ProfileId,
        base_manifest: &SyncManifest,
        target_manifest: &SyncManifest,
        session_epoch: Option<&str>,
    ) -> Result<(), Problem> {
        let _commit_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        base_manifest.validate()?;
        target_manifest.validate()?;
        descriptor_from_manifest(base_manifest)?;
        descriptor_from_manifest(target_manifest)?;
        let next_generation = base_manifest.generation.checked_add(1).ok_or_else(|| {
            Problem::new(
                "sync_generation_exhausted",
                "Profile sync generation cannot be incremented",
                409,
                "desktop-projection",
            )
        })?;
        if base_manifest.workspace_id != target_manifest.workspace_id
            || base_manifest.resource_id != target_manifest.resource_id
            || target_manifest.generation != next_generation
            || target_manifest.parent_generation != Some(base_manifest.generation)
            || !valid_manifest_checksum(&base_manifest.root_checksum)
            || !valid_manifest_checksum(&target_manifest.root_checksum)
        {
            return Err(problem(
                "profile_sync_commit_marker_invalid",
                "The profile sync commit does not follow the active manifest generation",
            ));
        }
        let profile_root = confined_path(&self.root, Path::new(profile_id.as_str()))?;
        fs::create_dir_all(&profile_root).map_err(projection_staging_io_problem)?;
        let marker = SyncCommitMarker {
            profile_id: profile_id.clone(),
            base_generation: base_manifest.generation,
            base_root_checksum: base_manifest.root_checksum.clone(),
            target_generation: target_manifest.generation,
            target_root_checksum: target_manifest.root_checksum.clone(),
            session_epoch: session_epoch.map(str::to_owned),
        };
        let bytes = serde_json::to_vec(&marker).map_err(|_| unavailable())?;
        let relative = PathBuf::from(profile_id.as_str()).join(SYNC_COMMIT_MARKER);
        let marker_path = confined_path(&self.root, &relative)?;
        let active_marker_path = confined_path(
            &self.root,
            &PathBuf::from(profile_id.as_str()).join(ACTIVE_SESSION_MARKER),
        )?;
        let active_session_matches_base = match fs::symlink_metadata(&active_marker_path) {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() <= 64 * 1024 =>
            {
                let active_bytes =
                    read_projection_marker(&active_marker_path, "profile_session_marker_invalid")?
                        .ok_or_else(|| {
                            projection_state_problem(
                                "profile_session_marker_invalid",
                                "The local profile session marker disappeared during validation",
                                422,
                            )
                        })?;
                let active: ActiveSessionMarker = serde_json::from_slice(&active_bytes)
                    .map_err(|_| projection_state_problem(
                        "profile_session_marker_invalid",
                        "The local profile session marker is invalid; preserve local profile data",
                        422,
                    ))?;
                if active.state != "active"
                    || active.profile_id != *profile_id
                    || active.generation == 0
                    || !valid_manifest_checksum(&active.root_checksum)
                {
                    return Err(projection_state_problem(
                        "profile_session_marker_invalid",
                        "The local profile session marker does not match this profile",
                        422,
                    ));
                }
                if (active.generation, active.root_checksum.as_str())
                    != (
                        base_manifest.generation,
                        base_manifest.root_checksum.as_str(),
                    )
                {
                    return Err(projection_state_problem(
                        "profile_sync_commit_stale",
                        "The profile session advanced while this sync was uploading; local data was preserved",
                        409,
                    ));
                }
                if active.session_epoch.as_deref() != session_epoch {
                    return Err(projection_state_problem(
                        "profile_sync_commit_stale",
                        "A different profile session superseded this sync upload; local data was preserved",
                        409,
                    ));
                }
                true
            }
            Ok(_) => {
                return Err(projection_state_problem(
                    "profile_session_marker_invalid",
                    "The local profile session marker is not a valid regular file",
                    422,
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let completed =
                    read_projection_marker(&marker_path, "profile_sync_commit_marker_invalid")?;
                if let Some(bytes) = completed {
                    let existing: SyncCommitMarker = serde_json::from_slice(&bytes).map_err(|_| {
                        problem(
                            "profile_sync_commit_marker_invalid",
                            "The local sync commit record is invalid; local profile data was preserved",
                        )
                    })?;
                    if existing.profile_id == *profile_id
                        && existing.base_generation == base_manifest.generation
                        && existing.base_root_checksum == base_manifest.root_checksum
                        && existing.target_generation == target_manifest.generation
                        && existing.target_root_checksum == target_manifest.root_checksum
                        && existing.session_epoch.as_deref() == session_epoch
                    {
                        return Ok(());
                    }
                }
                false
            }
            Err(error) => return Err(projection_marker_io_problem(error)),
        };
        match fs::symlink_metadata(&marker_path) {
            Ok(metadata)
                if metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() <= 64 * 1024 =>
            {
                let existing_bytes =
                    read_projection_marker(&marker_path, "profile_sync_commit_marker_invalid")?
                        .ok_or_else(|| {
                            projection_state_problem(
                                "profile_sync_commit_marker_invalid",
                                "The local sync commit record disappeared during validation",
                                422,
                            )
                        })?;
                let existing: SyncCommitMarker = serde_json::from_slice(&existing_bytes)
                    .map_err(|_| problem(
                        "profile_sync_commit_marker_invalid",
                        "The local sync commit record is invalid; local profile data has been preserved",
                    ))?;
                if existing.profile_id != *profile_id
                    || existing.base_generation.checked_add(1) != Some(existing.target_generation)
                    || !valid_manifest_checksum(&existing.base_root_checksum)
                    || !valid_manifest_checksum(&existing.target_root_checksum)
                {
                    return Err(problem(
                        "profile_sync_commit_marker_invalid",
                        "The local sync commit record has invalid profile or generation metadata",
                    ));
                }
                if existing.target_generation == target_manifest.generation {
                    if existing.base_generation == base_manifest.generation
                        && existing.base_root_checksum == base_manifest.root_checksum
                        && existing.target_root_checksum == target_manifest.root_checksum
                        && existing.session_epoch.as_deref() == session_epoch
                    {
                        return Ok(());
                    }
                    return Err(projection_state_problem(
                        "profile_sync_commit_stale",
                        "A different sync result already targets this generation; local data was preserved",
                        409,
                    ));
                }
                if existing.target_generation > base_manifest.generation {
                    return Err(projection_state_problem(
                        "profile_sync_commit_stale",
                        "A newer local sync commit is already pending; the older result was not applied",
                        409,
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(problem(
                    "profile_sync_commit_marker_invalid",
                    "The local sync commit record is not a valid regular file",
                ));
            }
            Err(error) => return Err(projection_marker_io_problem(error)),
        }
        if !active_session_matches_base {
            return Err(projection_state_problem(
                "profile_sync_commit_stale",
                "The profile session ended while this sync was uploading; local data was preserved",
                409,
            ));
        }
        let temporary_relative = PathBuf::from(profile_id.as_str())
            .join(format!("{SYNC_COMMIT_MARKER}.{}", unique_suffix()));
        let temporary = confined_path(&self.root, &temporary_relative)?;
        let write_result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(projection_staging_io_problem)?;
            file.write_all(&bytes)
                .map_err(projection_staging_io_problem)?;
            file.sync_all().map_err(projection_staging_io_problem)?;
            // `rename` replaces the old marker atomically on the same filesystem.
            // Removing it first creates a crash window where neither commit record exists.
            fs::rename(&temporary, &marker_path).map_err(projection_staging_io_problem)
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }

    pub async fn hydrate_encrypted_with_proxy_credentials(
        &self,
        client: &RemoteApiClient,
        auth: &CoordinationAuth,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
        client_key: Option<&ClientKey>,
        recovery_key: Option<&RecoveryKey>,
        correlation_id: &str,
    ) -> Result<(ProjectionReceipt, Vec<HydratedProxyCredential>), Problem> {
        self.hydrate_encrypted_with_proxy_credentials_mode(
            client,
            auth,
            profile_id,
            manifest,
            client_key,
            recovery_key,
            correlation_id,
            false,
        )
        .await
    }

    pub async fn hydrate_encrypted_with_proxy_credentials_for_repair(
        &self,
        client: &RemoteApiClient,
        auth: &CoordinationAuth,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
        client_key: Option<&ClientKey>,
        recovery_key: Option<&RecoveryKey>,
        correlation_id: &str,
    ) -> Result<(ProjectionReceipt, Vec<HydratedProxyCredential>), Problem> {
        self.hydrate_encrypted_with_proxy_credentials_mode(
            client,
            auth,
            profile_id,
            manifest,
            client_key,
            recovery_key,
            correlation_id,
            true,
        )
        .await
    }

    async fn hydrate_encrypted_with_proxy_credentials_mode(
        &self,
        client: &RemoteApiClient,
        auth: &CoordinationAuth,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
        client_key: Option<&ClientKey>,
        recovery_key: Option<&RecoveryKey>,
        correlation_id: &str,
        repair_stale_proxy: bool,
    ) -> Result<(ProjectionReceipt, Vec<HydratedProxyCredential>), Problem> {
        let mut download_metrics = ProfileDownloadMetrics::default();
        manifest.validate()?;
        ensure_hydration_generation_current(
            &self.root.join(profile_id.as_str()),
            profile_id,
            manifest,
        )?;
        if manifest.objects.is_empty() {
            let descriptor = descriptor_from_manifest(manifest)?;
            self.record_hydration_fence(profile_id, manifest)?;
            let _commit_guard = self
                .commit_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ensure_hydration_generation_current(
                &self.root.join(profile_id.as_str()),
                profile_id,
                manifest,
            )?;
            return self
                .publish(profile_id, manifest, &descriptor, Vec::new())
                .map(|receipt| (receipt, Vec::new()));
        }
        let encryption = manifest.payload_encryption.as_ref().ok_or_else(|| {
            problem(
                "profile_payload_encryption_required",
                "This profile has no supported client-side encryption metadata",
            )
        })?;
        let remote_descriptor = descriptor_from_manifest(manifest)?;
        self.record_hydration_fence(profile_id, manifest)?;
        let generation_path = decrypted_generation_path(&self.root, profile_id, manifest)?;
        let generation_exists = local_generation_directory_exists(&generation_path)?;
        let cached_generation = if generation_exists {
            let profile_id = profile_id.clone();
            let manifest = manifest.clone();
            let generation_path = generation_path.clone();
            let verification = tokio::task::spawn_blocking(move || {
                verify_decrypted_generation(&profile_id, &manifest, &generation_path)
            })
            .await
            .map_err(|_| unavailable())?;
            match verification {
                Ok(_) => true,
                Err(error)
                    if matches!(
                        error.code.as_str(),
                        "profile_projection_file_invalid" | "profile_projection_incomplete"
                    ) =>
                {
                    false
                }
                Err(error) => return Err(error),
            }
        } else {
            false
        };
        if generation_exists && !cached_generation {
            return Err(projection_state_problem(
                "profile_local_changes_recovery_required",
                "This device has local profile data that differs from the last synced generation. Preserve the local profile data and retry synchronization before launching it again.",
                409,
            ));
        }
        let clear_metadata = proxy_metadata_keys(manifest);
        let proxy_envelopes = proxy_credential_keys(manifest);
        let envelope_object = remote_descriptor
            .objects
            .iter()
            .find(|object| object.path == RESOURCE_KEY_ENVELOPE_KEY)
            .ok_or_else(|| {
                problem(
                    "resource_key_envelope_missing",
                    "Resource key envelope is missing",
                )
            })?;
        let cached_envelope = if !repair_stale_proxy {
            if let Some(client_key) = client_key {
                let cache_scope = checksum(
                    format!(
                        "{}/{}/{}",
                        manifest.workspace_id.as_str(),
                        manifest.resource_id.as_str(),
                        checksum(client_key.as_bytes())
                    )
                    .as_bytes(),
                );
                read_cached_manifest_object(
                    &self.root,
                    &cache_scope,
                    &encryption.resource_key_envelope,
                    Some("resource-key-envelope.json"),
                    correlation_id,
                )
                .await?
            } else {
                None
            }
        } else {
            None
        };
        let (envelope_bytes, _envelope_memory_permit) = match cached_envelope {
            Some(cached) => cached,
            None => {
                let (bytes, permit) = download_verified_manifest_object(
                    client,
                    &manifest.workspace_id,
                    auth,
                    &manifest.resource_id,
                    manifest.generation,
                    &envelope_object.path,
                    envelope_object.size,
                    &envelope_object.checksum,
                    correlation_id,
                    repair_stale_proxy,
                )
                .await?;
                download_metrics.record(bytes.len());
                (bytes, permit)
            }
        };
        let envelope: ResourceKeyEnvelope =
            serde_json::from_slice(&envelope_bytes).map_err(|_| {
                problem(
                    "resource_key_envelope_invalid",
                    "Resource key envelope is invalid",
                )
            })?;
        let unwrapped_key = if client_key.is_none() {
            let recovery_key = recovery_key.ok_or_else(|| {
                problem(
                    "sync_key_recovery_required",
                    "This profile needs an authorized managed key delivery",
                )
            })?;
            Some(envelope.unwrap(&manifest.workspace_id, &manifest.resource_id, recovery_key)?)
        } else {
            None
        };
        let key = client_key
            .or(unwrapped_key.as_ref())
            .ok_or_else(|| problem("profile_key_unavailable", "Profile key is unavailable"))?;
        if unwrapped_key
            .as_ref()
            .is_some_and(|unwrapped| client_key != Some(unwrapped))
        {
            return Err(problem(
                "workspace_key_vault_mismatch",
                "Stored profile key does not match the active manifest",
            ));
        }
        if let PayloadEncryptionAlgorithm::Aes256GcmChunkV2 = encryption.algorithm {
            let expected_key_version = checksum(key.as_bytes());
            if encryption.key_version.as_deref() != Some(expected_key_version.as_str()) {
                return Err(problem(
                    "profile_payload_key_version_mismatch",
                    "Profile payload key version does not match the recovery key",
                ));
            }
        }
        let cache_scope = checksum(
            format!(
                "{}/{}/{}",
                manifest.workspace_id.as_str(),
                manifest.resource_id.as_str(),
                checksum(key.as_bytes())
            )
            .as_bytes(),
        );
        for object in remote_descriptor
            .objects
            .iter()
            .filter(|object| clear_metadata.contains(&object.path))
        {
            let reference = ManifestObjectReference {
                key: object.path.clone(),
                size: object.size,
                checksum: object.checksum.clone(),
            };
            let cached = if repair_stale_proxy {
                None
            } else {
                read_cached_manifest_object(
                    &self.root,
                    &cache_scope,
                    &reference,
                    None,
                    correlation_id,
                )
                .await?
            };
            if cached.is_some() {
                continue;
            }
            let (bytes, permit) = download_verified_manifest_object(
                client,
                &manifest.workspace_id,
                auth,
                &manifest.resource_id,
                manifest.generation,
                &object.path,
                object.size,
                &object.checksum,
                correlation_id,
                repair_stale_proxy,
            )
            .await?;
            download_metrics.record(bytes.len());
            if !repair_stale_proxy {
                if let Err(error) = cache_downloaded_manifest_object(
                    self.root.clone(),
                    cache_scope.clone(),
                    object.path.clone(),
                    bytes,
                    permit,
                )
                .await
                {
                    tauri_plugin_log::log::warn!(
                        "Legacy proxy metadata cache write failed: code={} status={}",
                        error.code,
                        error.status
                    );
                }
            }
        }
        let mut credentials = Vec::with_capacity(proxy_envelopes.len());
        for (proxy_id, revision, reference) in &proxy_envelopes {
            let proxy_cache_alias = format!(
                "proxy-{}-{}-envelope.json",
                checksum(proxy_id.as_str().as_bytes()),
                revision
            );
            let cached = if repair_stale_proxy {
                None
            } else {
                read_cached_manifest_object(
                    &self.root,
                    &cache_scope,
                    reference,
                    Some(&proxy_cache_alias),
                    correlation_id,
                )
                .await?
            };
            let (bytes, _transfer_memory_permit) = match cached {
                Some(cached) => cached,
                None => {
                    let (bytes, permit) = download_verified_manifest_object(
                        client,
                        &manifest.workspace_id,
                        auth,
                        &manifest.resource_id,
                        manifest.generation,
                        &reference.key,
                        reference.size,
                        &reference.checksum,
                        correlation_id,
                        repair_stale_proxy,
                    )
                    .await?;
                    download_metrics.record(bytes.len());
                    (bytes, permit)
                }
            };
            let envelope: ProxyCredentialEnvelope =
                serde_json::from_slice(&bytes).map_err(|_| {
                    problem(
                        "proxy_credential_envelope_invalid",
                        "Proxy credential envelope is invalid",
                    )
                })?;
            let secret = envelope.open(
                &manifest.workspace_id,
                &manifest.resource_id,
                proxy_id,
                *revision,
                key,
            )?;
            let secret = SecretValue::new(secret).map_err(|_| {
                problem(
                    "proxy_credential_envelope_invalid",
                    "Proxy credential envelope is invalid",
                )
            })?;
            credentials.push(HydratedProxyCredential {
                proxy_id: proxy_id.clone(),
                revision: *revision,
                secret,
            });
        }
        if cached_generation {
            ensure_hydration_generation_current(
                &self.root.join(profile_id.as_str()),
                &profile_id,
                &manifest,
            )?;
            let verify_profile_id = profile_id.clone();
            let verify_manifest = manifest.clone();
            let generation_path = generation_path.clone();
            let receipt = tokio::task::spawn_blocking(move || {
                verify_decrypted_generation(&verify_profile_id, &verify_manifest, &generation_path)
            })
            .await
            .map_err(|_| unavailable())?;
            let _commit_guard = self
                .commit_lock
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            ensure_hydration_generation_current(
                &self.root.join(profile_id.as_str()),
                &profile_id,
                &manifest,
            )?;
            let verified = receipt.map(|receipt| (receipt, credentials));
            if verified.is_ok() {
                download_metrics.outcome = "cached";
            }
            return verified;
        }

        let mut staging = DecryptedGenerationStaging::new(&self.root, profile_id, manifest)?;
        let chunk_object_keys = manifest
            .file_chunks
            .iter()
            .flat_map(|file| file.chunks.iter().map(|chunk| chunk.object.key.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        let download_slots = profile_download_slots();
        let download_client = client.clone();
        let download_auth = auth.clone();
        let workspace_id = manifest.workspace_id.clone();
        let resource_id = manifest.resource_id.clone();
        let generation = manifest.generation;
        let correlation_id = correlation_id.to_owned();
        let downloadable_objects = remote_descriptor
            .objects
            .iter()
            .filter(|object| {
                !should_download_manifest_object(&object.path, &clear_metadata, &proxy_envelopes)
                    && !chunk_object_keys.contains(object.path.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut downloaded_objects = stream::iter(downloadable_objects.into_iter().map(|object| {
            let download_slots = Arc::clone(&download_slots);
            let client = download_client.clone();
            let auth = download_auth.clone();
            let workspace_id = workspace_id.clone();
            let resource_id = resource_id.clone();
            let correlation_id = correlation_id.clone();
            async move {
                let memory_permit =
                    profile_transfer_budget::acquire(object.size, 2, &correlation_id).await?;
                let slot = download_slots
                    .acquire_owned()
                    .await
                    .map_err(|_| unavailable())?;
                let bytes = client
                    .download_blob(
                        CoordinationScope {
                            workspace_id: &workspace_id,
                            auth: &auth,
                        },
                        &resource_id,
                        &object.path,
                        generation,
                        &object.checksum,
                        object.size,
                        &correlation_id,
                    )
                    .await?;
                Ok::<_, Problem>((object, bytes, slot, memory_permit))
            }
        }))
        .buffered(MAX_CONCURRENT_PROFILE_DOWNLOADS);
        while let Some((object, bytes, _download_slot, _memory_permit)) =
            downloaded_objects.try_next().await?
        {
            if bytes.len() as u64 != object.size || checksum(&bytes) != object.checksum {
                return Err(problem(
                    "profile_object_checksum_mismatch",
                    "Profile object failed its ciphertext checksum",
                ));
            }
            download_metrics.record(bytes.len());
            let chunk = AuthenticatedChunk::decode(&bytes)?;
            let plaintext = match encryption.algorithm {
                PayloadEncryptionAlgorithm::Aes256GcmChunkV1 => chunk.decrypt(
                    key,
                    &profile_object_aad(
                        &manifest.workspace_id,
                        &manifest.resource_id,
                        manifest.generation,
                        &object.path,
                    ),
                )?,
                PayloadEncryptionAlgorithm::Aes256GcmChunkV2 => chunk.decrypt(
                    key,
                    &profile_object_aad_v2(
                        &manifest.workspace_id,
                        &manifest.resource_id,
                        &object.path,
                    ),
                )?,
            };
            staging.write_object(&object.path, &plaintext)?;
        }

        for file in &manifest.file_chunks {
            let (mut destination, _) = staging.create_file(&file.path)?;
            let mut aggregate = Sha256::new();
            let mut total_size = 0u64;
            let download_slots = profile_download_slots();
            let chunks = file
                .chunks
                .iter()
                .enumerate()
                .map(|(index, chunk)| {
                    (
                        index,
                        chunk.object.key.clone(),
                        chunk.object.checksum.clone(),
                        chunk.object.size,
                    )
                })
                .collect::<Vec<_>>();
            let download_client = client.clone();
            let download_auth = auth.clone();
            let workspace_id = manifest.workspace_id.clone();
            let resource_id = manifest.resource_id.clone();
            let generation = manifest.generation;
            let correlation_id = correlation_id.to_owned();
            let mut downloaded_chunks = stream::iter(chunks.into_iter().map(
                |(index, object_key, object_checksum, object_size)| {
                    let download_slots = Arc::clone(&download_slots);
                    let client = download_client.clone();
                    let auth = download_auth.clone();
                    let workspace_id = workspace_id.clone();
                    let resource_id = resource_id.clone();
                    let correlation_id = correlation_id.clone();
                    async move {
                        let memory_permit =
                            profile_transfer_budget::acquire(object_size, 2, &correlation_id)
                                .await?;
                        let permit = download_slots
                            .acquire_owned()
                            .await
                            .map_err(|_| unavailable())?;
                        let encrypted = client
                            .download_blob(
                                CoordinationScope {
                                    workspace_id: &workspace_id,
                                    auth: &auth,
                                },
                                &resource_id,
                                &object_key,
                                generation,
                                &object_checksum,
                                object_size,
                                &correlation_id,
                            )
                            .await?;
                        // Keep count and byte admission until the ordered
                        // consumer authenticates and writes this chunk. The
                        // byte charge accounts for ciphertext/plaintext overlap.
                        Ok::<_, Problem>((index, encrypted, permit, memory_permit))
                    }
                },
            ))
            .buffered(MAX_CONCURRENT_PROFILE_DOWNLOADS);

            for (expected_index, chunk) in file.chunks.iter().enumerate() {
                if chunk.index != expected_index as u64 {
                    return Err(problem(
                        "profile_file_chunks_invalid",
                        "Chunked profile file sequence is invalid",
                    ));
                }
                let (downloaded_index, encrypted, _download_permit, _download_memory_permit) =
                    downloaded_chunks.try_next().await?.ok_or_else(|| {
                        problem(
                            "profile_file_chunks_invalid",
                            "Chunked profile file sequence is incomplete",
                        )
                    })?;
                if downloaded_index != expected_index {
                    return Err(problem(
                        "profile_file_chunks_invalid",
                        "Chunked profile file sequence is out of order",
                    ));
                }
                download_metrics.record(encrypted.len());
                if encrypted.len() as u64 != chunk.object.size
                    || checksum(&encrypted) != chunk.object.checksum
                {
                    return Err(problem(
                        "profile_file_chunk_checksum_mismatch",
                        "A profile file chunk failed its ciphertext checksum",
                    ));
                }
                let chunk_aad = profile_file_chunk_aad(
                    &manifest.workspace_id,
                    &manifest.resource_id,
                    &file.path,
                    chunk.index,
                    &chunk.plaintext_checksum,
                );
                let plaintext_chunk =
                    AuthenticatedChunk::decode(&encrypted)?.decrypt(key, &chunk_aad)?;
                if plaintext_chunk.len() as u64 != chunk.plaintext_size
                    || checksum(&plaintext_chunk) != chunk.plaintext_checksum
                {
                    return Err(problem(
                        "profile_file_chunk_checksum_mismatch",
                        "A profile file chunk failed authentication",
                    ));
                }
                aggregate.update(&plaintext_chunk);
                destination
                    .write_all(&plaintext_chunk)
                    .map_err(projection_staging_io_problem)?;
                total_size = total_size.saturating_add(plaintext_chunk.len() as u64);
            }
            let aggregate_checksum = format!("{:x}", aggregate.finalize());
            if total_size != file.plaintext_size || aggregate_checksum != file.plaintext_checksum {
                return Err(problem(
                    "profile_file_checksum_mismatch",
                    "A reconstructed profile file failed its checksum",
                ));
            }
            staging.finish_file(&file.path, destination, total_size, aggregate_checksum)?;
        }
        let _commit_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        ensure_hydration_generation_current(
            &self.root.join(profile_id.as_str()),
            profile_id,
            manifest,
        )?;
        let published = staging
            .publish(profile_id, manifest)
            .map(|receipt| (receipt, credentials));
        if published.is_ok() {
            download_metrics.outcome = "hydrated";
        }
        published
    }

    #[cfg(test)]
    fn publish_decrypted(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
        objects: Vec<(String, Vec<u8>)>,
    ) -> Result<ProjectionReceipt, Problem> {
        let mut staging = DecryptedGenerationStaging::new(&self.root, profile_id, manifest)?;
        let object_count = objects.len();
        let objects_by_key: BTreeMap<_, _> = objects.into_iter().collect();
        if objects_by_key.len() != object_count {
            return Err(problem(
                "profile_projection_incomplete",
                "Profile projection is incomplete",
            ));
        }
        for (path, bytes) in objects_by_key {
            staging.write_object(&path, &bytes)?;
        }
        staging.publish(profile_id, manifest)
    }

    fn publish(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
        descriptor: &PayloadDescriptor,
        objects: Vec<(String, Vec<u8>)>,
    ) -> Result<ProjectionReceipt, Problem> {
        descriptor.verify_manifest(manifest)?;
        let profile_root = confined_path(&self.root, Path::new(profile_id.as_str()))?;
        fs::create_dir_all(&profile_root).map_err(projection_staging_io_problem)?;
        let generation_name = format!(
            "generations/{}-{}",
            descriptor.generation, descriptor.root_checksum
        );
        let generation_path = confined_path(&profile_root, Path::new(&generation_name))?;
        if local_generation_directory_exists(&generation_path)? {
            return verify_generation(profile_id, descriptor, &generation_path);
        }
        let staging_name = format!(
            "staging/{}-{}-{}",
            descriptor.generation,
            std::process::id(),
            unique_suffix()
        );
        let staging_path = confined_path(&profile_root, Path::new(&staging_name))?;
        fs::create_dir_all(&staging_path).map_err(projection_staging_io_problem)?;
        let supplied: BTreeMap<_, _> = objects.into_iter().collect();
        if supplied.len() != descriptor.objects.len() {
            return Err(problem(
                "profile_projection_incomplete",
                "Profile projection is incomplete",
            ));
        }
        for object in &descriptor.objects {
            let bytes = supplied.get(&object.path).ok_or_else(|| {
                problem(
                    "profile_projection_incomplete",
                    "Profile projection is incomplete",
                )
            })?;
            object.verify(bytes)?;
            write_projection_verified(
                &staging_path,
                Path::new(&object.path),
                bytes,
                &object.checksum,
            )?;
        }
        let marker = ProjectionMarker {
            state: "complete".to_owned(),
            profile_id: profile_id.clone(),
            generation: descriptor.generation,
            root_checksum: descriptor.root_checksum.clone(),
        };
        let marker_bytes = serde_json::to_vec(&marker).map_err(|_| unavailable())?;
        write_projection_verified(
            &staging_path,
            Path::new(MARKER),
            &marker_bytes,
            &checksum(&marker_bytes),
        )?;
        if let Some(parent) = generation_path.parent() {
            fs::create_dir_all(parent).map_err(projection_staging_io_problem)?;
        }
        fs::rename(&staging_path, &generation_path).map_err(projection_staging_io_problem)?;
        verify_generation(profile_id, descriptor, &generation_path)
    }

    #[cfg(test)]
    fn snapshot_with_proxy(
        &self,
        profile_path: &Path,
        previous: &SyncManifest,
        fencing_token: u64,
        proxy: Option<RemoteProxySyncSnapshot>,
    ) -> Result<(PayloadDescriptor, Vec<(String, Vec<u8>)>, SyncManifest), Problem> {
        let mut files = Vec::new();
        collect_files(profile_path, profile_path, &mut files)?;
        let proxy_dependency =
            proxy.map(|proxy| (proxy.proxy_id, proxy.revision, proxy.credential));
        let objects = files
            .iter()
            .map(|(path, bytes)| PayloadObject::new(path.clone(), bytes))
            .collect::<Result<Vec<_>, _>>()?;
        let next_generation = previous.generation.checked_add(1).ok_or_else(|| {
            Problem::new(
                "sync_generation_exhausted",
                "Profile sync generation cannot be incremented",
                409,
                "desktop-projection",
            )
        })?;
        let descriptor = PayloadDescriptor::new(
            previous.workspace_id.clone(),
            previous.resource_id.clone(),
            next_generation,
            objects,
        )?;
        let live: BTreeMap<_, _> = descriptor
            .objects
            .iter()
            .map(|object| (object.path.as_str(), object))
            .collect();
        let now = unix_time_millis("profile_sync_snapshot")?;
        let mut tombstones = previous
            .tombstones
            .iter()
            .filter(|tombstone| {
                tombstone.retention_deadline_unix_ms > now
                    && !live.contains_key(tombstone.key.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        for object in &previous.objects {
            if !live.contains_key(object.key.as_str()) {
                tombstones.push(ManifestTombstone::new(
                    object.key.clone(),
                    descriptor.generation,
                    now.max(1),
                    checked_add_millis(now, 30 * 24 * 60 * 60 * 1_000, "profile_sync_snapshot")?,
                )?);
            }
        }
        let mut manifest = SyncManifest::new_with_tombstones(
            previous.workspace_id.clone(),
            previous.resource_id.clone(),
            fencing_token,
            descriptor.generation,
            Some(previous.generation),
            &descriptor.canonical_root_bytes(),
            descriptor
                .objects
                .iter()
                .map(|object| ManifestObject {
                    key: object.path.clone(),
                    size: object.size,
                    checksum: object.checksum.clone(),
                    source_generation: None,
                })
                .collect(),
            tombstones,
        )?;
        if let Some((proxy_id, revision, _credential)) = proxy_dependency {
            manifest.dependencies.push(ManifestDependency::Proxy {
                proxy_id,
                revision,
                credential_version: None,
                metadata_object: None,
                credential_envelope_object: None,
            });
            manifest.validate()?;
        }
        Ok((descriptor, files, manifest))
    }

    pub fn snapshot_with_proxy_encryption(
        &self,
        profile_path: &Path,
        previous: &SyncManifest,
        fencing_token: u64,
        proxy: Option<RemoteProxySyncSnapshot>,
        client_key: &ClientKey,
        recovery_key: &RecoveryKey,
    ) -> Result<(PayloadDescriptor, Vec<SnapshotBlob>, SyncManifest), Problem> {
        previous.validate()?;
        descriptor_from_manifest(previous)?;
        if fencing_token == 0 {
            return Err(Problem::new(
                "invalid_manifest_fencing_token",
                "A valid profile lease fencing token is required before snapshotting",
                400,
                "sync",
            ));
        }
        validate_profile_source_root(profile_path)?;
        let mut plaintext_files = Vec::new();
        collect_file_paths(profile_path, profile_path, &mut plaintext_files)?;
        let generation = previous.generation.checked_add(1).ok_or_else(unavailable)?;
        let (proxy_dependency, proxy_credential) = match proxy {
            Some(proxy) => {
                let credential = proxy
                    .credential
                    .map(|secret| (proxy.proxy_id.clone(), proxy.revision, secret));
                (
                    Some((proxy.proxy_id, proxy.revision, proxy.credential_version)),
                    credential,
                )
            }
            None => (None, None),
        };
        let mut dependencies = proxy_dependency
            .map(|(proxy_id, revision, credential_version)| {
                vec![ManifestDependency::Proxy {
                    proxy_id,
                    revision,
                    credential_version,
                    metadata_object: None,
                    credential_envelope_object: None,
                }]
            })
            .unwrap_or_default();
        let mut files = Vec::with_capacity(plaintext_files.len() + 1);
        let mut file_chunks = Vec::new();
        let mut live_file_paths = std::collections::BTreeSet::new();
        let key_version = checksum(client_key.as_bytes());
        let previous_objects: BTreeMap<_, _> = previous
            .objects
            .iter()
            .map(|object| (object.key.as_str(), object))
            .collect();
        let previous_file_chunks: BTreeMap<_, _> = previous
            .file_chunks
            .iter()
            .map(|file| (file.path.as_str(), file))
            .collect();
        let mut reused_sources = BTreeMap::new();
        let cache_scope = checksum(
            format!(
                "{}/{}/{}",
                previous.workspace_id.as_str(),
                previous.resource_id.as_str(),
                key_version
            )
            .as_bytes(),
        );
        for (path, source_path, expected_size) in plaintext_files {
            live_file_paths.insert(path.clone());
            if expected_size > MAX_IN_MEMORY_SNAPSHOT_FILE_BYTES {
                let path_metadata_before =
                    fs::symlink_metadata(&source_path).map_err(profile_source_io_problem)?;
                validate_profile_file_size(path_metadata_before.len())?;
                if path_metadata_before.len() != expected_size {
                    return Err(profile_changed_during_snapshot());
                }
                let mut source = fs::File::open(&source_path).map_err(profile_source_io_problem)?;
                let metadata_before = source.metadata().map_err(profile_source_io_problem)?;
                verify_snapshot_file_metadata(
                    &path_metadata_before,
                    &metadata_before,
                    expected_size,
                )?;
                let mut aggregate = Sha256::new();
                let mut chunks = Vec::new();
                let mut remaining = expected_size;
                let mut index = 0usize;
                while remaining > 0 {
                    let chunk_len = remaining.min(PROFILE_FILE_CHUNK_BYTES) as usize;
                    let mut plaintext_chunk = vec![0; chunk_len];
                    source
                        .read_exact(&mut plaintext_chunk)
                        .map_err(profile_source_io_problem)?;
                    aggregate.update(&plaintext_chunk);
                    let (blob, chunk, source_generation) = stage_profile_file_chunk(
                        &self.root,
                        &cache_scope,
                        &previous.workspace_id,
                        &previous.resource_id,
                        &path,
                        index,
                        &plaintext_chunk,
                        client_key,
                        &key_version,
                        previous.generation,
                        &previous_file_chunks,
                        &previous_objects,
                    )?;
                    if let Some(source_generation) = source_generation {
                        reused_sources.insert(blob.key.clone(), source_generation);
                    }
                    files.push(blob);
                    chunks.push(chunk);
                    remaining -= chunk_len as u64;
                    index += 1;
                }
                let mut trailing = [0u8; 1];
                if source
                    .read(&mut trailing)
                    .map_err(profile_source_io_problem)?
                    != 0
                {
                    return Err(profile_changed_during_snapshot());
                }
                let metadata_after = source.metadata().map_err(profile_source_io_problem)?;
                verify_snapshot_file_metadata(&metadata_before, &metadata_after, expected_size)?;
                let path_metadata_after =
                    fs::symlink_metadata(&source_path).map_err(profile_source_io_problem)?;
                verify_snapshot_file_metadata(
                    &path_metadata_before,
                    &path_metadata_after,
                    expected_size,
                )?;
                file_chunks.push(ManifestFileChunks {
                    path,
                    plaintext_size: expected_size,
                    plaintext_checksum: format!("{:x}", aggregate.finalize()),
                    chunks,
                });
                continue;
            }
            let plaintext = read_profile_object(&source_path, expected_size)?;
            if plaintext.len() as u64 != expected_size {
                return Err(Problem::new(
                    "profile_changed_during_snapshot",
                    "Profile files changed while the sync snapshot was being prepared",
                    409,
                    "desktop-projection",
                )
                .retryable(true));
            }
            if (plaintext.len() as u64) > MAX_PROFILE_PLAINTEXT_BYTES {
                let file_checksum = checksum(&plaintext);
                let mut chunks = Vec::new();
                for (index, plaintext_chunk) in plaintext
                    .chunks(PROFILE_FILE_CHUNK_BYTES as usize)
                    .enumerate()
                {
                    let plaintext_checksum = checksum(plaintext_chunk);
                    let chunk_aad = profile_file_chunk_aad(
                        &previous.workspace_id,
                        &previous.resource_id,
                        &path,
                        index as u64,
                        &plaintext_checksum,
                    );
                    let cache_identity = format!(
                        "{}/{}/{}/{}/{}/{}",
                        previous.workspace_id.as_str(),
                        previous.resource_id.as_str(),
                        key_version,
                        path,
                        index,
                        plaintext_checksum
                    );
                    let cache_name = format!("{}.bin", checksum(cache_identity.as_bytes()));
                    let cache_path =
                        PathBuf::from(format!(".sync-cache/{cache_scope}/{cache_name}"));
                    let prior_chunk = previous_file_chunks
                        .get(path.as_str())
                        .and_then(|file| file.chunks.get(index))
                        .filter(|chunk| {
                            chunk.index == index as u64
                                && chunk.plaintext_size == plaintext_chunk.len() as u64
                                && chunk.plaintext_checksum == plaintext_checksum
                        });
                    let cache_absolute_path = confined_path(&self.root, &cache_path)?;
                    let cached = if let Some(cached) = read_optional_snapshot_cache(
                        &cache_absolute_path,
                        MAX_ENCRYPTED_OBJECT_BYTES as u64,
                    )? {
                        (|| {
                            let chunk = AuthenticatedChunk::decode(&cached).ok()?;
                            if chunk.decrypt(client_key, &chunk_aad).ok()?.as_slice()
                                != plaintext_chunk
                            {
                                return None;
                            }
                            let expected_key = format!(
                                ".buglogin/file-chunks/{}/{:016x}-{}-{}",
                                checksum(path.as_bytes()),
                                index,
                                plaintext_checksum,
                                checksum(&cached)
                            );
                            let source_generation = prior_chunk
                                .and_then(|chunk| previous_objects.get(chunk.object.key.as_str()))
                                .filter(|object| {
                                    object.key == expected_key
                                        && object.size == cached.len() as u64
                                        && object.checksum == checksum(&cached)
                                })
                                .map(|object| {
                                    object.source_generation.unwrap_or(previous.generation)
                                });
                            Some((cached, source_generation))
                        })()
                    } else {
                        None
                    };
                    let (encrypted, source_generation, cache_path) = match cached {
                        Some((cached, source_generation)) => {
                            let cache_path =
                                write_snapshot_cache(&self.root, &cache_path, &cached, false)?;
                            (cached, source_generation, cache_path)
                        }
                        None => {
                            let encrypted = AuthenticatedChunk::encrypt(
                                client_key,
                                plaintext_chunk,
                                &chunk_aad,
                            )?
                            .encoded();
                            let staged_path =
                                write_snapshot_cache(&self.root, &cache_path, &encrypted, true)?;
                            (encrypted, None, staged_path)
                        }
                    };
                    let ciphertext_checksum = checksum(&encrypted);
                    let object_key = format!(
                        ".buglogin/file-chunks/{}/{:016x}-{}-{}",
                        checksum(path.as_bytes()),
                        index,
                        plaintext_checksum,
                        ciphertext_checksum
                    );
                    let object = ManifestObject::new(object_key.clone(), &encrypted)?;
                    if let Some(source_generation) = source_generation {
                        reused_sources.insert(object_key.clone(), source_generation);
                    }
                    chunks.push(ManifestFileChunk {
                        index: index as u64,
                        plaintext_size: plaintext_chunk.len() as u64,
                        plaintext_checksum,
                        object: ManifestObjectReference::from(&object),
                    });
                    files.push(SnapshotBlob {
                        key: object_key,
                        path: cache_path,
                        size: object.size,
                        checksum: object.checksum,
                    });
                }
                file_chunks.push(ManifestFileChunks {
                    path,
                    plaintext_size: plaintext.len() as u64,
                    plaintext_checksum: file_checksum,
                    chunks,
                });
                continue;
            }
            {
                let plaintext_checksum = checksum(&plaintext);
                let cache_identity = format!(
                    "{}/{}/{}/{}/{}",
                    previous.workspace_id.as_str(),
                    previous.resource_id.as_str(),
                    key_version,
                    path,
                    plaintext_checksum
                );
                let cache_name = format!("{}.bin", checksum(cache_identity.as_bytes()));
                let cache_path =
                    PathBuf::from(format!(".sync-cache/{}/{}", cache_scope, cache_name));
                let prior = previous_objects.get(path.as_str()).copied();
                let cache_absolute_path = confined_path(&self.root, &cache_path)?;
                let cached = if let Some(cached) = read_optional_snapshot_cache(
                    &cache_absolute_path,
                    MAX_ENCRYPTED_OBJECT_BYTES as u64,
                )? {
                    if let Ok(chunk) = AuthenticatedChunk::decode(&cached) {
                        if let Ok(decrypted) = chunk.decrypt(
                            client_key,
                            &profile_object_aad_v2(
                                &previous.workspace_id,
                                &previous.resource_id,
                                &path,
                            ),
                        ) {
                            if checksum(&decrypted) == plaintext_checksum && decrypted == plaintext
                            {
                                let source_generation = prior
                                    .filter(|prior| {
                                        prior.size == cached.len() as u64
                                            && prior.checksum == checksum(&cached)
                                    })
                                    .map(|prior| {
                                        prior.source_generation.unwrap_or(previous.generation)
                                    });
                                Some((cached, source_generation))
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };
                let (encrypted, source_generation, cached_path) = match cached {
                    Some((cached, source_generation)) => {
                        let cached_path =
                            write_snapshot_cache(&self.root, &cache_path, &cached, false)?;
                        (cached, source_generation, cached_path)
                    }
                    None => {
                        let encrypted = AuthenticatedChunk::encrypt(
                            client_key,
                            &plaintext,
                            &profile_object_aad_v2(
                                &previous.workspace_id,
                                &previous.resource_id,
                                &path,
                            ),
                        )?
                        .encoded();
                        let cached_path =
                            write_snapshot_cache(&self.root, &cache_path, &encrypted, true)?;
                        (encrypted, None, cached_path)
                    }
                };
                if let Some(source_generation) = source_generation {
                    reused_sources.insert(path.clone(), source_generation);
                }
                if encrypted.len() > MAX_ENCRYPTED_OBJECT_BYTES {
                    return Err(problem(
                        "profile_object_too_large",
                        "A profile file exceeds the encrypted sync object limit",
                    ));
                }
                let encrypted_checksum = checksum(&encrypted);
                files.push(SnapshotBlob {
                    key: path.clone(),
                    path: cached_path,
                    size: encrypted.len() as u64,
                    checksum: encrypted_checksum,
                });
            }
        }
        let envelope_cache = PathBuf::from(format!(
            ".sync-cache/{cache_scope}/resource-key-envelope.json"
        ));
        let cached_envelope = (|| -> Result<Option<Vec<u8>>, Problem> {
            let path = confined_path(&self.root, &envelope_cache)?;
            let Some(bytes) = read_projection_marker(&path, "profile_snapshot_cache_invalid")?
            else {
                return Ok(None);
            };
            let Ok(envelope) = serde_json::from_slice::<ResourceKeyEnvelope>(&bytes) else {
                return Ok(None);
            };
            let Ok(unwrapped) =
                envelope.unwrap(&previous.workspace_id, &previous.resource_id, recovery_key)
            else {
                return Ok(None);
            };
            Ok((unwrapped.as_bytes() == client_key.as_bytes()).then_some(bytes))
        })()?;
        let envelope_bytes = match cached_envelope {
            Some(bytes) => bytes,
            None => {
                let envelope = ResourceKeyEnvelope::wrap(
                    previous.workspace_id.clone(),
                    previous.resource_id.clone(),
                    client_key,
                    recovery_key,
                )?;
                let bytes = serde_json::to_vec(&envelope).map_err(|_| unavailable())?;
                write_projection_verified(&self.root, &envelope_cache, &bytes, &checksum(&bytes))?;
                bytes
            }
        };
        files.push(stage_snapshot_metadata(
            &self.root,
            &cache_scope,
            RESOURCE_KEY_ENVELOPE_KEY,
            &envelope_bytes,
        )?);

        if let Some((proxy_id, revision, secret)) = proxy_credential {
            let proxy_envelope_key = format!(
                ".buglogin/proxy-dependencies/{}/{}/credential-envelope.json",
                proxy_id.as_str(),
                revision
            );
            let proxy_cache = PathBuf::from(format!(
                ".sync-cache/{cache_scope}/proxy-{}-{}-envelope.json",
                checksum(proxy_id.as_str().as_bytes()),
                revision
            ));
            let cached_proxy_envelope = (|| -> Result<Option<Vec<u8>>, Problem> {
                let path = confined_path(&self.root, &proxy_cache)?;
                let Some(bytes) = read_projection_marker(&path, "profile_snapshot_cache_invalid")?
                else {
                    return Ok(None);
                };
                let Ok(envelope) = serde_json::from_slice::<ProxyCredentialEnvelope>(&bytes) else {
                    return Ok(None);
                };
                let Ok(unwrapped) = envelope.open(
                    &previous.workspace_id,
                    &previous.resource_id,
                    &proxy_id,
                    revision,
                    client_key,
                ) else {
                    return Ok(None);
                };
                Ok((unwrapped == secret.as_bytes()).then_some(bytes))
            })()?;
            let proxy_envelope_bytes = match cached_proxy_envelope {
                Some(bytes) => bytes,
                None => {
                    let envelope = ProxyCredentialEnvelope::seal(
                        previous.workspace_id.clone(),
                        previous.resource_id.clone(),
                        proxy_id.clone(),
                        revision,
                        client_key,
                        secret.as_bytes(),
                    )?;
                    let bytes = serde_json::to_vec(&envelope).map_err(|_| unavailable())?;
                    write_projection_verified(&self.root, &proxy_cache, &bytes, &checksum(&bytes))?;
                    bytes
                }
            };
            files.push(stage_snapshot_metadata(
                &self.root,
                &cache_scope,
                &proxy_envelope_key,
                &proxy_envelope_bytes,
            )?);
            let dependency_matches = dependencies.iter().any(|dependency| {
                matches!(dependency, ManifestDependency::Proxy {
                    proxy_id: dependency_id,
                    revision: dependency_revision,
                    ..
                } if dependency_id == &proxy_id && *dependency_revision == revision)
            });
            if !dependency_matches {
                return Err(problem(
                    "proxy_dependency_missing",
                    "Proxy credential does not match the profile dependency",
                ));
            }
            let staged_proxy_envelope = files.last().ok_or_else(unavailable)?;
            let object = ManifestObject {
                key: proxy_envelope_key,
                size: staged_proxy_envelope.size,
                checksum: staged_proxy_envelope.checksum.clone(),
                source_generation: None,
            };
            let Some(ManifestDependency::Proxy {
                credential_envelope_object,
                ..
            }) = dependencies.iter_mut().find(|dependency| {
                matches!(
                    dependency,
                    ManifestDependency::Proxy { proxy_id: id, .. } if id == &proxy_id
                )
            })
            else {
                return Err(problem(
                    "proxy_dependency_missing",
                    "Proxy credential does not match the profile dependency",
                ));
            };
            *credential_envelope_object = Some(ManifestObjectReference::from(&object));
        }

        let objects = files
            .iter()
            .map(|blob| PayloadObject {
                path: blob.key.clone(),
                size: blob.size,
                checksum: blob.checksum.clone(),
            })
            .collect::<Vec<_>>();
        let descriptor = PayloadDescriptor::new_with_file_chunks(
            previous.workspace_id.clone(),
            previous.resource_id.clone(),
            generation,
            objects,
            file_chunks.clone(),
        )?;
        let live_object_keys = descriptor
            .objects
            .iter()
            .map(|object| object.path.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let live_chunked_file_paths = descriptor
            .file_chunks
            .iter()
            .map(|file| file.path.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let now = unix_time_millis("profile_sync_snapshot")?;
        let mut tombstones = previous
            .tombstones
            .iter()
            .filter(|tombstone| {
                tombstone.retention_deadline_unix_ms > now
                    && !live_object_keys.contains(tombstone.key.as_str())
                    && (tombstone.object_retired
                        || !live_file_paths.contains(tombstone.key.as_str()))
            })
            .cloned()
            .collect::<Vec<_>>();
        for object in &previous.objects {
            if !live_object_keys.contains(object.key.as_str())
                && (!live_file_paths.contains(&object.key)
                    || live_chunked_file_paths.contains(object.key.as_str()))
            {
                tombstones.push(ManifestTombstone::new_object(
                    object.key.clone(),
                    generation,
                    now.max(1),
                    checked_add_millis(now, 30 * 24 * 60 * 60 * 1_000, "profile_sync_snapshot")?,
                )?);
            }
        }
        for file in &previous.file_chunks {
            if !live_file_paths.contains(&file.path) {
                tombstones.push(ManifestTombstone::new(
                    file.path.clone(),
                    generation,
                    now.max(1),
                    checked_add_millis(now, 30 * 24 * 60 * 60 * 1_000, "profile_sync_snapshot")?,
                )?);
            }
        }
        let tombstone_cache = PathBuf::from(format!(
            ".sync-cache/{cache_scope}/tombstones-{}-{}-{}.json",
            previous.generation, previous.root_checksum, descriptor.root_checksum
        ));
        let snapshot_guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let cached_tombstones = (|| -> Result<Option<Vec<ManifestTombstone>>, Problem> {
            let path = confined_path(&self.root, &tombstone_cache)?;
            let bytes = match read_projection_marker(&path, "profile_snapshot_cache_invalid")? {
                Some(bytes) => bytes,
                None => return Ok(None),
            };
            let cached: SnapshotTombstoneCache = serde_json::from_slice(&bytes).map_err(|_| {
                projection_state_problem(
                    "profile_snapshot_cache_invalid",
                    "The staged profile retry metadata is corrupt; preserve local profile data",
                    422,
                )
            })?;
            let cached_tombstone_bytes =
                serde_json::to_vec(&cached.tombstones).map_err(|_| unavailable())?;
            if cached.base_generation != previous.generation
                || cached.base_root_checksum != previous.root_checksum
                || cached.descriptor_root_checksum != descriptor.root_checksum
                || cached.checksum != checksum(&cached_tombstone_bytes)
            {
                return Err(projection_state_problem(
                    "profile_snapshot_cache_invalid",
                    "The staged profile retry metadata failed integrity validation",
                    422,
                ));
            }
            if cached.tombstones.iter().any(|item| {
                item.deleted_at_unix_ms == 0
                    || item.retention_deadline_unix_ms <= item.deleted_at_unix_ms
            }) {
                return Err(projection_state_problem(
                    "profile_snapshot_cache_invalid",
                    "The staged profile retry metadata is corrupt; preserve local profile data",
                    422,
                ));
            }
            if cached
                .tombstones
                .iter()
                .any(|item| item.retention_deadline_unix_ms <= now)
            {
                return Ok(None);
            }
            let identity = |items: &[ManifestTombstone]| {
                let mut items = items
                    .iter()
                    .map(|item| {
                        (
                            item.key.clone(),
                            item.deleted_generation,
                            item.object_retired,
                        )
                    })
                    .collect::<Vec<_>>();
                items.sort_unstable();
                items
            };
            if identity(&cached.tombstones) != identity(&tombstones) {
                return Err(projection_state_problem(
                    "profile_snapshot_cache_invalid",
                    "The staged profile retry metadata does not match this snapshot",
                    422,
                ));
            }
            Ok(Some(cached.tombstones))
        })()?;
        let tombstones = match cached_tombstones {
            Some(tombstones) => tombstones,
            None => {
                let tombstone_bytes = serde_json::to_vec(&tombstones).map_err(|_| unavailable())?;
                let cache = SnapshotTombstoneCache {
                    base_generation: previous.generation,
                    base_root_checksum: previous.root_checksum.clone(),
                    descriptor_root_checksum: descriptor.root_checksum.clone(),
                    checksum: checksum(&tombstone_bytes),
                    tombstones,
                };
                let bytes = serde_json::to_vec(&cache).map_err(|_| unavailable())?;
                if bytes.len() > 64 * 1024 {
                    return Err(Problem::new(
                        "profile_tombstones_too_large",
                        "Profile deletion history exceeds the local retry metadata limit",
                        413,
                        "desktop-projection",
                    ));
                }
                write_projection_verified(&self.root, &tombstone_cache, &bytes, &checksum(&bytes))?;
                cache.tombstones
            }
        };
        drop(snapshot_guard);
        let envelope_object = descriptor
            .objects
            .iter()
            .find(|object| object.path == RESOURCE_KEY_ENVELOPE_KEY)
            .ok_or_else(unavailable)?;
        let payload_encryption = ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some(key_version),
            resource_key_envelope: ManifestObjectReference {
                key: envelope_object.path.clone(),
                size: envelope_object.size,
                checksum: envelope_object.checksum.clone(),
            },
        };
        let manifest_objects = descriptor
            .objects
            .iter()
            .map(|object| ManifestObject {
                key: object.path.clone(),
                size: object.size,
                checksum: object.checksum.clone(),
                source_generation: None,
            })
            .collect::<Vec<_>>();
        let mut manifest = if descriptor.file_chunks.is_empty() {
            SyncManifest::new_with_tombstones(
                previous.workspace_id.clone(),
                previous.resource_id.clone(),
                fencing_token,
                descriptor.generation,
                Some(previous.generation),
                &descriptor.canonical_root_bytes(),
                manifest_objects,
                tombstones,
            )?
        } else {
            SyncManifest::new_chunked_with_tombstones(
                previous.workspace_id.clone(),
                previous.resource_id.clone(),
                fencing_token,
                descriptor.generation,
                Some(previous.generation),
                &descriptor.canonical_root_bytes(),
                manifest_objects,
                tombstones,
                payload_encryption.clone(),
                descriptor.file_chunks.clone(),
            )?
        };
        manifest.payload_encryption = Some(payload_encryption);
        manifest.dependencies = dependencies;
        for object in &mut manifest.objects {
            object.source_generation = reused_sources.get(&object.key).copied();
        }
        manifest.validate()?;
        Ok((descriptor, files, manifest))
    }
}

struct DecryptedGenerationStaging {
    staging_path: PathBuf,
    generation_path: PathBuf,
    local_objects: Vec<PayloadObject>,
    published: bool,
}

impl DecryptedGenerationStaging {
    fn new(root: &Path, profile_id: &ProfileId, manifest: &SyncManifest) -> Result<Self, Problem> {
        let profile_root = confined_path(root, Path::new(profile_id.as_str()))?;
        fs::create_dir_all(&profile_root).map_err(projection_staging_io_problem)?;
        let generation_path = decrypted_generation_path(root, profile_id, manifest)?;
        let staging_name = format!(
            "staging/{}-{}-{}",
            manifest.generation,
            std::process::id(),
            unique_suffix()
        );
        let staging_path = confined_path(&profile_root, Path::new(&staging_name))?;
        fs::create_dir_all(&staging_path).map_err(projection_staging_io_problem)?;
        Ok(Self {
            staging_path,
            generation_path,
            local_objects: Vec::new(),
            published: false,
        })
    }

    fn write_object(&mut self, path: &str, bytes: &[u8]) -> Result<(), Problem> {
        let object = PayloadObject::new(path.to_owned(), bytes)?;
        write_projection_verified(&self.staging_path, Path::new(path), bytes, &object.checksum)?;
        self.local_objects.push(object);
        Ok(())
    }

    fn create_file(&self, path: &str) -> Result<(fs::File, PathBuf), Problem> {
        let destination = confined_path(&self.staging_path, Path::new(path))?;
        let parent = destination.parent().ok_or_else(unavailable)?;
        fs::create_dir_all(parent).map_err(projection_staging_io_problem)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(projection_staging_io_problem)?;
        Ok((file, destination))
    }

    fn finish_file(
        &mut self,
        path: &str,
        file: fs::File,
        size: u64,
        checksum: String,
    ) -> Result<(), Problem> {
        file.sync_all().map_err(projection_staging_io_problem)?;
        let destination = confined_path(&self.staging_path, Path::new(path))?;
        if fs::metadata(destination)
            .map_err(projection_staging_io_problem)?
            .len()
            != size
        {
            return Err(problem(
                "profile_file_size_invalid",
                "Reconstructed profile file size is invalid",
            ));
        }
        self.local_objects.push(PayloadObject {
            path: path.to_owned(),
            size,
            checksum,
        });
        Ok(())
    }

    fn publish(
        mut self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
    ) -> Result<ProjectionReceipt, Problem> {
        let profile_root = self
            .staging_path
            .parent()
            .and_then(Path::parent)
            .ok_or_else(unavailable)?;
        ensure_hydration_generation_current(profile_root, profile_id, manifest)?;
        let descriptor = PayloadDescriptor::new(
            manifest.workspace_id.clone(),
            manifest.resource_id.clone(),
            manifest.generation,
            std::mem::take(&mut self.local_objects),
        )?;
        let marker = EncryptedProjectionMarker {
            state: "complete".to_owned(),
            profile_id: profile_id.clone(),
            generation: manifest.generation,
            remote_root_checksum: manifest.root_checksum.clone(),
            local_objects: descriptor.objects,
        };
        let marker_bytes = serde_json::to_vec(&marker).map_err(|_| unavailable())?;
        write_projection_verified(
            &self.staging_path,
            Path::new(MARKER),
            &marker_bytes,
            &checksum(&marker_bytes),
        )?;
        if let Some(parent) = self.generation_path.parent() {
            fs::create_dir_all(parent).map_err(projection_staging_io_problem)?;
        }
        if local_generation_directory_exists(&self.generation_path)? {
            return verify_decrypted_generation(profile_id, manifest, &self.generation_path);
        }
        fs::rename(&self.staging_path, &self.generation_path)
            .map_err(projection_staging_io_problem)?;
        self.published = true;
        verify_decrypted_generation(profile_id, manifest, &self.generation_path)
    }
}

impl Drop for DecryptedGenerationStaging {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_dir_all(&self.staging_path);
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectionMarker {
    state: String,
    profile_id: ProfileId,
    generation: u64,
    root_checksum: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ActiveSessionMarker {
    state: String,
    profile_id: ProfileId,
    generation: u64,
    root_checksum: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_epoch: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct HydrationFenceMarker {
    profile_id: ProfileId,
    generation: u64,
    root_checksum: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncCommitMarker {
    profile_id: ProfileId,
    base_generation: u64,
    base_root_checksum: String,
    target_generation: u64,
    target_root_checksum: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_epoch: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotTombstoneCache {
    base_generation: u64,
    base_root_checksum: String,
    descriptor_root_checksum: String,
    tombstones: Vec<ManifestTombstone>,
    checksum: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct EncryptedProjectionMarker {
    state: String,
    profile_id: ProfileId,
    generation: u64,
    remote_root_checksum: String,
    local_objects: Vec<PayloadObject>,
}

fn verify_decrypted_generation(
    profile_id: &ProfileId,
    manifest: &SyncManifest,
    generation_path: &Path,
) -> Result<ProjectionReceipt, Problem> {
    if !local_generation_directory_exists(generation_path)? {
        return Err(problem(
            "profile_local_generation_missing",
            "The local profile generation is missing; preserve profile data and retry hydration",
        ));
    }
    let marker_path = generation_path.join(MARKER);
    let marker_bytes = read_projection_marker(&marker_path, "profile_projection_marker_invalid")?
        .ok_or_else(|| {
        problem(
            "profile_projection_marker_missing",
            "Profile projection marker is missing",
        )
    })?;
    let marker: EncryptedProjectionMarker =
        serde_json::from_slice(&marker_bytes).map_err(|_| {
            problem(
                "profile_projection_marker_invalid",
                "Profile projection marker is invalid",
            )
        })?;
    if marker.state != "complete"
        || &marker.profile_id != profile_id
        || marker.generation != manifest.generation
        || marker.remote_root_checksum != manifest.root_checksum
    {
        return Err(problem(
            "profile_projection_marker_invalid",
            "Profile projection marker is invalid",
        ));
    }
    validate_local_projection_objects(generation_path, &marker.local_objects)?;
    let mut expected_paths: std::collections::HashSet<&str> = marker
        .local_objects
        .iter()
        .map(|object| object.path.as_str())
        .collect();
    let mut unexpected_file = false;
    visit_profile_file_paths(generation_path, generation_path, &mut |path, _, _| {
        if !expected_paths.remove(path.as_str()) {
            unexpected_file = true;
        }
        Ok(())
    })?;
    if unexpected_file || !expected_paths.is_empty() {
        return Err(problem(
            "profile_projection_incomplete",
            "Profile projection file set does not match its verified marker",
        ));
    }
    for object in &marker.local_objects {
        verify_staged_file(generation_path, object)?;
    }
    Ok(ProjectionReceipt {
        profile_id: profile_id.clone(),
        generation: manifest.generation,
        root_checksum: manifest.root_checksum.clone(),
        path: generation_path.to_path_buf(),
    })
}

fn local_generation_directory_exists(path: &Path) -> Result<bool, Problem> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(projection_state_problem(
            "profile_local_generation_invalid",
            "The local profile generation is not a regular directory; preserve local profile data",
            422,
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(projection_generation_io_problem(error)),
    }
}

fn local_generation_matches_projection_marker(
    profile_id: &ProfileId,
    generation: u64,
    root_checksum: &str,
    generation_path: &Path,
) -> Result<bool, Problem> {
    let generation_metadata =
        fs::symlink_metadata(generation_path).map_err(projection_generation_io_problem)?;
    if !generation_metadata.is_dir() || generation_metadata.file_type().is_symlink() {
        return Err(problem(
            "profile_local_generation_invalid",
            "The preserved profile generation is not a regular directory; preserve local profile data",
        ));
    }
    let marker_path = generation_path.join(MARKER);
    let Some(marker_bytes) =
        read_projection_marker(&marker_path, "profile_projection_marker_invalid")?
    else {
        return Err(problem(
            "profile_projection_marker_missing",
            "The preserved profile generation has no integrity marker; preserve local profile data",
        ));
    };
    let marker: EncryptedProjectionMarker =
        serde_json::from_slice(&marker_bytes).map_err(|_| {
            problem(
                "profile_projection_marker_invalid",
                "Profile projection marker is invalid",
            )
        })?;
    if marker.state != "complete"
        || marker.profile_id != *profile_id
        || marker.generation != generation
        || marker.remote_root_checksum != root_checksum
    {
        return Err(problem(
            "profile_projection_marker_invalid",
            "The preserved profile generation marker does not match its identity",
        ));
    }
    validate_local_projection_objects(generation_path, &marker.local_objects)?;

    let mut expected_paths: std::collections::HashSet<&str> = marker
        .local_objects
        .iter()
        .map(|object| object.path.as_str())
        .collect();
    let mut unexpected_file = false;
    visit_profile_file_paths(generation_path, generation_path, &mut |path, _, _| {
        if !expected_paths.remove(path.as_str()) {
            unexpected_file = true;
        }
        Ok(())
    })?;
    if unexpected_file || !expected_paths.is_empty() {
        return Ok(false);
    }
    for object in &marker.local_objects {
        match verify_staged_file(generation_path, object) {
            Ok(()) => {}
            Err(error) if error.code == "profile_projection_file_invalid" => return Ok(false),
            Err(error) => return Err(error),
        }
    }
    Ok(true)
}

fn local_generation_is_recoverable(
    profile_id: &ProfileId,
    generation: u64,
    root_checksum: &str,
    generation_path: &Path,
) -> Result<bool, Problem> {
    let generation_metadata = match fs::symlink_metadata(generation_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(projection_marker_io_problem(error)),
    };
    if !generation_metadata.is_dir() || generation_metadata.file_type().is_symlink() {
        return Ok(false);
    }
    let marker_path = generation_path.join(MARKER);
    let Some(marker_bytes) =
        read_projection_marker(&marker_path, "profile_projection_marker_invalid")?
    else {
        return Err(problem(
            "profile_projection_marker_missing",
            "The preserved profile generation has no integrity marker; preserve local profile data",
        ));
    };
    let marker: EncryptedProjectionMarker =
        serde_json::from_slice(&marker_bytes).map_err(|_| {
            problem(
                "profile_projection_marker_invalid",
                "Profile projection marker is invalid; preserve local profile data",
            )
        })?;
    if marker.state != "complete"
        || marker.profile_id != *profile_id
        || marker.generation != generation
        || marker.remote_root_checksum != root_checksum
    {
        return Ok(false);
    }
    validate_local_projection_objects(generation_path, &marker.local_objects)?;
    // A valid base marker identifies the interrupted session. Its files may
    // have been edited, created, or deleted by the browser; those bytes are
    // the dirty recovery state and must not be compared with the base hashes.
    visit_profile_file_paths(generation_path, generation_path, &mut |_, _, _| Ok(()))?;
    Ok(true)
}

fn validate_local_projection_objects(
    generation_path: &Path,
    objects: &[PayloadObject],
) -> Result<(), Problem> {
    let mut paths = std::collections::HashSet::with_capacity(objects.len());
    for object in objects {
        if object.size > MAX_CHUNKED_PROFILE_FILE_BYTES
            || !valid_manifest_checksum(&object.checksum)
            || !paths.insert(object.path.as_str())
        {
            return Err(problem(
                "profile_projection_marker_invalid",
                "Profile projection marker contains invalid file metadata; preserve local profile data",
            ));
        }
        confined_path(generation_path, Path::new(&object.path)).map_err(|_| {
            problem(
                "profile_projection_marker_invalid",
                "Profile projection marker contains an invalid file path; preserve local profile data",
            )
        })?;
    }
    Ok(())
}

fn verify_staged_file(root: &Path, object: &PayloadObject) -> Result<(), Problem> {
    let path = confined_path(root, Path::new(&object.path))?;
    let metadata = fs::symlink_metadata(&path).map_err(projection_generation_io_problem)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != object.size {
        return Err(problem(
            "profile_projection_file_invalid",
            "Profile projection file failed verification",
        ));
    }
    let mut file = fs::File::open(path).map_err(projection_generation_io_problem)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(projection_generation_io_problem)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    if format!("{:x}", digest.finalize()) != object.checksum {
        return Err(problem(
            "profile_projection_file_invalid",
            "Profile projection file failed verification",
        ));
    }
    Ok(())
}

fn projection_generation_io_problem(error: std::io::Error) -> Problem {
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::UnexpectedEof => problem(
            "profile_projection_file_invalid",
            "Profile projection file is missing or truncated; preserve local profile data",
        ),
        std::io::ErrorKind::PermissionDenied => Problem::new(
            "profile_source_not_readable",
            "A local profile generation file cannot be read; check its permissions",
            500,
            "desktop-projection",
        ),
        std::io::ErrorKind::StorageFull => Problem::new(
            "device_storage_full",
            "Device storage is full",
            507,
            "desktop-projection",
        ),
        _ => unavailable(),
    }
}

fn decrypted_generation_path(
    root: &Path,
    profile_id: &ProfileId,
    manifest: &SyncManifest,
) -> Result<PathBuf, Problem> {
    decrypted_generation_path_parts(
        root,
        profile_id,
        manifest.generation,
        &manifest.root_checksum,
    )
}

fn decrypted_generation_path_parts(
    root: &Path,
    profile_id: &ProfileId,
    generation: u64,
    root_checksum: &str,
) -> Result<PathBuf, Problem> {
    let relative = PathBuf::from(profile_id.as_str())
        .join("generations")
        .join(format!("{generation}-{root_checksum}"));
    confined_path(root, &relative)
}

fn read_projection_marker(
    path: &Path,
    invalid_code: &'static str,
) -> Result<Option<Vec<u8>>, Problem> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(projection_marker_io_problem(error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
        return Err(problem(
            invalid_code,
            "A local profile recovery marker is invalid; preserve local profile data",
        ));
    }
    match read_bounded_regular_file(path, 64 * 1024, projection_marker_io_problem)? {
        Some(bytes) => Ok(Some(bytes)),
        None => Err(problem(
            invalid_code,
            "A local profile recovery marker changed while it was being read",
        )),
    }
}

fn read_bounded_regular_file(
    path: &Path,
    max_bytes: u64,
    map_io: fn(std::io::Error) -> Problem,
) -> Result<Option<Vec<u8>>, Problem> {
    let before = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(map_io(error)),
    };
    if !before.is_file() || before.file_type().is_symlink() || before.len() > max_bytes {
        return Ok(None);
    }
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(map_io(error)),
    };
    let opened = file.metadata().map_err(map_io)?;
    if !opened.is_file() || opened.len() != before.len() || opened.len() > max_bytes {
        return Ok(None);
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    std::io::Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(map_io)?;
    if bytes.len() as u64 != opened.len() || bytes.len() as u64 > max_bytes {
        return Ok(None);
    }
    let after = fs::symlink_metadata(path).map_err(map_io)?;
    let reopened = file.metadata().map_err(map_io)?;
    if !after.is_file()
        || after.file_type().is_symlink()
        || after.len() != opened.len()
        || !reopened.is_file()
        || reopened.len() != opened.len()
        || matches!(
            (before.modified(), after.modified()),
            (Ok(before), Ok(after)) if before != after
        )
    {
        return Ok(None);
    }
    Ok(Some(bytes))
}

fn projection_marker_io_problem(error: std::io::Error) -> Problem {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => Problem::new(
            "profile_marker_not_readable",
            "A local profile recovery marker cannot be read; check its permissions",
            500,
            "desktop-projection",
        ),
        std::io::ErrorKind::StorageFull => Problem::new(
            "device_storage_full",
            "Device storage is full",
            507,
            "desktop-projection",
        ),
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput => Problem::new(
            "profile_marker_read_failed",
            "A local profile recovery marker contains invalid data",
            422,
            "desktop-projection",
        ),
        _ => unavailable(),
    }
}

fn valid_manifest_checksum(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn validate_session_epoch(session_epoch: &str) -> Result<(), Problem> {
    if session_epoch.is_empty()
        || session_epoch.len() > 128
        || !session_epoch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(problem(
            "profile_session_epoch_invalid",
            "The profile session identifier is invalid",
        ));
    }
    Ok(())
}

fn projection_state_problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "desktop-projection")
}

fn write_hydration_fence(
    root: &Path,
    profile_id: &ProfileId,
    generation: u64,
    root_checksum: &str,
) -> Result<(), Problem> {
    let marker = HydrationFenceMarker {
        profile_id: profile_id.clone(),
        generation,
        root_checksum: root_checksum.to_owned(),
    };
    let bytes = serde_json::to_vec(&marker).map_err(|_| unavailable())?;
    let relative = PathBuf::from(profile_id.as_str()).join(HYDRATION_FENCE_MARKER);
    write_projection_verified(root, &relative, &bytes, &checksum(&bytes))
}

fn ensure_hydration_generation_current(
    profile_root: &Path,
    profile_id: &ProfileId,
    manifest: &SyncManifest,
) -> Result<(), Problem> {
    let fence_path = profile_root.join(HYDRATION_FENCE_MARKER);
    if let Some(bytes) = read_projection_marker(&fence_path, "profile_hydration_fence_invalid")? {
        let fence: HydrationFenceMarker = serde_json::from_slice(&bytes).map_err(|_| {
            problem(
                "profile_hydration_fence_invalid",
                "The local hydration fence is invalid; preserve local profile data",
            )
        })?;
        if fence.profile_id != *profile_id
            || fence.generation == 0
            || !valid_manifest_checksum(&fence.root_checksum)
        {
            return Err(problem(
                "profile_hydration_fence_invalid",
                "The local hydration fence has invalid identity; preserve local profile data",
            ));
        }
        if fence.generation > manifest.generation
            || (fence.generation == manifest.generation
                && fence.root_checksum != manifest.root_checksum)
        {
            return Err(projection_state_problem(
                "profile_hydration_stale",
                "A newer profile generation already superseded this hydration",
                409,
            ));
        }
    }
    let mut pending_commit_base: Option<(u64, String)> = None;
    let commit_path = profile_root.join(SYNC_COMMIT_MARKER);
    let commit_bytes = match fs::symlink_metadata(&commit_path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= 64 * 1024 =>
        {
            Some(
                read_projection_marker(&commit_path, "profile_sync_commit_marker_invalid")?
                    .ok_or_else(|| {
                        projection_state_problem(
                            "profile_sync_commit_marker_invalid",
                            "The local sync commit record disappeared during validation",
                            422,
                        )
                    })?,
            )
        }
        Ok(_) => {
            return Err(projection_state_problem(
                "profile_sync_commit_marker_invalid",
                "The local sync commit record is invalid; local profile data has been preserved",
                422,
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(projection_marker_io_problem(error)),
    };
    if let Some(bytes) = commit_bytes {
        let commit: SyncCommitMarker = serde_json::from_slice(&bytes).map_err(|_| {
            problem(
                "profile_sync_commit_marker_invalid",
                "The local sync commit record is invalid; local profile data has been preserved",
            )
        })?;
        if commit.profile_id != *profile_id {
            return Err(projection_state_problem(
                "profile_sync_commit_marker_invalid",
                "The local sync commit record does not match this profile",
                422,
            ));
        }
        if commit.base_generation.checked_add(1) != Some(commit.target_generation)
            || !valid_manifest_checksum(&commit.base_root_checksum)
            || !valid_manifest_checksum(&commit.target_root_checksum)
        {
            return Err(projection_state_problem(
                "profile_sync_commit_marker_invalid",
                "The local sync commit record contains invalid generation metadata",
                422,
            ));
        }
        let manifest_identity = (manifest.generation, manifest.root_checksum.as_str());
        let base_identity = (commit.base_generation, commit.base_root_checksum.as_str());
        let target_identity = (
            commit.target_generation,
            commit.target_root_checksum.as_str(),
        );
        pending_commit_base = Some((commit.base_generation, commit.base_root_checksum.clone()));
        if manifest_identity == base_identity {
            return Err(projection_state_problem(
                "profile_local_sync_recovery_required",
                "A local profile sync is pending; recover the preserved local generation before hydration",
                409,
            ));
        }
        if manifest_identity != target_identity {
            return Err(projection_state_problem(
                "profile_hydration_stale",
                "A newer pending sync superseded this hydration; local data was preserved",
                409,
            ));
        }
    }
    let marker_path = profile_root.join(ACTIVE_SESSION_MARKER);
    let metadata = match fs::symlink_metadata(&marker_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(projection_marker_io_problem(error)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
        return Err(projection_state_problem(
            "profile_session_marker_invalid",
            "The local profile session marker is invalid; preserve the profile data and contact support",
            422,
        ));
    }
    let bytes = read_projection_marker(&marker_path, "profile_session_marker_invalid")?
        .ok_or_else(|| {
            projection_state_problem(
                "profile_session_marker_invalid",
                "The local profile session marker disappeared during validation",
                422,
            )
        })?;
    let marker: ActiveSessionMarker = serde_json::from_slice(&bytes).map_err(|_| {
        problem(
            "profile_session_marker_invalid",
            "The local profile session marker is invalid; preserve the profile data and contact support",
        )
    })?;
    if marker.state != "active"
        || marker.profile_id != *profile_id
        || marker.generation == 0
        || !valid_manifest_checksum(&marker.root_checksum)
    {
        return Err(projection_state_problem(
            "profile_session_marker_invalid",
            "The local profile session marker does not match this profile",
            422,
        ));
    }
    let active_identity = (marker.generation, marker.root_checksum.as_str());
    let manifest_identity = (manifest.generation, manifest.root_checksum.as_str());
    let active_matches_pending_base = pending_commit_base
        .as_ref()
        .is_some_and(|(generation, checksum)| active_identity == (*generation, checksum.as_str()));
    if active_identity != manifest_identity && !active_matches_pending_base {
        return Err(projection_state_problem(
            "profile_hydration_stale",
            "A newer local profile session superseded this hydration; local data was preserved",
            409,
        ));
    }
    Ok(())
}

fn proxy_metadata_keys(manifest: &SyncManifest) -> std::collections::BTreeSet<String> {
    manifest
        .dependencies
        .iter()
        .filter_map(|dependency| match dependency {
            ManifestDependency::Proxy {
                metadata_object: Some(metadata_object),
                ..
            } => Some(metadata_object.key.clone()),
            ManifestDependency::Proxy {
                metadata_object: None,
                ..
            } => None,
        })
        .collect()
}

async fn download_verified_manifest_object(
    client: &RemoteApiClient,
    workspace_id: &buglogin_domain::ids::WorkspaceId,
    auth: &CoordinationAuth,
    resource_id: &buglogin_domain::ids::ResourceId,
    generation: u64,
    key: &str,
    size: u64,
    checksum: &str,
    correlation_id: &str,
    repair_stale_proxy: bool,
) -> Result<(Vec<u8>, tokio::sync::OwnedSemaphorePermit), Problem> {
    let memory_permit = profile_transfer_budget::acquire(size, 2, correlation_id).await?;
    let _download_slot = profile_download_slots()
        .acquire_owned()
        .await
        .map_err(|_| unavailable())?;
    let scope = CoordinationScope { workspace_id, auth };
    let bytes = if repair_stale_proxy {
        client
            .download_blob_for_proxy_repair(
                scope,
                resource_id,
                key,
                generation,
                checksum,
                size,
                correlation_id,
            )
            .await
    } else {
        client
            .download_blob(
                CoordinationScope { workspace_id, auth },
                resource_id,
                key,
                generation,
                checksum,
                size,
                correlation_id,
            )
            .await
    }?;
    Ok((bytes, memory_permit))
}

async fn read_cached_manifest_object(
    root: &Path,
    cache_scope: &str,
    reference: &ManifestObjectReference,
    legacy_alias: Option<&str>,
    correlation_id: &str,
) -> Result<Option<(Vec<u8>, tokio::sync::OwnedSemaphorePermit)>, Problem> {
    let memory_permit = profile_transfer_budget::acquire(reference.size, 2, correlation_id).await?;
    let root = root.to_path_buf();
    let cache_scope = cache_scope.to_owned();
    let reference = reference.clone();
    let legacy_alias = legacy_alias.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        let bytes = read_cached_manifest_object_bytes(
            &root,
            &cache_scope,
            &reference,
            legacy_alias.as_deref(),
        )?;
        Ok::<_, Problem>(bytes.map(|bytes| (bytes, memory_permit)))
    })
    .await
    .map_err(|_| unavailable())?
}

async fn cache_downloaded_manifest_object(
    root: PathBuf,
    cache_scope: String,
    key: String,
    bytes: Vec<u8>,
    memory_permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<(), Problem> {
    tokio::task::spawn_blocking(move || {
        let _permit = memory_permit;
        stage_snapshot_metadata(&root, &cache_scope, &key, &bytes).map(|_| ())
    })
    .await
    .map_err(|_| unavailable())?
}

fn read_cached_manifest_object_bytes(
    root: &Path,
    cache_scope: &str,
    reference: &ManifestObjectReference,
    legacy_alias: Option<&str>,
) -> Result<Option<Vec<u8>>, Problem> {
    let cache_name = format!(
        "object-{}-{}.bin",
        checksum(reference.key.as_bytes()),
        reference.checksum
    );
    let mut relative_paths = vec![PathBuf::from(format!(
        ".sync-cache/{cache_scope}/{cache_name}"
    ))];
    if let Some(alias) = legacy_alias {
        relative_paths.push(PathBuf::from(format!(".sync-cache/{cache_scope}/{alias}")));
    }
    for relative_path in relative_paths {
        let path = confined_path(root, &relative_path)?;
        let Some(bytes) = read_optional_snapshot_cache(&path, reference.size)? else {
            continue;
        };
        if bytes.len() as u64 == reference.size && checksum(&bytes) == reference.checksum {
            return Ok(Some(bytes));
        }
    }
    Ok(None)
}

fn proxy_credential_keys(
    manifest: &SyncManifest,
) -> Vec<(buglogin_domain::ids::ProxyId, u64, ManifestObjectReference)> {
    manifest
        .dependencies
        .iter()
        .filter_map(|dependency| match dependency {
            ManifestDependency::Proxy {
                proxy_id,
                revision,
                credential_envelope_object: Some(reference),
                ..
            } => Some((proxy_id.clone(), *revision, reference.clone())),
            ManifestDependency::Proxy {
                credential_envelope_object: None,
                ..
            } => None,
        })
        .collect()
}

fn should_download_manifest_object(
    path: &str,
    clear_metadata: &std::collections::BTreeSet<String>,
    proxy_envelopes: &[(buglogin_domain::ids::ProxyId, u64, ManifestObjectReference)],
) -> bool {
    path == RESOURCE_KEY_ENVELOPE_KEY
        || clear_metadata.contains(path)
        || proxy_envelopes
            .iter()
            .any(|(_, _, reference)| reference.key == path)
}

fn profile_object_aad(
    workspace_id: &buglogin_domain::ids::WorkspaceId,
    resource_id: &buglogin_domain::ids::ResourceId,
    generation: u64,
    path: &str,
) -> Vec<u8> {
    format!(
        "buglogin/profile-object/v1/{}/{}/{}/{}",
        workspace_id.as_str(),
        resource_id.as_str(),
        generation,
        path
    )
    .into_bytes()
}

fn profile_object_aad_v2(
    workspace_id: &buglogin_domain::ids::WorkspaceId,
    resource_id: &buglogin_domain::ids::ResourceId,
    path: &str,
) -> Vec<u8> {
    format!(
        "buglogin/profile-object/v2/{}/{}/{}",
        workspace_id.as_str(),
        resource_id.as_str(),
        path
    )
    .into_bytes()
}

fn profile_file_chunk_aad(
    workspace_id: &buglogin_domain::ids::WorkspaceId,
    resource_id: &buglogin_domain::ids::ResourceId,
    path: &str,
    index: u64,
    plaintext_checksum: &str,
) -> Vec<u8> {
    format!(
        "buglogin/profile-file-chunk/v1/{}/{}/{}/{index:016x}/{plaintext_checksum}",
        workspace_id.as_str(),
        resource_id.as_str(),
        path
    )
    .into_bytes()
}

fn descriptor_from_manifest(manifest: &SyncManifest) -> Result<PayloadDescriptor, Problem> {
    let descriptor = PayloadDescriptor {
        schema_version: PAYLOAD_SCHEMA_VERSION,
        workspace_id: manifest.workspace_id.clone(),
        resource_id: manifest.resource_id.clone(),
        generation: manifest.generation,
        root_checksum: manifest.root_checksum.clone(),
        objects: manifest
            .objects
            .iter()
            .map(|object| PayloadObject {
                path: object.key.clone(),
                size: object.size,
                checksum: object.checksum.clone(),
            })
            .collect(),
        file_chunks: manifest.file_chunks.clone(),
    };
    descriptor.verify_manifest(manifest)?;
    Ok(descriptor)
}

fn verify_generation(
    profile_id: &ProfileId,
    descriptor: &PayloadDescriptor,
    generation_path: &Path,
) -> Result<ProjectionReceipt, Problem> {
    let marker_path = generation_path.join(MARKER);
    let marker_bytes = read_projection_marker(&marker_path, "profile_projection_marker_invalid")?
        .ok_or_else(|| {
        problem(
            "profile_projection_marker_missing",
            "Profile projection marker is missing",
        )
    })?;
    let marker: ProjectionMarker = serde_json::from_slice(&marker_bytes).map_err(|_| {
        problem(
            "profile_projection_marker_invalid",
            "Profile projection marker is invalid",
        )
    })?;
    if marker.state != "complete"
        || &marker.profile_id != profile_id
        || marker.generation != descriptor.generation
        || marker.root_checksum != descriptor.root_checksum
    {
        return Err(problem(
            "profile_projection_marker_invalid",
            "Profile projection marker is invalid",
        ));
    }
    for object in &descriptor.objects {
        let bytes = read_verified(generation_path, Path::new(&object.path), &object.checksum)?;
        object.verify(&bytes)?;
    }
    Ok(ProjectionReceipt {
        profile_id: profile_id.clone(),
        generation: descriptor.generation,
        root_checksum: descriptor.root_checksum.clone(),
        path: generation_path.to_path_buf(),
    })
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn is_lock_contention(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    if error.raw_os_error() == Some(33) {
        return true;
    }
    false
}

#[cfg(test)]
fn collect_files(
    root: &Path,
    current: &Path,
    output: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), Problem> {
    let entries = fs::read_dir(current).map_err(profile_source_io_problem)?;
    for entry in entries {
        let entry = entry.map_err(profile_source_io_problem)?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(profile_source_io_problem)?;
        if is_chromium_lock_link(&metadata, &entry.file_name()) {
            continue;
        }
        if metadata.file_type().is_symlink() {
            return Err(problem(
                "profile_projection_symlink_rejected",
                "Profile projection symlink is rejected",
            ));
        }
        if metadata.is_dir() {
            let relative = path.strip_prefix(root).map_err(|_| {
                projection_state_problem(
                    "profile_projection_path_invalid",
                    "Profile source path escaped its root",
                    422,
                )
            })?;
            if is_ephemeral_profile_directory(relative) {
                continue;
            }
            collect_files(root, &path, output)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(problem(
                "profile_projection_entry_invalid",
                "Profile projection entry is invalid",
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| {
                projection_state_problem(
                    "profile_projection_path_invalid",
                    "Profile source path escaped its root",
                    422,
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        if is_ephemeral_profile_file(&relative) {
            continue;
        }
        validate_profile_file_size(metadata.len())?;
        output.push((relative, read_profile_file(&path, metadata.len())?));
    }
    Ok(())
}

fn collect_file_paths(
    root: &Path,
    current: &Path,
    output: &mut Vec<(String, PathBuf, u64)>,
) -> Result<(), Problem> {
    visit_profile_file_paths(root, current, &mut |relative, path, size| {
        output.push((relative, path, size));
        Ok(())
    })
}

fn visit_profile_file_paths<F>(root: &Path, current: &Path, visitor: &mut F) -> Result<(), Problem>
where
    F: FnMut(String, PathBuf, u64) -> Result<(), Problem>,
{
    let entries = fs::read_dir(current).map_err(profile_source_io_problem)?;
    for entry in entries {
        let entry = entry.map_err(profile_source_io_problem)?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(profile_source_io_problem)?;
        if is_chromium_lock_link(&metadata, &entry.file_name()) {
            continue;
        }
        if metadata.file_type().is_symlink() {
            return Err(problem(
                "profile_projection_symlink_rejected",
                "Profile projection symlink is rejected",
            ));
        }
        if metadata.is_dir() {
            let relative = path.strip_prefix(root).map_err(|_| {
                projection_state_problem(
                    "profile_projection_path_invalid",
                    "Profile source path escaped its root",
                    422,
                )
            })?;
            if is_ephemeral_profile_directory(relative) {
                continue;
            }
            visit_profile_file_paths(root, &path, visitor)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(problem(
                "profile_projection_entry_invalid",
                "Profile projection entry is invalid",
            ));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| {
                projection_state_problem(
                    "profile_projection_path_invalid",
                    "Profile source path escaped its root",
                    422,
                )
            })?
            .to_string_lossy()
            .replace('\\', "/");
        if is_ephemeral_profile_file(&relative) {
            continue;
        }
        validate_profile_file_size(metadata.len())?;
        visitor(relative, path, metadata.len())?;
    }
    Ok(())
}

fn is_ephemeral_profile_directory(path: &Path) -> bool {
    path.components().any(|component| {
        component.as_os_str().to_str().is_some_and(|name| {
            name.eq_ignore_ascii_case("GPUCache") || is_ephemeral_profile_path(name)
        })
    })
}

const CHROMIUM_LOCK_LINKS: [&str; 4] = [
    "SingletonLock",
    "SingletonSocket",
    "SingletonCookie",
    "RunningChromeVersion",
];

/// Chromium leaves these lock links behind when it is stopped or killed on
/// macOS/Linux; they are machine-local process state, never profile data.
pub(crate) fn is_chromium_lock_link(metadata: &fs::Metadata, name: &std::ffi::OsStr) -> bool {
    metadata.file_type().is_symlink()
        && name
            .to_str()
            .is_some_and(|name| CHROMIUM_LOCK_LINKS.contains(&name))
}

fn is_projection_control_file(path: &str) -> bool {
    !path.contains('/') && path.starts_with(".buglogin-")
}

fn is_ephemeral_profile_file(path: &str) -> bool {
    is_projection_control_file(path)
        || path
            .rsplit('/')
            .next()
            .is_some_and(|name| name.to_ascii_lowercase().starts_with("browsermetrics"))
}

fn cleanup_abandoned_staging(root: &Path) -> Result<(), Problem> {
    let entries = fs::read_dir(root).map_err(projection_cleanup_io_problem)?;
    for entry in entries {
        let entry = entry.map_err(projection_cleanup_io_problem)?;
        let profile_root = entry.path();
        let file_type = entry.file_type().map_err(projection_cleanup_io_problem)?;
        if !file_type.is_dir()
            || file_type.is_symlink()
            || ProfileId::new(entry.file_name().to_string_lossy().as_ref()).is_err()
        {
            continue;
        }
        let staging_path = profile_root.join("staging");
        let staging_type = match fs::symlink_metadata(&staging_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(projection_cleanup_io_problem(error)),
        };
        if staging_type.is_dir() && !staging_type.file_type().is_symlink() {
            fs::remove_dir_all(staging_path).map_err(projection_cleanup_io_problem)?;
        }
    }
    Ok(())
}

fn projection_cleanup_io_problem(error: std::io::Error) -> Problem {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => Problem::new(
            "profile_staging_not_removable",
            "Abandoned temporary profile staging cannot be removed; check local permissions",
            500,
            "desktop-projection",
        ),
        std::io::ErrorKind::StorageFull => Problem::new(
            "device_storage_full",
            "Device storage is full",
            507,
            "desktop-projection",
        ),
        _ => unavailable(),
    }
}

fn projection_staging_io_problem(error: std::io::Error) -> Problem {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => Problem::new(
            "profile_projection_not_writable",
            "Local profile projection data cannot be written; check local permissions",
            500,
            "desktop-projection",
        ),
        std::io::ErrorKind::StorageFull => Problem::new(
            "device_storage_full",
            "Device storage is full",
            507,
            "desktop-projection",
        ),
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput => Problem::new(
            "profile_projection_write_failed",
            "Local profile projection storage rejected invalid data",
            422,
            "desktop-projection",
        ),
        _ => unavailable(),
    }
}

fn projection_root_io_problem(error: std::io::Error) -> Problem {
    match error.kind() {
        std::io::ErrorKind::PermissionDenied => Problem::new(
            "profile_projection_not_readable",
            "Local profile projection data cannot be read; check local permissions",
            500,
            "desktop-projection",
        ),
        std::io::ErrorKind::StorageFull => Problem::new(
            "device_storage_full",
            "Device storage is full",
            507,
            "desktop-projection",
        ),
        _ => unavailable(),
    }
}

fn stage_snapshot_metadata(
    root: &Path,
    cache_scope: &str,
    key: &str,
    bytes: &[u8],
) -> Result<SnapshotBlob, Problem> {
    let object_checksum = checksum(bytes);
    let cache_name = format!(
        "object-{}-{}.bin",
        checksum(key.as_bytes()),
        object_checksum
    );
    let relative_path = PathBuf::from(format!(".sync-cache/{cache_scope}/{cache_name}"));
    let path = confined_path(root, &relative_path).map_err(snapshot_write_problem)?;
    write_projection_verified(root, &relative_path, bytes, &object_checksum)?;
    Ok(SnapshotBlob {
        key: key.to_owned(),
        path,
        size: bytes.len() as u64,
        checksum: object_checksum,
    })
}

fn write_snapshot_cache(
    root: &Path,
    cache_alias: &Path,
    bytes: &[u8],
    refresh_alias: bool,
) -> Result<PathBuf, Problem> {
    let object_checksum = checksum(bytes);
    if refresh_alias {
        write_projection_verified(root, cache_alias, bytes, &object_checksum)?;
    }
    let identity = cache_alias
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            Problem::new(
                "profile_snapshot_cache_path_invalid",
                "Profile snapshot cache path is invalid",
                422,
                "desktop-projection",
            )
        })?;
    let immutable_path = cache_alias.with_file_name(format!(
        "{}-{}.bin",
        identity.trim_end_matches(".bin"),
        object_checksum
    ));
    let path = confined_path(root, &immutable_path).map_err(snapshot_write_problem)?;
    match fs::symlink_metadata(&path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() == bytes.len() as u64 =>
        {
            if read_optional_snapshot_cache(&path, bytes.len() as u64)?
                .is_some_and(|existing| checksum(&existing) == object_checksum)
            {
                return Ok(path);
            }
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(snapshot_write_problem(snapshot_cache_io_problem(error))),
    }
    write_projection_verified(root, &immutable_path, bytes, &object_checksum)?;
    confined_path(root, &immutable_path).map_err(snapshot_write_problem)
}

fn read_optional_snapshot_cache(path: &Path, max_bytes: u64) -> Result<Option<Vec<u8>>, Problem> {
    match fs::symlink_metadata(path) {
        Ok(metadata)
            if metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= max_bytes =>
        {
            read_bounded_regular_file(path, max_bytes, snapshot_cache_read_problem)
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(snapshot_cache_read_problem(error)),
    }
}

fn stage_profile_file_chunk(
    root: &Path,
    cache_scope: &str,
    workspace_id: &buglogin_domain::ids::WorkspaceId,
    resource_id: &buglogin_domain::ids::ResourceId,
    path: &str,
    index: usize,
    plaintext: &[u8],
    client_key: &ClientKey,
    key_version: &str,
    previous_generation: u64,
    previous_file_chunks: &BTreeMap<&str, &ManifestFileChunks>,
    previous_objects: &BTreeMap<&str, &ManifestObject>,
) -> Result<(SnapshotBlob, ManifestFileChunk, Option<u64>), Problem> {
    let plaintext_checksum = checksum(plaintext);
    let aad = profile_file_chunk_aad(
        workspace_id,
        resource_id,
        path,
        index as u64,
        &plaintext_checksum,
    );
    let cache_identity = format!(
        "{}/{}/{}/{}/{}/{}",
        workspace_id.as_str(),
        resource_id.as_str(),
        key_version,
        path,
        index,
        plaintext_checksum
    );
    let cache_name = format!("{}.bin", checksum(cache_identity.as_bytes()));
    let cache_relative_path = PathBuf::from(format!(".sync-cache/{cache_scope}/{cache_name}"));
    let cache_path = confined_path(root, &cache_relative_path)?;
    let cached = if let Some(bytes) =
        read_optional_snapshot_cache(&cache_path, MAX_ENCRYPTED_OBJECT_BYTES as u64)?
    {
        (|| {
            let decoded = AuthenticatedChunk::decode(&bytes).ok()?;
            if decoded.decrypt(client_key, &aad).ok()?.as_slice() != plaintext {
                return None;
            }
            let prior_chunk = previous_file_chunks
                .get(path)
                .and_then(|file| file.chunks.get(index))
                .filter(|chunk| {
                    chunk.index == index as u64
                        && chunk.plaintext_size == plaintext.len() as u64
                        && chunk.plaintext_checksum == plaintext_checksum
                });
            let source_generation = prior_chunk
                .and_then(|chunk| previous_objects.get(chunk.object.key.as_str()).copied())
                .filter(|object| {
                    let expected_key = format!(
                        ".buglogin/file-chunks/{}/{:016x}-{}-{}",
                        checksum(path.as_bytes()),
                        index,
                        plaintext_checksum,
                        checksum(&bytes)
                    );
                    object.key == expected_key
                        && object.size == bytes.len() as u64
                        && object.checksum == checksum(&bytes)
                })
                .map(|object| object.source_generation.unwrap_or(previous_generation));
            Some((bytes, source_generation))
        })()
    } else {
        None
    };
    let (encrypted, source_generation, cache_path) = match cached {
        Some((bytes, source_generation)) => {
            let cache_path = write_snapshot_cache(root, &cache_relative_path, &bytes, false)?;
            (bytes, source_generation, cache_path)
        }
        None => {
            let bytes = AuthenticatedChunk::encrypt(client_key, plaintext, &aad)?.encoded();
            let cache_path = write_snapshot_cache(root, &cache_relative_path, &bytes, true)?;
            (bytes, None, cache_path)
        }
    };
    let ciphertext_checksum = checksum(&encrypted);
    let object_key = format!(
        ".buglogin/file-chunks/{}/{:016x}-{}-{}",
        checksum(path.as_bytes()),
        index,
        plaintext_checksum,
        ciphertext_checksum
    );
    let object = ManifestObject::new(object_key.clone(), &encrypted)?;
    Ok((
        SnapshotBlob {
            key: object_key,
            path: cache_path,
            size: object.size,
            checksum: object.checksum.clone(),
        },
        ManifestFileChunk {
            index: index as u64,
            plaintext_size: plaintext.len() as u64,
            plaintext_checksum,
            object: ManifestObjectReference::from(&object),
        },
        source_generation,
    ))
}

fn profile_changed_during_snapshot() -> Problem {
    Problem::new(
        "profile_changed_during_snapshot",
        "Profile files changed while the sync snapshot was being prepared",
        409,
        "desktop-projection",
    )
    .retryable(true)
}

fn snapshot_write_problem(error: Problem) -> Problem {
    match error.code.as_str() {
        "blob_write_failed" | "storage_root_unavailable" => error.retryable(true),
        _ => error,
    }
}

fn snapshot_cache_io_problem(error: std::io::Error) -> Problem {
    match error.kind() {
        std::io::ErrorKind::StorageFull => Problem::new(
            "device_storage_full",
            "Device storage is full",
            507,
            "storage",
        ),
        std::io::ErrorKind::PermissionDenied => Problem::new(
            "blob_storage_not_writable",
            "Profile storage is not writable",
            500,
            "storage",
        ),
        _ => Problem::new("blob_write_failed", "Blob write failed", 500, "storage"),
    }
}

fn write_projection_verified(
    root: &Path,
    relative_path: &Path,
    bytes: &[u8],
    expected_checksum: &str,
) -> Result<(), Problem> {
    write_verified(root, relative_path, bytes, expected_checksum).map_err(snapshot_write_problem)
}

fn unavailable() -> Problem {
    problem(
        "profile_projection_unavailable",
        "Profile projection is unavailable",
    )
    .retryable(true)
}

fn profile_source_io_problem(error: std::io::Error) -> Problem {
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::UnexpectedEof => {
            profile_changed_during_snapshot()
        }
        std::io::ErrorKind::PermissionDenied => Problem::new(
            "profile_source_not_readable",
            "A local profile file cannot be read; check its permissions",
            500,
            "desktop-projection",
        ),
        std::io::ErrorKind::StorageFull => Problem::new(
            "device_storage_full",
            "Device storage is full",
            507,
            "desktop-projection",
        ),
        std::io::ErrorKind::InvalidData | std::io::ErrorKind::InvalidInput => Problem::new(
            "profile_source_read_failed",
            "A local profile file contains invalid data or cannot be read",
            422,
            "desktop-projection",
        ),
        _ => unavailable(),
    }
}

fn validate_profile_source_root(profile_path: &Path) -> Result<(), Problem> {
    match fs::symlink_metadata(profile_path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(problem(
            "profile_projection_entry_invalid",
            "Profile source root must be a regular directory",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(Problem::new(
            "profile_source_missing",
            "The local profile source directory is missing; restore it before syncing",
            409,
            "desktop-projection",
        )),
        Err(error) => Err(profile_source_io_problem(error)),
    }
}

fn validate_profile_file_size(size: u64) -> Result<(), Problem> {
    if size > MAX_CHUNKED_PROFILE_FILE_BYTES {
        return Err(Problem::new(
            "profile_object_too_large",
            "A profile file exceeds the chunked sync file limit",
            413,
            "desktop-projection",
        ));
    }
    Ok(())
}

#[cfg(test)]
fn read_profile_file(path: &Path, expected_size: u64) -> Result<Vec<u8>, Problem> {
    let path_metadata_before = fs::symlink_metadata(path).map_err(profile_source_io_problem)?;
    let mut file = fs::File::open(path).map_err(profile_source_io_problem)?;
    let metadata_before = file.metadata().map_err(profile_source_io_problem)?;
    validate_profile_file_size(metadata_before.len())?;
    verify_snapshot_file_metadata(&path_metadata_before, &metadata_before, expected_size)?;
    let mut bytes = Vec::new();
    (&mut file)
        .take(expected_size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(profile_source_io_problem)?;
    if bytes.len() as u64 != expected_size {
        return Err(profile_changed_during_snapshot());
    }
    let metadata_after = file.metadata().map_err(profile_source_io_problem)?;
    verify_snapshot_file_metadata(&metadata_before, &metadata_after, expected_size)?;
    let path_metadata_after = fs::symlink_metadata(path).map_err(profile_source_io_problem)?;
    verify_snapshot_file_metadata(&path_metadata_before, &path_metadata_after, expected_size)?;
    Ok(bytes)
}

fn read_profile_object(path: &Path, expected_size: u64) -> Result<Vec<u8>, Problem> {
    let path_metadata_before = fs::symlink_metadata(path).map_err(profile_source_io_problem)?;
    let mut file = fs::File::open(path).map_err(profile_source_io_problem)?;
    let metadata_before = file.metadata().map_err(profile_source_io_problem)?;
    verify_snapshot_file_metadata(&path_metadata_before, &metadata_before, expected_size)?;
    let size = metadata_before.len();
    if size != expected_size || size > MAX_PROFILE_PLAINTEXT_BYTES {
        return Err(profile_changed_during_snapshot());
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(expected_size.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(profile_source_io_problem)?;
    if bytes.len() as u64 != expected_size || bytes.len() as u64 > MAX_PROFILE_PLAINTEXT_BYTES {
        return Err(profile_changed_during_snapshot());
    }
    let metadata_after = file.metadata().map_err(profile_source_io_problem)?;
    verify_snapshot_file_metadata(&metadata_before, &metadata_after, expected_size)?;
    let path_metadata_after = fs::symlink_metadata(path).map_err(profile_source_io_problem)?;
    verify_snapshot_file_metadata(&path_metadata_before, &path_metadata_after, expected_size)?;
    Ok(bytes)
}

fn verify_snapshot_file_metadata(
    before: &fs::Metadata,
    after: &fs::Metadata,
    expected_size: u64,
) -> Result<(), Problem> {
    if !before.is_file()
        || before.file_type().is_symlink()
        || !after.is_file()
        || after.file_type().is_symlink()
    {
        return Err(problem(
            "profile_projection_entry_invalid",
            "Profile source changed to a non-regular file during snapshot",
        ));
    }
    let modified_changed = matches!(
        (before.modified(), after.modified()),
        (Ok(before), Ok(after)) if before != after
    );
    if before.len() != expected_size || after.len() != expected_size || modified_changed {
        return Err(profile_changed_during_snapshot());
    }
    Ok(())
}

fn problem(code: &'static str, title: &'static str) -> Problem {
    let status = if matches!(
        code,
        "profile_local_sync_conflict"
            | "profile_local_sync_recovery_required"
            | "profile_local_changes_recovery_required"
            | "profile_sync_commit_stale"
            | "profile_hydration_stale"
            | "profile_session_already_active"
            | "sync_key_recovery_required"
            | "profile_key_unavailable"
    ) {
        409
    } else if matches!(
        code,
        "profile_projection_marker_missing"
            | "profile_projection_marker_invalid"
            | "profile_projection_file_invalid"
            | "profile_projection_incomplete"
            | "profile_projection_entry_invalid"
            | "profile_projection_symlink_rejected"
            | "profile_local_generation_missing"
            | "profile_sync_commit_marker_invalid"
            | "profile_session_marker_invalid"
            | "profile_snapshot_blob_corrupt"
            | "profile_payload_encryption_required"
            | "resource_key_envelope_missing"
            | "resource_key_envelope_invalid"
            | "resource_key_envelope_mismatch"
            | "profile_payload_key_version_mismatch"
            | "profile_object_checksum_mismatch"
            | "profile_file_chunks_invalid"
            | "profile_file_chunk_checksum_mismatch"
            | "profile_file_checksum_mismatch"
            | "profile_file_size_invalid"
            | "profile_object_too_large"
            | "proxy_credential_envelope_mismatch"
            | "proxy_credential_envelope_invalid"
            | "proxy_dependency_missing"
    ) {
        422
    } else {
        503
    };
    Problem::new(code, title, status, "desktop-projection")
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use super::*;

    #[test]
    fn projection_retry_classification_separates_transient_io_from_state_conflicts() {
        let changed = profile_changed_during_snapshot();
        let unavailable = unavailable();
        let conflict = problem("profile_local_sync_conflict", "Conflict");
        let corrupt = problem("profile_projection_file_invalid", "Corrupt");
        let storage_full = Problem::new("device_storage_full", "Full", 507, "test");
        let storage_denied = Problem::new("blob_storage_not_writable", "Denied", 500, "test");
        let transient_write = snapshot_write_problem(Problem::new(
            "blob_write_failed",
            "Temporary write failure",
            500,
            "test",
        ));

        assert_eq!(changed.status, 409);
        assert!(changed.retryable);
        assert_eq!(unavailable.status, 503);
        assert!(unavailable.retryable);
        assert_eq!(conflict.status, 409);
        assert!(!conflict.retryable);
        assert_eq!(corrupt.status, 422);
        assert!(!corrupt.retryable);
        assert_eq!(snapshot_write_problem(storage_full.clone()).status, 507);
        assert!(!snapshot_write_problem(storage_full).retryable);
        assert!(!snapshot_write_problem(storage_denied).retryable);
        assert!(transient_write.retryable);

        let cleanup_denied = projection_cleanup_io_problem(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        ));
        assert_eq!(cleanup_denied.code, "profile_staging_not_removable");
        assert!(!cleanup_denied.retryable);
        let cleanup_timeout =
            projection_cleanup_io_problem(std::io::Error::from(std::io::ErrorKind::TimedOut));
        assert!(cleanup_timeout.retryable);

        let staging_denied = projection_staging_io_problem(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        ));
        assert_eq!(staging_denied.code, "profile_projection_not_writable");
        assert!(!staging_denied.retryable);
        let staging_full =
            projection_staging_io_problem(std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert_eq!(staging_full.status, 507);
        assert!(!staging_full.retryable);
        let root_denied =
            projection_root_io_problem(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(root_denied.code, "profile_projection_not_readable");
        assert!(!root_denied.retryable);
        let root_full =
            projection_root_io_problem(std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert_eq!(root_full.status, 507);
        assert!(!root_full.retryable);
        assert!(
            projection_root_io_problem(std::io::Error::from(std::io::ErrorKind::TimedOut))
                .retryable
        );
        assert!(
            projection_staging_io_problem(std::io::Error::from(std::io::ErrorKind::TimedOut))
                .retryable
        );
        let lock_contention = std::io::Error::from(std::io::ErrorKind::WouldBlock);
        let lock_result = if is_lock_contention(&lock_contention) {
            Problem::new(
                "profile_projection_in_use",
                "Profile data is already open by another BugLogin process",
                409,
                "desktop-projection",
            )
        } else {
            projection_staging_io_problem(lock_contention)
        };
        assert_eq!(lock_result.status, 409);
        assert!(!lock_result.retryable);

        let marker_denied = projection_marker_io_problem(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        ));
        assert_eq!(marker_denied.code, "profile_marker_not_readable");
        assert!(!marker_denied.retryable);
        let marker_remove_denied = projection_marker_io_problem(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        ));
        assert_eq!(marker_remove_denied.status, 500);
        assert!(!marker_remove_denied.retryable);
        let marker_remove_full =
            projection_marker_io_problem(std::io::Error::from(std::io::ErrorKind::StorageFull));
        assert_eq!(marker_remove_full.status, 507);
        assert!(!marker_remove_full.retryable);
        assert!(
            projection_marker_io_problem(std::io::Error::from(std::io::ErrorKind::TimedOut))
                .retryable
        );
        let temp = std::env::temp_dir().join(format!(
            "buglogin-projection-corrupt-snapshot-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::write(&temp, b"bad staged bytes").unwrap();
        let staged_corruption = SnapshotBlob {
            key: "Preferences".to_owned(),
            path: temp.clone(),
            size: 1,
            checksum: checksum(b"expected"),
        }
        .read_bytes()
        .unwrap_err();
        assert_eq!(staged_corruption.status, 503);
        assert!(staged_corruption.retryable);
        fs::remove_file(temp).unwrap();
    }

    #[test]
    fn snapshot_source_io_retries_only_when_the_source_can_reappear_or_io_is_transient() {
        let missing = profile_source_io_problem(std::io::Error::from(std::io::ErrorKind::NotFound));
        let denied =
            profile_source_io_problem(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        let transient =
            profile_source_io_problem(std::io::Error::from(std::io::ErrorKind::TimedOut));
        let truncated =
            profile_source_io_problem(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));

        assert_eq!(missing.code, "profile_changed_during_snapshot");
        assert!(missing.retryable);
        assert_eq!(denied.code, "profile_source_not_readable");
        assert_eq!(denied.status, 500);
        assert!(!denied.retryable);
        assert!(transient.retryable);
        assert_eq!(truncated.code, "profile_changed_during_snapshot");
        assert!(truncated.retryable);
    }

    #[test]
    fn snapshot_source_size_change_between_inventory_and_read_is_retryable() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-source-size-change-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("Preferences");
        fs::write(&source, b"changed").unwrap();

        let error = read_profile_file(&source, b"original".len() as u64).unwrap_err();

        assert_eq!(error.code, "profile_changed_during_snapshot");
        assert!(error.retryable);
        assert_eq!(fs::read(&source).unwrap(), b"changed");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_cache_permissions_are_not_retryable_but_missing_cache_is_rebuildable() {
        let missing =
            snapshot_cache_read_problem(std::io::Error::from(std::io::ErrorKind::NotFound));
        let denied =
            snapshot_cache_read_problem(std::io::Error::from(std::io::ErrorKind::PermissionDenied));

        assert_eq!(missing.code, "profile_snapshot_blob_corrupt");
        assert!(missing.retryable);
        assert_eq!(denied.code, "blob_storage_not_readable");
        assert_eq!(denied.status, 500);
        assert!(!denied.retryable);
    }

    #[test]
    fn chunk_cache_reuse_requires_the_manifest_object_key_to_match_content() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-chunk-key-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let workspace = WorkspaceId::new("ws_chunk_key").unwrap();
        let resource = ResourceId::new("res_chunk_key").unwrap();
        let path = "Default/Cookies";
        let index = 0usize;
        let plaintext = b"authenticated cached chunk";
        let client_key = ClientKey::from_bytes([0x71; 32]);
        let key_version = checksum(client_key.as_bytes());
        let plaintext_checksum = checksum(plaintext);
        let aad = profile_file_chunk_aad(
            &workspace,
            &resource,
            path,
            index as u64,
            &plaintext_checksum,
        );
        let encrypted = AuthenticatedChunk::encrypt(&client_key, plaintext, &aad)
            .unwrap()
            .encoded();
        let cache_scope = "test-cache-scope";
        let cache_identity = format!(
            "{}/{}/{}/{}/{}/{}",
            workspace.as_str(),
            resource.as_str(),
            key_version,
            path,
            index,
            plaintext_checksum
        );
        let cache_name = format!("{}.bin", checksum(cache_identity.as_bytes()));
        let cache_path = root.join(format!(".sync-cache/{cache_scope}/{cache_name}"));
        fs::create_dir_all(cache_path.parent().unwrap()).unwrap();
        fs::write(&cache_path, &encrypted).unwrap();

        let arbitrary_key = ".buglogin/file-chunks/unrelated/forged-key";
        let previous_object = ManifestObject::new(arbitrary_key, &encrypted).unwrap();
        let previous_objects = BTreeMap::from([(arbitrary_key, &previous_object)]);
        let previous_chunk = ManifestFileChunk {
            index: index as u64,
            plaintext_size: plaintext.len() as u64,
            plaintext_checksum,
            object: ManifestObjectReference::from(&previous_object),
        };
        let previous_file = ManifestFileChunks {
            path: path.to_owned(),
            plaintext_size: plaintext.len() as u64,
            plaintext_checksum: checksum(plaintext),
            chunks: vec![previous_chunk],
        };
        let previous_file_chunks = BTreeMap::from([(path, &previous_file)]);

        let (_, _, source_generation) = stage_profile_file_chunk(
            &root,
            cache_scope,
            &workspace,
            &resource,
            path,
            index,
            plaintext,
            &client_key,
            &key_version,
            7,
            &previous_file_chunks,
            &previous_objects,
        )
        .unwrap();

        assert_eq!(source_generation, None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn generation_file_permissions_are_not_misreported_as_dirty_data() {
        let denied = projection_generation_io_problem(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        ));
        let missing =
            projection_generation_io_problem(std::io::Error::from(std::io::ErrorKind::NotFound));

        assert_eq!(denied.code, "profile_source_not_readable");
        assert!(!denied.retryable);
        assert_eq!(missing.code, "profile_projection_file_invalid");
        assert!(!missing.retryable);
    }

    #[test]
    fn bounded_snapshot_cache_reader_rejects_oversize_before_allocation() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-bounded-cache-reader-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("object.bin");
        fs::write(&path, vec![b'x'; 33]).unwrap();

        let result = read_optional_snapshot_cache(&path, 32).unwrap();

        assert!(result.is_none());
        assert_eq!(fs::metadata(&path).unwrap().len(), 33);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cached_manifest_object_requires_exact_manifest_checksum_for_aliases() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-manifest-cache-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let scope = "profile-scope";
        let alias = "proxy-envelope.json";
        let cache_dir = root.join(".sync-cache").join(scope);
        fs::create_dir_all(&cache_dir).unwrap();
        let expected = b"manifest-bound cached envelope";
        fs::write(cache_dir.join(alias), expected).unwrap();
        let reference = ManifestObjectReference {
            key: ".buglogin/proxy-dependencies/proxy/1/credential-envelope.json".to_owned(),
            size: expected.len() as u64,
            checksum: checksum(expected),
        };
        assert_eq!(
            read_cached_manifest_object_bytes(&root, scope, &reference, Some(alias)).unwrap(),
            Some(expected.to_vec())
        );
        let wrong_reference = ManifestObjectReference {
            checksum: checksum(b"manifest-bound cached envelopf"),
            ..reference
        };
        assert_eq!(
            read_cached_manifest_object_bytes(&root, scope, &wrong_reference, Some(alias)).unwrap(),
            None
        );
        let produced = b"verified remote legacy metadata";
        let produced_reference = ManifestObjectReference {
            key: ".buglogin/proxy-dependencies/proxy/1/metadata.json".to_owned(),
            size: produced.len() as u64,
            checksum: checksum(produced),
        };
        stage_snapshot_metadata(&root, scope, &produced_reference.key, produced).unwrap();
        assert_eq!(
            read_cached_manifest_object_bytes(&root, scope, &produced_reference, None).unwrap(),
            Some(produced.to_vec())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_blob_rejects_oversized_local_cache_before_reading() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-oversized-snapshot-cache-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("oversized.bin");
        fs::File::create(&path)
            .unwrap()
            .set_len(MAX_ENCRYPTED_OBJECT_BYTES as u64 + 1)
            .unwrap();
        let blob = SnapshotBlob {
            key: "Preferences".to_owned(),
            path,
            size: MAX_ENCRYPTED_OBJECT_BYTES as u64 + 1,
            checksum: String::new(),
        };

        let error = blob.read_bytes().unwrap_err();

        assert_eq!(error.code, "profile_snapshot_blob_corrupt");
        assert!(error.retryable);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn new_browser_file_marks_decrypted_generation_dirty_with_existing_profile() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-new-file-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_new_file").unwrap();
        let resource = ResourceId::new("res_new_file").unwrap();
        let profile = ProfileId::new("prf_new_file").unwrap();
        let manifest = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let receipt = projection
            .publish_decrypted(&profile, &manifest, Vec::new())
            .unwrap();
        assert!(
            projection
                .dirty_local_generation(&profile, &manifest)
                .unwrap()
                .is_none()
        );
        fs::create_dir_all(receipt.path.join("Default")).unwrap();
        fs::write(receipt.path.join("Default/Cookies"), b"new browser data").unwrap();
        assert_eq!(
            projection
                .dirty_local_generation(&profile, &manifest)
                .unwrap(),
            Some(receipt.path)
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_empty_genesis_manifest_is_hydratable() {
        let workspace = WorkspaceId::new("ws_legacy_empty_genesis").unwrap();
        let resource = ResourceId::new("res_legacy_empty_genesis").unwrap();
        let manifest = SyncManifest::new(workspace, resource, 1, 1, None, b"", Vec::new()).unwrap();

        let descriptor = descriptor_from_manifest(&manifest).unwrap();
        descriptor.verify_manifest(&manifest).unwrap();
    }

    #[test]
    fn snapshot_memory_charge_uses_peak_file_working_set() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-snapshot-memory-charge-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let profile_path = root.join("profile");
        fs::create_dir_all(&profile_path).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(profile_path.join("small-a"))
            .unwrap()
            .set_len(8 * 1024 * 1024)
            .unwrap();
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(profile_path.join("small-b"))
            .unwrap()
            .set_len(16 * 1024 * 1024)
            .unwrap();

        let projection = ProfileProjection::open(root.join("projection")).unwrap();
        assert_eq!(
            projection
                .snapshot_memory_charge_bytes(&profile_path)
                .unwrap(),
            48 * 1024 * 1024
        );
        fs::remove_file(profile_path.join("small-a")).unwrap();
        fs::remove_file(profile_path.join("small-b")).unwrap();

        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(profile_path.join("chunked"))
            .unwrap()
            .set_len(MAX_PROFILE_PLAINTEXT_BYTES + 1)
            .unwrap();
        assert_eq!(
            projection
                .snapshot_memory_charge_bytes(&profile_path)
                .unwrap(),
            12 * 1024 * 1024
        );

        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn profile_projection_skips_chromium_lock_links_left_after_a_stop() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-profile-lock-links-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(root.join("Default")).unwrap();
        fs::write(root.join("Default/Cookies"), b"session").unwrap();
        for name in [
            "SingletonLock",
            "SingletonSocket",
            "SingletonCookie",
            "RunningChromeVersion",
        ] {
            std::os::unix::fs::symlink("MacBook-Pro-12345", root.join(name)).unwrap();
        }

        let mut files = Vec::new();
        collect_file_paths(&root, &root, &mut files).unwrap();
        assert_eq!(
            files.iter().map(|file| file.0.as_str()).collect::<Vec<_>>(),
            vec!["Default/Cookies"]
        );
        let mut snapshot = Vec::new();
        collect_files(&root, &root, &mut snapshot).unwrap();
        assert_eq!(
            snapshot
                .iter()
                .map(|file| file.0.as_str())
                .collect::<Vec<_>>(),
            vec!["Default/Cookies"]
        );

        std::os::unix::fs::symlink("/etc/hosts", root.join("Default/Bookmarks")).unwrap();
        let mut rejected = Vec::new();
        assert_eq!(
            collect_file_paths(&root, &root, &mut rejected)
                .unwrap_err()
                .code,
            "profile_projection_symlink_rejected"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_projection_excludes_browser_cache_but_keeps_persistent_state() {
        for path in [
            "Default/Cache",
            "Default/Code Cache",
            "Default/GPUCache",
            "Default/Media Cache",
            "Crashpad/pending",
            "Default/Service Worker/ScriptCache",
            "Crash Reports",
        ] {
            assert!(is_ephemeral_profile_directory(Path::new(path)));
        }
        for path in [
            "Default/Cookies",
            "Default/Network",
            "Default/Local Storage",
            "Default/IndexedDB",
            "Default/Service Worker",
            "Default/Extensions",
        ] {
            assert!(!is_ephemeral_profile_directory(Path::new(path)));
        }

        let root = std::env::temp_dir().join(format!(
            "buglogin-profile-filter-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(root.join("Default/Cache/subdir")).unwrap();
        fs::create_dir_all(root.join("Default/Code Cache")).unwrap();
        fs::create_dir_all(root.join("Default/Local Storage")).unwrap();
        fs::write(root.join("Default/Cache/subdir/rebuildable.bin"), b"cache").unwrap();
        fs::write(root.join("Default/Code Cache/compiled.bin"), b"compiled").unwrap();
        fs::write(root.join("Default/Cookies"), b"session").unwrap();
        fs::write(root.join("Default/Local Storage/data.bin"), b"persistent").unwrap();

        let mut files = Vec::new();
        collect_file_paths(&root, &root, &mut files).unwrap();
        files.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            files.iter().map(|file| file.0.as_str()).collect::<Vec<_>>(),
            vec!["Default/Cookies", "Default/Local Storage/data.bin"]
        );

        let mut snapshot = Vec::new();
        collect_files(&root, &root, &mut snapshot).unwrap();
        snapshot.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            snapshot
                .iter()
                .map(|file| file.0.as_str())
                .collect::<Vec<_>>(),
            vec!["Default/Cookies", "Default/Local Storage/data.bin"]
        );
        fs::remove_dir_all(root).unwrap();
    }
    use buglogin_domain::ids::{ProxyId, ResourceId, WorkspaceId};
    use buglogin_sync::coordination::CoordinationAuth;
    use buglogin_sync::crypto::{AuthenticatedChunk, ClientKey, RecoveryKey};
    use buglogin_sync::manifest::{ManifestObject, ManifestObjectReference};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn recovery_manifest(
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
        generation: u64,
        parent_generation: Option<u64>,
        bytes: &[u8],
    ) -> SyncManifest {
        let object = PayloadObject::new("Default/Cookies", bytes).unwrap();
        let descriptor = PayloadDescriptor::new(
            workspace_id.clone(),
            resource_id.clone(),
            generation,
            vec![object.clone()],
        )
        .unwrap();
        SyncManifest::new(
            workspace_id.clone(),
            resource_id.clone(),
            generation,
            generation,
            parent_generation,
            &descriptor.canonical_root_bytes(),
            vec![ManifestObject::new(object.path, bytes).unwrap()],
        )
        .unwrap()
    }

    #[test]
    fn new_browser_file_marks_decrypted_generation_dirty() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-new-file-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_new_file").unwrap();
        let resource = ResourceId::new("res_new_file").unwrap();
        let profile = ProfileId::new("prf_new_file").unwrap();
        let manifest = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let receipt = projection
            .publish_decrypted(&profile, &manifest, Vec::new())
            .unwrap();
        assert!(
            projection
                .dirty_local_generation(&profile, &manifest)
                .unwrap()
                .is_none()
        );
        fs::create_dir_all(receipt.path.join("Default")).unwrap();
        fs::write(receipt.path.join("Default/Cookies"), b"new browser data").unwrap();
        assert_eq!(
            projection
                .dirty_local_generation(&profile, &manifest)
                .unwrap(),
            Some(receipt.path)
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn active_session_recovery_returns_modified_generation_without_deleting_it() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-active-recovery-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_active_recovery").unwrap();
        let resource = ResourceId::new("res_active_recovery").unwrap();
        let profile = ProfileId::new("prf_active_recovery").unwrap();
        let manifest = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &manifest,
                vec![("Default/Cookies".to_owned(), b"base".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(&profile, manifest.generation, &manifest.root_checksum)
            .unwrap();
        let active_marker_path = root.join(profile.as_str()).join(ACTIVE_SESSION_MARKER);
        let active_marker_before = fs::read(&active_marker_path).unwrap();
        let mut malformed_manifest = manifest.clone();
        malformed_manifest.root_checksum = "invalid".to_owned();
        let error = projection
            .active_session_recovery_path(&profile, &malformed_manifest)
            .unwrap_err();
        assert_eq!(error.code, "invalid_manifest_checksum");
        assert_eq!(fs::read(&active_marker_path).unwrap(), active_marker_before);
        assert_eq!(
            projection
                .dirty_local_generation(&profile, &malformed_manifest)
                .unwrap_err()
                .code,
            "invalid_manifest_checksum"
        );
        assert_eq!(
            projection
                .existing_local_generation(&profile, &malformed_manifest)
                .unwrap_err()
                .code,
            "invalid_manifest_checksum"
        );
        fs::write(receipt.path.join("Default/Cookies"), b"local dirty data").unwrap();

        let recovered = projection
            .active_session_recovery_path(&profile, &manifest)
            .unwrap()
            .expect("the interrupted session's local generation must be recovered");

        assert_eq!(recovered, receipt.path);
        assert_eq!(
            fs::read(recovered.join("Default/Cookies")).unwrap(),
            b"local dirty data"
        );
        assert!(recovered.join(MARKER).exists());

        let newer_remote = recovery_manifest(&workspace, &resource, 2, Some(1), b"newer");
        let conflicted_recovery = projection
            .active_session_recovery_path(&profile, &newer_remote)
            .unwrap()
            .expect("a safe local copy remains launchable after a remote update");
        assert_eq!(conflicted_recovery, receipt.path);
        assert!(
            projection
                .active_session_conflicts_with_manifest(&profile, &newer_remote)
                .unwrap()
        );
        assert_eq!(
            fs::read(recovered.join("Default/Cookies")).unwrap(),
            b"local dirty data"
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn active_session_conflict_rejects_invalid_manifest_and_commit_marker_metadata() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-conflict-marker-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_conflict_marker").unwrap();
        let resource = ResourceId::new("res_conflict_marker").unwrap();
        let profile = ProfileId::new("prf_conflict_marker").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        let active_path = root.join(profile.as_str()).join(ACTIVE_SESSION_MARKER);
        let malformed_active = ActiveSessionMarker {
            state: "active".to_owned(),
            profile_id: profile.clone(),
            generation: 0,
            root_checksum: base.root_checksum.clone(),
            session_epoch: None,
        };
        let malformed_active_bytes = serde_json::to_vec(&malformed_active).unwrap();
        fs::write(&active_path, &malformed_active_bytes).unwrap();
        let error = projection
            .active_session_conflicts_with_manifest(&profile, &base)
            .unwrap_err();
        assert_eq!(error.code, "profile_session_marker_invalid");
        let error = projection
            .prepare_sync_commit(
                &profile,
                &base,
                &recovery_manifest(&workspace, &resource, 2, Some(1), b"target"),
            )
            .unwrap_err();
        assert_eq!(error.code, "profile_session_marker_invalid");
        fs::write(
            &active_path,
            serde_json::to_vec(&ActiveSessionMarker {
                state: "active".to_owned(),
                profile_id: profile.clone(),
                generation: base.generation,
                root_checksum: base.root_checksum.clone(),
                session_epoch: None,
            })
            .unwrap(),
        )
        .unwrap();

        let mut invalid_manifest = base.clone();
        invalid_manifest.root_checksum = "invalid".to_owned();
        let error = projection
            .active_session_conflicts_with_manifest(&profile, &invalid_manifest)
            .unwrap_err();
        assert_eq!(error.code, "invalid_manifest_checksum");

        let commit_path = root.join(profile.as_str()).join(SYNC_COMMIT_MARKER);
        let mut invalid_commit = SyncCommitMarker {
            profile_id: profile.clone(),
            base_generation: base.generation,
            base_root_checksum: base.root_checksum.clone(),
            target_generation: base.generation + 2,
            target_root_checksum: base.root_checksum.clone(),
            session_epoch: None,
        };
        fs::write(&commit_path, serde_json::to_vec(&invalid_commit).unwrap()).unwrap();
        let mut invalid_manifest_with_commit = base.clone();
        invalid_manifest_with_commit.root_checksum = "invalid".to_owned();
        let error = projection
            .active_session_conflicts_with_manifest(&profile, &invalid_manifest_with_commit)
            .unwrap_err();
        assert_eq!(error.code, "invalid_manifest_checksum");

        let error = projection
            .active_session_conflicts_with_manifest(&profile, &base)
            .unwrap_err();
        assert_eq!(error.code, "profile_sync_commit_marker_invalid");
        assert!(!error.retryable);

        invalid_commit.target_generation = base.generation + 1;
        invalid_commit.target_root_checksum = "invalid".to_owned();
        fs::write(&commit_path, serde_json::to_vec(&invalid_commit).unwrap()).unwrap();
        let error = projection
            .active_session_conflicts_with_manifest(&profile, &base)
            .unwrap_err();
        assert_eq!(error.code, "profile_sync_commit_marker_invalid");
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn session_epoch_fences_stale_begin_commit_and_clear_with_idempotent_retry() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-session-epoch-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_session_epoch").unwrap();
        let resource = ResourceId::new("res_session_epoch").unwrap();
        let profile = ProfileId::new("prf_session_epoch").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");

        let invalid_generation = projection
            .begin_active_session_for_epoch(&profile, 0, &base.root_checksum, "session-a")
            .unwrap_err();
        assert_eq!(invalid_generation.code, "profile_session_marker_invalid");
        assert!(!root.join(profile.as_str()).exists());

        projection
            .begin_active_session_for_epoch(
                &profile,
                base.generation,
                &base.root_checksum,
                "session-a",
            )
            .unwrap();
        projection
            .begin_active_session_for_epoch(
                &profile,
                base.generation,
                &base.root_checksum,
                "session-a",
            )
            .unwrap();
        let stale_begin = projection
            .begin_active_session_for_epoch(
                &profile,
                target.generation,
                &target.root_checksum,
                "session-b",
            )
            .unwrap_err();
        assert_eq!(stale_begin.code, "profile_session_already_active");

        let stale_commit = projection
            .prepare_sync_commit_for_epoch(&profile, &base, &target, "session-b")
            .unwrap_err();
        assert_eq!(stale_commit.code, "profile_sync_commit_stale");
        projection
            .prepare_sync_commit_for_epoch(&profile, &base, &target, "session-a")
            .unwrap();
        projection
            .prepare_sync_commit_for_epoch(&profile, &base, &target, "session-a")
            .unwrap();

        let active_marker = root.join(profile.as_str()).join(ACTIVE_SESSION_MARKER);
        let active_bytes = fs::read(&active_marker).unwrap();
        let commit_marker = root.join(profile.as_str()).join(SYNC_COMMIT_MARKER);
        let valid_commit_bytes = fs::read(&commit_marker).unwrap();
        let mut stale_commit: SyncCommitMarker =
            serde_json::from_slice(&valid_commit_bytes).unwrap();
        stale_commit.session_epoch = Some("session-old".to_owned());
        let stale_commit_bytes = serde_json::to_vec(&stale_commit).unwrap();
        fs::write(&commit_marker, &stale_commit_bytes).unwrap();
        let stale_commit_clear = projection
            .clear_active_session_for_epoch(&profile, "session-a")
            .unwrap_err();
        assert_eq!(stale_commit_clear.code, "profile_sync_commit_stale");
        assert_eq!(fs::read(&active_marker).unwrap(), active_bytes);
        assert_eq!(fs::read(&commit_marker).unwrap(), stale_commit_bytes);
        let fence: HydrationFenceMarker = serde_json::from_slice(
            &fs::read(root.join(profile.as_str()).join(HYDRATION_FENCE_MARKER)).unwrap(),
        )
        .unwrap();
        assert_eq!(fence.generation, base.generation);
        fs::write(&commit_marker, &valid_commit_bytes).unwrap();

        let mut wrong_base: SyncCommitMarker =
            serde_json::from_slice(&fs::read(&commit_marker).unwrap()).unwrap();
        wrong_base.base_root_checksum = checksum(b"another session base");
        let wrong_base_bytes = serde_json::to_vec(&wrong_base).unwrap();
        fs::write(&commit_marker, &wrong_base_bytes).unwrap();
        let wrong_base_clear = projection
            .clear_active_session_for_epoch(&profile, "session-a")
            .unwrap_err();
        assert_eq!(wrong_base_clear.code, "profile_sync_commit_stale");
        assert_eq!(fs::read(&active_marker).unwrap(), active_bytes);
        assert_eq!(fs::read(&commit_marker).unwrap(), wrong_base_bytes);
        let fence: HydrationFenceMarker = serde_json::from_slice(
            &fs::read(root.join(profile.as_str()).join(HYDRATION_FENCE_MARKER)).unwrap(),
        )
        .unwrap();
        assert_eq!(fence.generation, base.generation);
        fs::write(&commit_marker, &valid_commit_bytes).unwrap();

        let mut wrong_base_generation: SyncCommitMarker =
            serde_json::from_slice(&valid_commit_bytes).unwrap();
        wrong_base_generation.base_generation += 1;
        wrong_base_generation.target_generation += 1;
        let wrong_base_generation_bytes = serde_json::to_vec(&wrong_base_generation).unwrap();
        fs::write(&commit_marker, &wrong_base_generation_bytes).unwrap();
        let wrong_base_generation_clear = projection
            .clear_active_session_for_epoch(&profile, "session-a")
            .unwrap_err();
        assert_eq!(
            wrong_base_generation_clear.code,
            "profile_sync_commit_stale"
        );
        assert_eq!(fs::read(&active_marker).unwrap(), active_bytes);
        assert_eq!(
            fs::read(&commit_marker).unwrap(),
            wrong_base_generation_bytes
        );
        let fence: HydrationFenceMarker = serde_json::from_slice(
            &fs::read(root.join(profile.as_str()).join(HYDRATION_FENCE_MARKER)).unwrap(),
        )
        .unwrap();
        assert_eq!(fence.generation, base.generation);
        fs::write(&commit_marker, &valid_commit_bytes).unwrap();

        let stale_clear = projection
            .clear_active_session_for_epoch(&profile, "session-b")
            .unwrap_err();
        assert_eq!(stale_clear.code, "profile_session_stale");
        projection
            .clear_active_session_for_epoch(&profile, "session-a")
            .unwrap();
        projection
            .begin_active_session_for_epoch(
                &profile,
                target.generation,
                &target.root_checksum,
                "session-b",
            )
            .unwrap();

        let mut malformed_state: serde_json::Value =
            serde_json::from_slice(&fs::read(&active_marker).unwrap()).unwrap();
        malformed_state["state"] = serde_json::Value::String("stale".to_owned());
        let malformed_bytes = serde_json::to_vec(&malformed_state).unwrap();
        fs::write(&active_marker, &malformed_bytes).unwrap();
        let malformed_clear = projection
            .clear_active_session_for_epoch(&profile, "session-b")
            .unwrap_err();
        assert_eq!(malformed_clear.code, "profile_session_marker_invalid");
        assert_eq!(fs::read(&active_marker).unwrap(), malformed_bytes);

        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_session_paths_survive_projection_restart() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-pending-restart-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let workspace = WorkspaceId::new("ws_pending_restart").unwrap();
        let resource = ResourceId::new("res_pending_restart").unwrap();
        let profile = ProfileId::new("prf_pending_restart").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        {
            let projection = ProfileProjection::open(&root).unwrap();
            let receipt = projection
                .publish_decrypted(
                    &profile,
                    &base,
                    vec![("Default/Cookies".to_owned(), b"base".to_vec())],
                )
                .unwrap();
            projection
                .begin_active_session(&profile, base.generation, &base.root_checksum)
                .unwrap();
            projection
                .prepare_sync_commit(&profile, &base, &target)
                .unwrap();
            projection
                .begin_active_session(&profile, base.generation, &base.root_checksum)
                .unwrap();
            fs::write(receipt.path.join("Default/Cookies"), b"local dirty data").unwrap();
        }

        let projection = ProfileProjection::open(&root).unwrap();
        assert_eq!(
            projection.pending_session_paths().unwrap(),
            vec![(
                profile.clone(),
                decrypted_generation_path(&root.canonicalize().unwrap(), &profile, &base).unwrap()
            )]
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_session_paths_reject_corrupt_commit_marker_without_falling_back() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-pending-invalid-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let profile = ProfileId::new("prf_pending_invalid").unwrap();
        let profile_root = root.join(profile.as_str());
        fs::create_dir_all(&profile_root).unwrap();
        fs::write(profile_root.join(SYNC_COMMIT_MARKER), b"{invalid").unwrap();

        let error = projection.pending_session_paths().unwrap_err();

        assert_eq!(error.code, "profile_sync_commit_marker_invalid");
        assert_eq!(error.status, 422);
        assert!(!error.retryable);
        assert_eq!(
            fs::read(profile_root.join(SYNC_COMMIT_MARKER)).unwrap(),
            b"{invalid"
        );

        let checksum = "a".repeat(64);
        let malformed_commit = SyncCommitMarker {
            profile_id: profile.clone(),
            base_generation: 1,
            base_root_checksum: checksum.clone(),
            target_generation: 3,
            target_root_checksum: checksum,
            session_epoch: None,
        };
        let malformed_bytes = serde_json::to_vec(&malformed_commit).unwrap();
        fs::write(profile_root.join(SYNC_COMMIT_MARKER), &malformed_bytes).unwrap();
        let error = projection.pending_session_paths().unwrap_err();
        assert_eq!(error.code, "profile_sync_commit_marker_invalid");
        assert_eq!(
            fs::read(profile_root.join(SYNC_COMMIT_MARKER)).unwrap(),
            malformed_bytes
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verified_local_recovery_receipt_preserves_pending_sync_data_for_launch() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-verified-recovery-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_verified_recovery").unwrap();
        let resource = ResourceId::new("res_verified_recovery").unwrap();
        let profile = ProfileId::new("prf_verified_recovery").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Cookies".to_owned(), b"base".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap();
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        fs::write(receipt.path.join("Default/Cookies"), b"local dirty data").unwrap();

        let recovery = projection
            .verified_local_recovery_receipt(&profile, &base)
            .unwrap()
            .expect("matching remote base permits local recovery launch");

        assert_eq!(
            recovery,
            ProjectionReceipt {
                profile_id: profile.clone(),
                generation: base.generation,
                root_checksum: base.root_checksum.clone(),
                path: receipt.path.clone(),
            }
        );
        assert_eq!(
            fs::read(recovery.path.join("Default/Cookies")).unwrap(),
            b"local dirty data"
        );
        assert!(
            root.join(profile.as_str())
                .join(SYNC_COMMIT_MARKER)
                .exists()
        );
        assert!(
            root.join(profile.as_str())
                .join(ACTIVE_SESSION_MARKER)
                .exists()
        );

        let newer_remote = recovery_manifest(&workspace, &resource, 3, Some(2), b"newer");
        let conflicted_receipt = projection
            .verified_local_recovery_receipt(&profile, &newer_remote)
            .unwrap()
            .expect("verified local data remains available when cloud advances");
        assert_eq!(conflicted_receipt.path, receipt.path);
        assert!(
            projection
                .active_session_conflicts_with_manifest(&profile, &newer_remote)
                .unwrap()
        );
        let commit_path = root.join(profile.as_str()).join(SYNC_COMMIT_MARKER);
        let mut malformed_commit: serde_json::Value =
            serde_json::from_slice(&fs::read(&commit_path).unwrap()).unwrap();
        malformed_commit["targetGeneration"] = serde_json::Value::from(base.generation + 3);
        let malformed_commit_bytes = serde_json::to_vec(&malformed_commit).unwrap();
        fs::write(&commit_path, &malformed_commit_bytes).unwrap();
        let error = projection
            .verified_local_recovery_receipt(&profile, &newer_remote)
            .unwrap_err();
        assert_eq!(error.code, "profile_sync_commit_marker_invalid");
        assert_eq!(fs::read(&commit_path).unwrap(), malformed_commit_bytes);
        assert_eq!(
            fs::read(receipt.path.join("Default/Cookies")).unwrap(),
            b"local dirty data"
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_commit_recovery_preserves_base_on_remote_conflict_and_keeps_post_snapshot_changes() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-commit-recovery-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_commit_recovery").unwrap();
        let resource = ResourceId::new("res_commit_recovery").unwrap();
        let profile = ProfileId::new("prf_commit_recovery").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        let conflict = recovery_manifest(&workspace, &resource, 3, Some(2), b"other device");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Cookies".to_owned(), b"base".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap();
        fs::write(receipt.path.join("Default/Cookies"), b"local dirty data").unwrap();

        let recovery = projection
            .active_session_recovery_path(&profile, &conflict)
            .unwrap()
            .expect("conflicting local profile remains available for recovery");
        assert_eq!(recovery, receipt.path);
        assert!(
            projection
                .active_session_conflicts_with_manifest(&profile, &conflict)
                .unwrap()
        );
        assert_eq!(
            fs::read(receipt.path.join("Default/Cookies")).unwrap(),
            b"local dirty data"
        );
        assert!(receipt.path.join(MARKER).exists());

        assert_eq!(
            projection
                .active_session_recovery_path(&profile, &target)
                .unwrap(),
            Some(receipt.path.clone())
        );
        assert_eq!(
            fs::read(receipt.path.join("Default/Cookies")).unwrap(),
            b"local dirty data"
        );
        assert!(
            root.join(profile.as_str())
                .join(SYNC_COMMIT_MARKER)
                .exists()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_commit_recovery_clears_stale_marker_when_local_base_is_unchanged() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-commit-unchanged-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_commit_unchanged").unwrap();
        let resource = ResourceId::new("res_commit_unchanged").unwrap();
        let profile = ProfileId::new("prf_commit_unchanged").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        let newer_remote = recovery_manifest(&workspace, &resource, 3, Some(2), b"newer");
        projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Cookies".to_owned(), b"base".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap();

        assert!(
            projection
                .active_session_recovery_path(&profile, &newer_remote)
                .unwrap()
                .is_none()
        );
        assert!(
            !root
                .join(profile.as_str())
                .join(SYNC_COMMIT_MARKER)
                .exists()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn committed_target_keeps_post_snapshot_local_changes_for_next_sync() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-committed-dirty-base-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_committed_dirty_base").unwrap();
        let resource = ResourceId::new("res_committed_dirty_base").unwrap();
        let profile = ProfileId::new("prf_committed_dirty_base").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"committed");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Cookies".to_owned(), b"snapshot bytes".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap();
        fs::write(
            receipt.path.join("Default/Cookies"),
            b"changed after snapshot",
        )
        .unwrap();

        let recovery = projection
            .active_session_recovery_path(&profile, &target)
            .unwrap()
            .expect("post-snapshot local data remains available after remote commit");
        let receipt_for_target = projection
            .verified_local_recovery_receipt(&profile, &target)
            .unwrap()
            .expect("dirty base is reopened as the working copy for the committed target");

        assert_eq!(recovery, receipt.path);
        assert_eq!(receipt_for_target.path, receipt.path);
        assert_eq!(receipt_for_target.generation, target.generation);
        assert_eq!(receipt_for_target.root_checksum, target.root_checksum);
        assert_eq!(
            fs::read(receipt.path.join("Default/Cookies")).unwrap(),
            b"changed after snapshot"
        );
        assert!(
            root.join(profile.as_str())
                .join(SYNC_COMMIT_MARKER)
                .exists()
        );
        assert!(
            !root
                .join(profile.as_str())
                .join(ACTIVE_SESSION_MARKER)
                .exists()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_sync_commit_cannot_replace_newer_pending_generation_or_dirty_data() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-stale-commit-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_stale_commit").unwrap();
        let resource = ResourceId::new("res_stale_commit").unwrap();
        let profile = ProfileId::new("prf_stale_commit").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let generation_two = recovery_manifest(&workspace, &resource, 2, Some(1), b"two");
        let generation_three = recovery_manifest(&workspace, &resource, 3, Some(2), b"three");
        projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Cookies".to_owned(), b"base local data".to_vec())],
            )
            .unwrap();
        let receipt = projection
            .publish_decrypted(
                &profile,
                &generation_two,
                vec![("Default/Cookies".to_owned(), b"dirty local data".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(
                &profile,
                generation_two.generation,
                &generation_two.root_checksum,
            )
            .unwrap();
        projection
            .prepare_sync_commit(&profile, &generation_two, &generation_three)
            .unwrap();

        let stale = projection
            .prepare_sync_commit(&profile, &base, &generation_two)
            .unwrap_err();

        assert_eq!(stale.code, "profile_sync_commit_stale");
        assert_eq!(stale.status, 409);
        assert!(!stale.retryable);
        let marker: SyncCommitMarker = serde_json::from_slice(
            &fs::read(root.join(profile.as_str()).join(SYNC_COMMIT_MARKER)).unwrap(),
        )
        .unwrap();
        assert_eq!(marker.target_generation, generation_three.generation);
        assert_eq!(
            fs::read(receipt.path.join("Default/Cookies")).unwrap(),
            b"dirty local data"
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_pending_commit_is_preserved_and_not_replaced_by_retry() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-invalid-commit-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_invalid_commit").unwrap();
        let resource = ResourceId::new("res_invalid_commit").unwrap();
        let profile = ProfileId::new("prf_invalid_commit").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Cookies".to_owned(), b"dirty local data".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        let marker_path = root.join(profile.as_str()).join(SYNC_COMMIT_MARKER);
        let invalid_marker = b"{not valid commit metadata}";
        fs::write(&marker_path, invalid_marker).unwrap();

        let error = projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap_err();

        assert_eq!(error.code, "profile_sync_commit_marker_invalid");
        assert_eq!(error.status, 422);
        assert!(!error.retryable);
        assert_eq!(fs::read(&marker_path).unwrap(), invalid_marker);
        assert_eq!(
            fs::read(receipt.path.join("Default/Cookies")).unwrap(),
            b"dirty local data"
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_upload_cannot_prepare_commit_after_session_advances() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-stale-upload-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_stale_upload").unwrap();
        let resource = ResourceId::new("res_stale_upload").unwrap();
        let profile = ProfileId::new("prf_stale_upload").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"upload");
        let current = recovery_manifest(&workspace, &resource, 2, Some(1), b"new session");
        projection
            .begin_active_session(&profile, current.generation, &current.root_checksum)
            .unwrap();

        let error = projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap_err();

        assert_eq!(error.code, "profile_sync_commit_stale");
        assert_eq!(error.status, 409);
        assert!(!error.retryable);
        assert!(
            !root
                .join(profile.as_str())
                .join(SYNC_COMMIT_MARKER)
                .exists()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_upload_cannot_prepare_commit_after_session_marker_is_cleared() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-cleared-session-marker-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_cleared_session_marker").unwrap();
        let resource = ResourceId::new("res_cleared_session_marker").unwrap();
        let profile = ProfileId::new("prf_cleared_session_marker").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        projection.clear_active_session_marker(&profile).unwrap();

        let error = projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap_err();

        assert_eq!(error.code, "profile_sync_commit_stale");
        assert_eq!(error.status, 409);
        assert!(!error.retryable);
        assert!(
            !root
                .join(profile.as_str())
                .join(SYNC_COMMIT_MARKER)
                .exists()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_commit_requires_target_manifest_parent_to_match_uploaded_base() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-commit-parent-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_commit_parent").unwrap();
        let resource = ResourceId::new("res_commit_parent").unwrap();
        let profile = ProfileId::new("prf_commit_parent").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let mut target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        target.parent_generation = None;

        let error = projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap_err();

        assert_eq!(error.code, "invalid_manifest_parent");
        assert_eq!(error.status, 400);
        assert!(!error.retryable);
        assert!(
            !root
                .join(profile.as_str())
                .join(SYNC_COMMIT_MARKER)
                .exists()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_commit_rejects_manifest_checksum_corruption_before_marker_write() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-commit-integrity-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_commit_integrity").unwrap();
        let resource = ResourceId::new("res_commit_integrity").unwrap();
        let profile = ProfileId::new("prf_commit_integrity").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let mut target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        target.objects[0].checksum = checksum(b"different ciphertext");

        let error = projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap_err();

        assert_eq!(error.code, "payload_root_checksum_mismatch");
        assert_eq!(error.status, 409);
        assert!(!error.retryable);
        assert!(
            !root
                .join(profile.as_str())
                .join(SYNC_COMMIT_MARKER)
                .exists()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stale_hydration_is_fenced_by_newer_active_generation() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-stale-hydration-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_stale_hydration").unwrap();
        let resource = ResourceId::new("res_stale_hydration").unwrap();
        let profile = ProfileId::new("prf_stale_hydration").unwrap();
        let old = recovery_manifest(&workspace, &resource, 1, None, b"old");
        let current = recovery_manifest(&workspace, &resource, 2, Some(1), b"current");
        projection
            .begin_active_session(&profile, current.generation, &current.root_checksum)
            .unwrap();

        let stale =
            ensure_hydration_generation_current(&root.join(profile.as_str()), &profile, &old)
                .unwrap_err();

        assert_eq!(stale.code, "profile_hydration_stale");
        assert_eq!(stale.status, 409);
        assert!(!stale.retryable);
        assert!(
            !decrypted_generation_path(&root, &profile, &old)
                .unwrap()
                .exists()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn durable_hydration_fence_rejects_older_generation_and_allows_same_retry() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-hydration-fence-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let workspace = WorkspaceId::new("ws_hydration_fence").unwrap();
        let resource = ResourceId::new("res_hydration_fence").unwrap();
        let profile = ProfileId::new("prf_hydration_fence").unwrap();
        let old = recovery_manifest(&workspace, &resource, 1, None, b"old");
        let current = recovery_manifest(&workspace, &resource, 2, Some(1), b"current");
        let competing = recovery_manifest(&workspace, &resource, 2, Some(1), b"competing");
        {
            let projection = ProfileProjection::open(&root).unwrap();
            projection
                .record_hydration_fence(&profile, &current)
                .unwrap();
            projection
                .record_hydration_fence(&profile, &current)
                .unwrap();
            let stale = projection
                .record_hydration_fence(&profile, &old)
                .unwrap_err();
            assert_eq!(stale.code, "profile_hydration_stale");
            assert_eq!(stale.status, 409);
            assert!(!stale.retryable);
            let fork = projection
                .record_hydration_fence(&profile, &competing)
                .unwrap_err();
            assert_eq!(fork.code, "profile_hydration_stale");
            let stale_session = projection
                .begin_active_session(&profile, old.generation, &old.root_checksum)
                .unwrap_err();
            assert_eq!(stale_session.code, "profile_hydration_stale");
        }
        let projection = ProfileProjection::open(&root).unwrap();

        let stale =
            ensure_hydration_generation_current(&root.join(profile.as_str()), &profile, &old)
                .unwrap_err();

        assert_eq!(stale.code, "profile_hydration_stale");
        assert!(!stale.retryable);
        ensure_hydration_generation_current(&root.join(profile.as_str()), &profile, &current)
            .unwrap();
        let fence_path = root.join(profile.as_str()).join(HYDRATION_FENCE_MARKER);
        let zero_fence = HydrationFenceMarker {
            profile_id: profile.clone(),
            generation: 0,
            root_checksum: current.root_checksum.clone(),
        };
        let zero_fence_bytes = serde_json::to_vec(&zero_fence).unwrap();
        fs::write(&fence_path, &zero_fence_bytes).unwrap();
        let error =
            ensure_hydration_generation_current(&root.join(profile.as_str()), &profile, &current)
                .unwrap_err();
        assert_eq!(error.code, "profile_hydration_fence_invalid");
        assert_eq!(fs::read(&fence_path).unwrap(), zero_fence_bytes);
        let error = projection
            .record_hydration_fence(&profile, &current)
            .unwrap_err();
        assert_eq!(error.code, "profile_hydration_fence_invalid");
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_sync_clear_advances_hydration_fence_to_committed_target() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-commit-hydration-fence-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_commit_hydration_fence").unwrap();
        let resource = ResourceId::new("res_commit_hydration_fence").unwrap();
        let profile = ProfileId::new("prf_commit_hydration_fence").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap();

        projection.clear_active_session(&profile).unwrap();

        let stale =
            ensure_hydration_generation_current(&root.join(profile.as_str()), &profile, &base)
                .unwrap_err();
        assert_eq!(stale.code, "profile_hydration_stale");
        ensure_hydration_generation_current(&root.join(profile.as_str()), &profile, &target)
            .unwrap();
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hydration_refuses_pending_commit_base_and_preserves_dirty_generation() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-hydrate-pending-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_hydrate_pending").unwrap();
        let resource = ResourceId::new("res_hydrate_pending").unwrap();
        let profile = ProfileId::new("prf_hydrate_pending").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Cookies".to_owned(), b"dirty local data".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap();

        let recovery =
            ensure_hydration_generation_current(&root.join(profile.as_str()), &profile, &base)
                .unwrap_err();

        assert_eq!(recovery.code, "profile_local_sync_recovery_required");
        assert_eq!(recovery.status, 409);
        assert!(!recovery.retryable);
        ensure_hydration_generation_current(&root.join(profile.as_str()), &profile, &target)
            .expect("the exact pending target may be hydrated from its recorded base");
        assert_eq!(
            fs::read(receipt.path.join("Default/Cookies")).unwrap(),
            b"dirty local data"
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn active_session_recovery_clears_stale_marker_when_local_generation_is_unchanged() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-active-unchanged-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_active_unchanged").unwrap();
        let resource = ResourceId::new("res_active_unchanged").unwrap();
        let profile = ProfileId::new("prf_active_unchanged").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let newer_remote = recovery_manifest(&workspace, &resource, 2, Some(1), b"newer");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Cookies".to_owned(), b"base".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();

        assert!(
            projection
                .active_session_recovery_path(&profile, &newer_remote)
                .unwrap()
                .is_none()
        );
        assert!(
            !root
                .join(profile.as_str())
                .join(ACTIVE_SESSION_MARKER)
                .exists()
        );
        assert_eq!(
            fs::read(receipt.path.join("Default/Cookies")).unwrap(),
            b"base"
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn hydration_reuses_verified_generation_and_cached_key_envelope() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-hydrate-cache-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_hydration_cache").unwrap();
        let resource = ResourceId::new("res_hydration_cache").unwrap();
        let profile = ProfileId::new("prf_hydration_cache").unwrap();
        let source = root.join("source-profile");
        fs::create_dir_all(source.join("Default")).unwrap();
        fs::write(source.join("Default/Preferences"), b"remote profile bytes").unwrap();
        let previous = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            b"",
            Vec::new(),
        )
        .unwrap();
        let client_key = ClientKey::from_bytes([0x71; 32]);
        let recovery_key = RecoveryKey::from_bytes([0x81; 32]);
        let (descriptor, encrypted_objects, manifest) = projection
            .snapshot_with_proxy_encryption(&source, &previous, 2, None, &client_key, &recovery_key)
            .unwrap();
        let profile_bytes = b"remote profile bytes".to_vec();
        projection
            .publish_decrypted(
                &profile,
                &manifest,
                vec![("Default/Preferences".to_owned(), profile_bytes)],
            )
            .unwrap();
        assert!(
            descriptor
                .objects
                .iter()
                .any(|object| object.path == "Default/Preferences")
        );
        let cache_scope = checksum(
            format!(
                "{}/{}/{}",
                workspace.as_str(),
                resource.as_str(),
                checksum(client_key.as_bytes())
            )
            .as_bytes(),
        );
        let envelope_reference = &manifest
            .payload_encryption
            .as_ref()
            .unwrap()
            .resource_key_envelope;
        let immutable_envelope = root.join(".sync-cache").join(&cache_scope).join(format!(
            "object-{}-{}.bin",
            checksum(envelope_reference.key.as_bytes()),
            envelope_reference.checksum
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let objects: BTreeMap<_, _> = encrypted_objects
            .into_iter()
            .map(|blob| (blob.key.clone(), blob.read_bytes().unwrap()))
            .collect();
        fs::remove_file(immutable_envelope).unwrap();
        assert!(
            root.join(".sync-cache")
                .join(&cache_scope)
                .join("resource-key-envelope.json")
                .is_file()
        );
        let server = tokio::spawn(async move {
            let mut paths = Vec::new();
            loop {
                let accepted =
                    tokio::time::timeout(Duration::from_millis(250), listener.accept()).await;
                let Ok(Ok((mut stream, _))) = accepted else {
                    break;
                };
                let request = read_http_request(&mut stream).await;
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .to_owned();
                let object_key = if path.contains("resource-key-envelope.json") {
                    RESOURCE_KEY_ENVELOPE_KEY
                } else if path.contains("Default%2FPreferences") {
                    "Default/Preferences"
                } else {
                    panic!("unexpected blob request: {path}");
                };
                let body = objects.get(object_key).unwrap();
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(body).await.unwrap();
                paths.push(path);
            }
            paths
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let auth = CoordinationAuth::new("desktop-cache", "lease-token").unwrap();
        let (receipt, credentials) = projection
            .hydrate_encrypted_with_proxy_credentials(
                &client,
                &auth,
                &profile,
                &manifest,
                Some(&client_key),
                Some(&recovery_key),
                "hydration-cache-test",
            )
            .await
            .unwrap();
        let requested_paths = server.await.unwrap();
        assert_eq!(receipt.generation, manifest.generation);
        assert!(credentials.is_empty());
        assert!(requested_paths.is_empty());
        let _ = fs::remove_dir_all(root);
    }

    async fn read_http_request(stream: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut chunk = [0; 2048];
        loop {
            let count = stream.read(&mut chunk).await.unwrap();
            assert!(count > 0, "client closed before sending request");
            bytes.extend_from_slice(&chunk[..count]);
            if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }

    #[tokio::test]
    async fn recovery_envelope_hydrates_matching_profile_content_on_independent_devices() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-cross-device-recovery-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let source_projection = ProfileProjection::open(&root.join("source")).unwrap();
        let workspace = WorkspaceId::new("ws_cross_device_recovery").unwrap();
        let resource = ResourceId::new("res_cross_device_recovery").unwrap();
        let source = root.join("source-profile");
        fs::create_dir_all(source.join("Default")).unwrap();
        fs::write(
            source.join("Default/Preferences"),
            b"same encrypted profile state on each device",
        )
        .unwrap();
        fs::create_dir_all(source.join("Default/Sessions")).unwrap();
        fs::write(
            source.join("Default/Sessions/Session_123456789"),
            b"simulated open-tab session state",
        )
        .unwrap();
        fs::write(
            source.join("Default/Sessions/Tabs_123456789"),
            b"simulated tab navigation state",
        )
        .unwrap();
        let previous = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            b"",
            Vec::new(),
        )
        .unwrap();
        let client_key = ClientKey::from_bytes([0x71; 32]);
        let recovery_key = RecoveryKey::from_bytes([0x81; 32]);
        let (descriptor, objects, manifest) = source_projection
            .snapshot_with_proxy_encryption(&source, &previous, 2, None, &client_key, &recovery_key)
            .unwrap();
        descriptor.verify_manifest(&manifest).unwrap();
        for path in [
            "Default/Sessions/Session_123456789",
            "Default/Sessions/Tabs_123456789",
        ] {
            assert!(descriptor.objects.iter().any(|object| object.path == path));
            assert!(manifest.objects.iter().any(|object| object.key == path));
        }
        let remote_objects: BTreeMap<_, _> = objects
            .iter()
            .map(|object| (object.key.clone(), object.read_bytes().unwrap()))
            .collect();
        let envelope_bytes = remote_objects
            .get(RESOURCE_KEY_ENVELOPE_KEY)
            .expect("encrypted manifest includes the recovery envelope");
        let envelope: ResourceKeyEnvelope = serde_json::from_slice(envelope_bytes).unwrap();
        let recovery_code = recovery_key.recovery_code();

        // Each device independently imports the user-held recovery material and
        // unwraps the same scoped envelope; no OS-vault state is shared here.
        let device_a_recovery = RecoveryKey::parse_code(&recovery_code).unwrap();
        let device_b_recovery = RecoveryKey::parse_code(&recovery_code).unwrap();
        let device_a_key = envelope
            .unwrap(&workspace, &resource, &device_a_recovery)
            .unwrap();
        let device_b_key = envelope
            .unwrap(&workspace, &resource, &device_b_recovery)
            .unwrap();
        assert_eq!(device_a_key, device_b_key);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let remote_objects = Arc::new(remote_objects);
        let active_downloads = Arc::new(AtomicUsize::new(0));
        let peak_downloads = Arc::new(AtomicUsize::new(0));
        let active_profile_downloads = Arc::new(AtomicUsize::new(0));
        let peak_profile_downloads = Arc::new(AtomicUsize::new(0));
        let server_peak_downloads = Arc::clone(&peak_downloads);
        let server_active_profile_downloads = Arc::clone(&active_profile_downloads);
        let server_peak_profile_downloads = Arc::clone(&peak_profile_downloads);
        let server = tokio::spawn(async move {
            loop {
                let accepted =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept()).await;
                let Ok(Ok((mut stream, _))) = accepted else {
                    break;
                };
                let remote_objects = Arc::clone(&remote_objects);
                let active_downloads = Arc::clone(&active_downloads);
                let peak_downloads = Arc::clone(&server_peak_downloads);
                let active_profile_downloads = Arc::clone(&server_active_profile_downloads);
                let peak_profile_downloads = Arc::clone(&server_peak_profile_downloads);
                tokio::spawn(async move {
                    let request = read_http_request(&mut stream).await;
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or_default();
                    let object_key = remote_objects
                        .keys()
                        .map(String::as_str)
                        .find(|key| path.contains(&key.replace('/', "%2F")))
                        .unwrap_or_else(|| panic!("unexpected blob request: {path}"));
                    let body = remote_objects.get(object_key).unwrap();
                    let active = active_downloads.fetch_add(1, Ordering::SeqCst) + 1;
                    peak_downloads.fetch_max(active, Ordering::SeqCst);
                    let is_profile_object = object_key != RESOURCE_KEY_ENVELOPE_KEY;
                    if is_profile_object {
                        let active = active_profile_downloads.fetch_add(1, Ordering::SeqCst) + 1;
                        peak_profile_downloads.fetch_max(active, Ordering::SeqCst);
                    }
                    tokio::time::sleep(Duration::from_millis(30)).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    stream.write_all(response.as_bytes()).await.unwrap();
                    stream.write_all(body).await.unwrap();
                    active_downloads.fetch_sub(1, Ordering::SeqCst);
                    if is_profile_object {
                        active_profile_downloads.fetch_sub(1, Ordering::SeqCst);
                    }
                });
            }
        });

        let client_a =
            RemoteApiClient::new(&format!("http://{address}"), "device-a".into()).unwrap();
        let client_b =
            RemoteApiClient::new(&format!("http://{address}"), "device-b".into()).unwrap();
        let auth_a = CoordinationAuth::new("device-a", "lease-a").unwrap();
        let auth_b = CoordinationAuth::new("device-b", "lease-b").unwrap();
        let projection_a = ProfileProjection::open(&root.join("device-a")).unwrap();
        let projection_b = ProfileProjection::open(&root.join("device-b")).unwrap();
        let profile_id = ProfileId::new("prf_cross_device_recovery").unwrap();
        let (result_a, result_b) = tokio::join!(
            projection_a.hydrate_encrypted_with_proxy_credentials(
                &client_a,
                &auth_a,
                &profile_id,
                &manifest,
                Some(&device_a_key),
                Some(&device_a_recovery),
                "cross-device-recovery-a",
            ),
            projection_b.hydrate_encrypted_with_proxy_credentials(
                &client_b,
                &auth_b,
                &profile_id,
                &manifest,
                Some(&device_b_key),
                Some(&device_b_recovery),
                "cross-device-recovery-b",
            )
        );
        let (receipt_a, _) = result_a.unwrap();
        let (receipt_b, _) = result_b.unwrap();

        let restored_a = fs::read(receipt_a.path.join("Default/Preferences")).unwrap();
        let restored_b = fs::read(receipt_b.path.join("Default/Preferences")).unwrap();
        assert_eq!(checksum(&restored_a), checksum(&restored_b));
        assert_eq!(restored_a, b"same encrypted profile state on each device");
        for (relative, expected) in [
            (
                "Default/Sessions/Session_123456789",
                b"simulated open-tab session state".as_slice(),
            ),
            (
                "Default/Sessions/Tabs_123456789",
                b"simulated tab navigation state".as_slice(),
            ),
        ] {
            let device_a_bytes = fs::read(receipt_a.path.join(relative)).unwrap();
            let device_b_bytes = fs::read(receipt_b.path.join(relative)).unwrap();
            assert_eq!(device_a_bytes, expected);
            assert_eq!(device_b_bytes, expected);
            assert_eq!(checksum(&device_a_bytes), checksum(&device_b_bytes));
        }
        assert_eq!(receipt_a.root_checksum, receipt_b.root_checksum);

        server.abort();
        assert!(peak_downloads.load(Ordering::SeqCst) >= 2);
        assert!(peak_downloads.load(Ordering::SeqCst) <= MAX_CONCURRENT_PROFILE_DOWNLOADS);
        assert!(peak_profile_downloads.load(Ordering::SeqCst) >= 2);
        assert!(peak_profile_downloads.load(Ordering::SeqCst) <= MAX_CONCURRENT_PROFILE_DOWNLOADS);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verified_generation_skips_profile_blobs_but_keeps_sync_metadata_downloads() {
        let proxy_id = ProxyId::new("prx_projection_cache").unwrap();
        let metadata_key = ".buglogin/proxy-dependencies/prx_projection_cache/4/metadata.json";
        let credential_key =
            ".buglogin/proxy-dependencies/prx_projection_cache/4/credential-envelope.json";
        let metadata = std::collections::BTreeSet::from([metadata_key.to_owned()]);
        let credentials = vec![(
            proxy_id,
            4,
            ManifestObjectReference {
                key: credential_key.to_owned(),
                size: 64,
                checksum: "a".repeat(64),
            },
        )];

        assert!(!should_download_manifest_object(
            "Default/Cookies",
            &metadata,
            &credentials,
        ));
        assert!(should_download_manifest_object(
            RESOURCE_KEY_ENVELOPE_KEY,
            &metadata,
            &credentials,
        ));
        assert!(should_download_manifest_object(
            metadata_key,
            &metadata,
            &credentials,
        ));
        assert!(should_download_manifest_object(
            credential_key,
            &metadata,
            &credentials,
        ));
    }

    #[test]
    fn allows_chunked_files_but_enforces_the_logical_file_limit() {
        assert!(validate_profile_file_size(MAX_PROFILE_PLAINTEXT_BYTES).is_ok());
        assert!(validate_profile_file_size(MAX_PROFILE_PLAINTEXT_BYTES + 1).is_ok());
        let error = validate_profile_file_size(MAX_CHUNKED_PROFILE_FILE_BYTES + 1).unwrap_err();
        assert_eq!(error.code, "profile_object_too_large");
        assert_eq!(error.status, 413);
    }

    #[test]
    fn v2_profile_aad_is_stable_across_generations_while_v1_remains_generation_bound() {
        let workspace = WorkspaceId::new("ws_projection").unwrap();
        let resource = ResourceId::new("res_projection").unwrap();
        let key = ClientKey::from_bytes([0x27; 32]);
        let v2_aad = profile_object_aad_v2(&workspace, &resource, "Default/Cookies");
        let encrypted = AuthenticatedChunk::encrypt(&key, b"cookie-data", &v2_aad).unwrap();
        assert_eq!(encrypted.decrypt(&key, &v2_aad).unwrap(), b"cookie-data");
        assert_eq!(
            v2_aad,
            b"buglogin/profile-object/v2/ws_projection/res_projection/Default/Cookies"
        );
        assert_ne!(
            profile_object_aad(&workspace, &resource, 3, "Default/Cookies"),
            profile_object_aad(&workspace, &resource, 4, "Default/Cookies")
        );
    }

    #[test]
    fn publishes_complete_generation_and_reuses_verified_cache() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_projection").unwrap();
        let resource = ResourceId::new("res_projection").unwrap();
        let profile = ProfileId::new("prf_projection").unwrap();
        let descriptor = PayloadDescriptor::new(
            workspace.clone(),
            resource.clone(),
            1,
            vec![PayloadObject::new("Default/Cookies", b"cookie-data").unwrap()],
        )
        .unwrap();
        let manifest = SyncManifest::new(
            workspace,
            resource,
            1,
            1,
            None,
            &descriptor.canonical_root_bytes(),
            vec![ManifestObject::new("Default/Cookies", b"cookie-data").unwrap()],
        )
        .unwrap();
        let first = projection
            .publish(
                &profile,
                &manifest,
                &descriptor,
                vec![("Default/Cookies".into(), b"cookie-data".to_vec())],
            )
            .unwrap();
        let second = projection
            .publish(&profile, &manifest, &descriptor, Vec::new())
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            fs::read(first.path.join("Default/Cookies")).unwrap(),
            b"cookie-data"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn incomplete_projection_never_publishes_generation() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-incomplete-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_projection").unwrap();
        let resource = ResourceId::new("res_projection").unwrap();
        let profile = ProfileId::new("prf_projection").unwrap();
        let descriptor = PayloadDescriptor::new(
            workspace.clone(),
            resource.clone(),
            1,
            vec![PayloadObject::new("Default/Cookies", b"cookie-data").unwrap()],
        )
        .unwrap();
        let manifest = SyncManifest::new(
            workspace,
            resource,
            1,
            1,
            None,
            &descriptor.canonical_root_bytes(),
            vec![ManifestObject::new("Default/Cookies", b"cookie-data").unwrap()],
        )
        .unwrap();
        assert_eq!(
            projection
                .publish(&profile, &manifest, &descriptor, Vec::new())
                .unwrap_err()
                .code,
            "profile_projection_incomplete"
        );
        assert!(!root.join(profile.as_str()).join("generations").exists());

        let staging = DecryptedGenerationStaging::new(&root, &profile, &manifest).unwrap();
        let staging_path = staging.staging_path.clone();
        let (mut incomplete_file, _) = staging.create_file("Default/Cookies").unwrap();
        incomplete_file.write_all(b"partial file").unwrap();
        drop(incomplete_file);
        drop(staging);
        assert!(!staging_path.exists());
        assert!(!root.join(profile.as_str()).join("generations").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verified_generation_rejects_untracked_local_files() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-untracked-file-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_untracked_file").unwrap();
        let resource = ResourceId::new("res_untracked_file").unwrap();
        let profile = ProfileId::new("prf_untracked_file").unwrap();
        let manifest = recovery_manifest(&workspace, &resource, 1, None, b"verified");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &manifest,
                vec![("Default/Preferences".to_owned(), b"verified".to_vec())],
            )
            .unwrap();
        fs::write(receipt.path.join("Default/untracked"), b"local data").unwrap();

        let error = verify_decrypted_generation(&profile, &manifest, &receipt.path).unwrap_err();

        assert_eq!(error.code, "profile_projection_incomplete");
        assert_eq!(error.status, 422);
        assert_eq!(
            fs::read(receipt.path.join("Default/untracked")).unwrap(),
            b"local data"
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn changed_generation_file_is_not_mistaken_for_clean_recovery_state() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-changed-recovery-file-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_changed_recovery_file").unwrap();
        let resource = ResourceId::new("res_changed_recovery_file").unwrap();
        let profile = ProfileId::new("prf_changed_recovery_file").unwrap();
        let manifest = recovery_manifest(&workspace, &resource, 1, None, b"baseline");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &manifest,
                vec![("Default/Preferences".to_owned(), b"baseline".to_vec())],
            )
            .unwrap();
        fs::write(receipt.path.join("Default/Preferences"), b"modified").unwrap();

        let is_clean = local_generation_matches_projection_marker(
            &profile,
            manifest.generation,
            &manifest.root_checksum,
            &receipt.path,
        )
        .unwrap();

        assert!(!is_clean);
        assert!(
            local_generation_is_recoverable(
                &profile,
                manifest.generation,
                &manifest.root_checksum,
                &receipt.path,
            )
            .unwrap()
        );
        assert_eq!(
            fs::read(receipt.path.join("Default/Preferences")).unwrap(),
            b"modified"
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clean_generation_marker_comparison_rejects_missing_and_extra_files() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-clean-file-set-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_clean_file_set").unwrap();
        let resource = ResourceId::new("res_clean_file_set").unwrap();
        let profile = ProfileId::new("prf_clean_file_set").unwrap();
        let manifest = recovery_manifest(&workspace, &resource, 1, None, b"root");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &manifest,
                vec![
                    ("Default/Preferences".to_owned(), b"prefs".to_vec()),
                    ("Default/Cookies".to_owned(), b"cookies".to_vec()),
                ],
            )
            .unwrap();
        assert!(
            local_generation_matches_projection_marker(
                &profile,
                manifest.generation,
                &manifest.root_checksum,
                &receipt.path,
            )
            .unwrap()
        );

        fs::remove_file(receipt.path.join("Default/Cookies")).unwrap();
        assert!(
            !local_generation_matches_projection_marker(
                &profile,
                manifest.generation,
                &manifest.root_checksum,
                &receipt.path,
            )
            .unwrap()
        );
        fs::write(receipt.path.join("Default/Cookies"), b"cookies").unwrap();
        fs::write(receipt.path.join("Default/untracked"), b"extra").unwrap();
        assert!(
            !local_generation_matches_projection_marker(
                &profile,
                manifest.generation,
                &manifest.root_checksum,
                &receipt.path,
            )
            .unwrap()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn local_generation_symlink_is_not_a_cache_hit_or_recovery_path() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-generation-symlink-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let external = std::env::temp_dir().join(format!(
            "buglogin-projection-generation-target-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&external).unwrap();
        let generation = root.join("generation");
        symlink(&external, &generation).unwrap();

        let error = local_generation_directory_exists(&generation).unwrap_err();

        assert_eq!(error.code, "profile_local_generation_invalid");
        assert_eq!(error.status, 422);
        assert!(
            fs::symlink_metadata(&generation)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(external).unwrap();
    }

    #[test]
    fn corrupt_projection_marker_is_not_classified_as_retryable_io() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-corrupt-marker-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_corrupt_projection_marker").unwrap();
        let resource = ResourceId::new("res_corrupt_projection_marker").unwrap();
        let profile = ProfileId::new("prf_corrupt_projection_marker").unwrap();
        let manifest = recovery_manifest(&workspace, &resource, 1, None, b"profile");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &manifest,
                vec![("Default/Preferences".to_owned(), b"profile".to_vec())],
            )
            .unwrap();
        fs::write(receipt.path.join(MARKER), b"{invalid marker").unwrap();

        let error = local_generation_is_recoverable(
            &profile,
            manifest.generation,
            &manifest.root_checksum,
            &receipt.path,
        )
        .unwrap_err();

        assert_eq!(error.code, "profile_projection_marker_invalid");
        assert_eq!(error.status, 422);
        assert!(!error.retryable);
        assert_eq!(
            fs::read(receipt.path.join(MARKER)).unwrap(),
            b"{invalid marker"
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_refuses_existing_generation_without_projection_marker() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-recovery-missing-marker-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let profile = ProfileId::new("prf_recovery_missing_marker").unwrap();
        let generation_path = root.join("generations/1");
        fs::create_dir_all(&generation_path).unwrap();
        fs::write(
            generation_path.join("Default-Cookies"),
            b"preserve dirty bytes",
        )
        .unwrap();

        let error =
            local_generation_is_recoverable(&profile, 1, &checksum(b"root"), &generation_path)
                .unwrap_err();

        assert_eq!(error.code, "profile_projection_marker_missing");
        assert!(!error.retryable);
        assert_eq!(
            fs::read(generation_path.join("Default-Cookies")).unwrap(),
            b"preserve dirty bytes"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_receipt_errors_when_commit_marker_points_to_missing_local_generation() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-missing-recovery-generation-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_missing_recovery_generation").unwrap();
        let resource = ResourceId::new("res_missing_recovery_generation").unwrap();
        let profile = ProfileId::new("prf_missing_recovery_generation").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Preferences".to_owned(), b"dirty local".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap();
        fs::remove_dir_all(&receipt.path).unwrap();

        let error = projection
            .verified_local_recovery_receipt(&profile, &target)
            .unwrap_err();

        assert_eq!(error.code, "profile_local_generation_missing");
        assert_eq!(error.status, 422);
        assert!(!error.retryable);
        assert!(
            root.join(profile.as_str())
                .join(SYNC_COMMIT_MARKER)
                .exists()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pending_commit_recovery_surfaces_corrupt_projection_marker() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-pending-corrupt-marker-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_pending_corrupt_marker").unwrap();
        let resource = ResourceId::new("res_pending_corrupt_marker").unwrap();
        let profile = ProfileId::new("prf_pending_corrupt_marker").unwrap();
        let base = recovery_manifest(&workspace, &resource, 1, None, b"base");
        let target = recovery_manifest(&workspace, &resource, 2, Some(1), b"target");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Preferences".to_owned(), b"dirty data".to_vec())],
            )
            .unwrap();
        projection
            .begin_active_session(&profile, base.generation, &base.root_checksum)
            .unwrap();
        projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap();
        fs::write(receipt.path.join(MARKER), b"{corrupt projection marker").unwrap();

        let error = projection
            .active_session_recovery_path(&profile, &base)
            .unwrap_err();

        assert_eq!(error.code, "profile_projection_marker_invalid");
        assert_eq!(error.status, 422);
        assert!(!error.retryable);
        assert_eq!(
            fs::read(receipt.path.join("Default/Preferences")).unwrap(),
            b"dirty data"
        );
        assert!(
            root.join(profile.as_str())
                .join(SYNC_COMMIT_MARKER)
                .exists()
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn recovery_rejects_projection_marker_with_duplicate_paths() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-duplicate-marker-path-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_duplicate_marker_path").unwrap();
        let resource = ResourceId::new("res_duplicate_marker_path").unwrap();
        let profile = ProfileId::new("prf_duplicate_marker_path").unwrap();
        let manifest = recovery_manifest(&workspace, &resource, 1, None, b"profile");
        let receipt = projection
            .publish_decrypted(
                &profile,
                &manifest,
                vec![("Default/Preferences".to_owned(), b"profile".to_vec())],
            )
            .unwrap();
        let marker_path = receipt.path.join(MARKER);
        let mut marker: EncryptedProjectionMarker =
            serde_json::from_slice(&fs::read(&marker_path).unwrap()).unwrap();
        marker.local_objects.push(marker.local_objects[0].clone());
        fs::write(&marker_path, serde_json::to_vec(&marker).unwrap()).unwrap();

        let error = local_generation_is_recoverable(
            &profile,
            manifest.generation,
            &manifest.root_checksum,
            &receipt.path,
        )
        .unwrap_err();

        assert_eq!(error.code, "profile_projection_marker_invalid");
        assert_eq!(error.status, 422);
        assert!(!error.retryable);
        assert_eq!(
            fs::read(receipt.path.join("Default/Preferences")).unwrap(),
            b"profile"
        );
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn oversized_recovery_marker_is_integrity_error_not_retryable_io() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-oversized-marker-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let marker = root.join(ACTIVE_SESSION_MARKER);
        fs::write(&marker, vec![b'x'; 64 * 1024 + 1]).unwrap();

        let error = read_projection_marker(&marker, "profile_session_marker_invalid").unwrap_err();

        assert_eq!(error.code, "profile_session_marker_invalid");
        assert_eq!(error.status, 422);
        assert!(!error.retryable);
        assert_eq!(fs::metadata(marker).unwrap().len(), 64 * 1024 + 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn snapshot_cache_paths_are_immutable_across_payload_replacement() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-immutable-cache-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        fs::create_dir_all(&root).unwrap();
        let alias = PathBuf::from(".sync-cache/profile/logical.bin");
        let first = write_snapshot_cache(&root, &alias, b"first ciphertext", true).unwrap();
        let first_path_bytes = fs::read(&first).unwrap();
        let second = write_snapshot_cache(&root, &alias, b"second ciphertext", true).unwrap();

        assert_ne!(first, second);
        assert_eq!(fs::read(first).unwrap(), first_path_bytes);
        assert_eq!(fs::read(&second).unwrap(), b"second ciphertext");
        assert_eq!(fs::read(root.join(&alias)).unwrap(), b"second ciphertext");
        fs::write(&second, b"partial cache write").unwrap();
        let repaired = write_snapshot_cache(&root, &alias, b"second ciphertext", false).unwrap();
        assert_eq!(repaired, second);
        assert_eq!(fs::read(repaired).unwrap(), b"second ciphertext");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn projection_root_lock_serializes_processes_and_open_cleans_crash_staging() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-restart-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let stale_profile = ProfileId::new("prf_stale_staging").unwrap();
        let stale_staging = root.join(stale_profile.as_str()).join("staging");
        fs::create_dir_all(stale_staging.join("interrupted-generation")).unwrap();
        fs::write(
            stale_staging.join("interrupted-generation/Default-Cookies.part"),
            b"partial ciphertext or plaintext must never be published",
        )
        .unwrap();

        let first = ProfileProjection::open(&root).unwrap();
        assert!(!stale_staging.exists());
        let second = ProfileProjection::open(&root).err().unwrap();
        assert_eq!(second.code, "profile_projection_in_use");
        drop(first);

        let reopened = ProfileProjection::open(&root).unwrap();
        drop(reopened);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_creates_canonical_successor_and_delete_tombstone() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-snapshot-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_projection").unwrap();
        let resource = ResourceId::new("res_projection").unwrap();
        let profile = ProfileId::new("prf_projection").unwrap();
        let descriptor = PayloadDescriptor::new(
            workspace.clone(),
            resource.clone(),
            1,
            vec![PayloadObject::new("Default/Cookies", b"cookie-data").unwrap()],
        )
        .unwrap();
        let manifest = SyncManifest::new(
            workspace,
            resource,
            1,
            1,
            None,
            &descriptor.canonical_root_bytes(),
            vec![ManifestObject::new("Default/Cookies", b"cookie-data").unwrap()],
        )
        .unwrap();
        let receipt = projection
            .publish(
                &profile,
                &manifest,
                &descriptor,
                vec![("Default/Cookies".into(), b"cookie-data".to_vec())],
            )
            .unwrap();
        fs::remove_file(receipt.path.join("Default/Cookies")).unwrap();
        fs::write(receipt.path.join("Preferences"), b"updated").unwrap();

        let (next_descriptor, files, next_manifest) = projection
            .snapshot_with_proxy(&receipt.path, &manifest, 2, None)
            .unwrap();

        assert_eq!(next_descriptor.generation, 2);
        assert_eq!(files, vec![("Preferences".into(), b"updated".to_vec())]);
        assert_eq!(next_manifest.parent_generation, Some(1));
        assert_eq!(next_manifest.fencing_token, 2);
        assert_eq!(next_manifest.tombstones.len(), 1);
        assert_eq!(next_manifest.tombstones[0].key, "Default/Cookies");
        next_descriptor.verify_manifest(&next_manifest).unwrap();
        next_manifest.validate_successor(&manifest, 1).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_recreates_live_path_after_tombstone_with_new_fence() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-tombstone-recreate-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_projection_recreate").unwrap();
        let resource = ResourceId::new("res_projection_recreate").unwrap();
        let previous = SyncManifest::new_with_tombstones(
            workspace,
            resource,
            4,
            1,
            None,
            b"deleted",
            Vec::new(),
            vec![ManifestTombstone::new("Default/Cookies", 1, 1, u64::MAX).unwrap()],
        )
        .unwrap();
        let profile_path = root.join("browser-profile");
        fs::create_dir_all(profile_path.join("Default")).unwrap();
        fs::write(profile_path.join("Default/Cookies"), b"fresh cookie db").unwrap();

        let (_descriptor, _files, next) = projection
            .snapshot_with_proxy(&profile_path, &previous, 5, None)
            .unwrap();

        assert!(next.tombstones.is_empty());
        next.validate_successor(&previous, 2).unwrap();
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_references_selected_proxy_revision_without_copying_metadata() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-proxy-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_projection").unwrap();
        let resource = ResourceId::new("res_projection").unwrap();
        let proxy_id = ProxyId::new("proxy_projection").unwrap();
        let profile_path = root.join("browser-profile");
        fs::create_dir_all(&profile_path).unwrap();
        fs::write(profile_path.join("Preferences"), b"profile-data").unwrap();
        let previous = SyncManifest::new(workspace, resource, 1, 1, None, b"", Vec::new()).unwrap();
        let (descriptor, files, manifest) = projection
            .snapshot_with_proxy(
                &profile_path,
                &previous,
                2,
                Some(RemoteProxySyncSnapshot {
                    proxy_id: proxy_id.clone(),
                    revision: 4,
                    credential_version: None,
                    credential: None,
                }),
            )
            .unwrap();

        let dependency = manifest.dependencies.first().unwrap();
        let ManifestDependency::Proxy {
            proxy_id: manifest_proxy_id,
            revision,
            metadata_object,
            credential_envelope_object,
            ..
        } = dependency;
        assert_eq!(manifest_proxy_id, &proxy_id);
        assert_eq!(*revision, 4);
        assert_eq!(credential_envelope_object, &None);
        assert_eq!(metadata_object, &None);
        assert!(
            !files
                .iter()
                .any(|(path, _)| path.contains("proxy-dependencies"))
        );
        assert_eq!(descriptor.generation, manifest.generation);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn encrypted_snapshot_hides_profile_bytes_and_binds_scope_and_generation() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-encrypted-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_encrypted_projection").unwrap();
        let resource = ResourceId::new("res_encrypted_projection").unwrap();
        let profile_path = root.join("browser-profile");
        fs::create_dir_all(&profile_path).unwrap();
        fs::write(profile_path.join("Preferences"), b"private-profile-data").unwrap();
        let previous = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            b"",
            Vec::new(),
        )
        .unwrap();
        let client_key = ClientKey::from_bytes([7; 32]);
        let recovery_key = RecoveryKey::from_bytes([9; 32]);

        let (descriptor, files, manifest) = projection
            .snapshot_with_proxy_encryption(
                &profile_path,
                &previous,
                2,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap();

        manifest.validate().unwrap();
        descriptor.verify_manifest(&manifest).unwrap();
        let encrypted = files.iter().find(|blob| blob.key == "Preferences").unwrap();
        let encrypted_bytes = encrypted.read_bytes().unwrap();
        assert!(
            !encrypted_bytes
                .windows(b"private-profile-data".len())
                .any(|window| { window == b"private-profile-data" })
        );
        let decrypted = AuthenticatedChunk::decode(&encrypted_bytes)
            .unwrap()
            .decrypt(
                &client_key,
                &profile_object_aad_v2(&workspace, &resource, "Preferences"),
            )
            .unwrap();
        assert_eq!(decrypted, b"private-profile-data");
        assert_eq!(
            manifest
                .payload_encryption
                .as_ref()
                .unwrap()
                .key_version
                .as_deref(),
            Some(checksum(client_key.as_bytes()).as_str())
        );

        let envelope_ref = &manifest
            .payload_encryption
            .as_ref()
            .unwrap()
            .resource_key_envelope;
        let envelope_bytes = files
            .iter()
            .find(|blob| blob.key == RESOURCE_KEY_ENVELOPE_KEY)
            .unwrap();
        let envelope_bytes = envelope_bytes.read_bytes().unwrap();
        assert_eq!(envelope_ref.checksum, checksum(&envelope_bytes));
        assert!(
            manifest
                .tombstones
                .iter()
                .all(|tombstone| tombstone.key != RESOURCE_KEY_ENVELOPE_KEY)
        );
        let envelope: ResourceKeyEnvelope = serde_json::from_slice(&envelope_bytes).unwrap();
        assert_eq!(
            envelope
                .unwrap(&workspace, &resource, &recovery_key)
                .unwrap(),
            client_key
        );

        let (next_descriptor, next_files, next_manifest) = projection
            .snapshot_with_proxy_encryption(
                &profile_path,
                &manifest,
                3,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap();
        let reused = next_manifest
            .objects
            .iter()
            .find(|object| object.key == "Preferences")
            .unwrap();
        assert_eq!(reused.source_generation, Some(manifest.generation));
        assert_eq!(
            next_descriptor
                .objects
                .iter()
                .find(|object| object.path == "Preferences")
                .unwrap()
                .checksum,
            reused.checksum
        );
        assert!(next_files.iter().any(|blob| {
            blob.key == "Preferences" && checksum(&blob.read_bytes().unwrap()) == reused.checksum
        }));
        let next_envelope = next_files
            .iter()
            .find(|blob| blob.key == RESOURCE_KEY_ENVELOPE_KEY)
            .unwrap();
        assert_eq!(next_envelope.checksum, envelope_ref.checksum);
        assert_eq!(
            next_manifest
                .objects
                .iter()
                .find(|object| object.key == RESOURCE_KEY_ENVELOPE_KEY)
                .unwrap()
                .source_generation,
            None
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn encrypted_snapshot_reuses_authenticated_cache_before_manifest_activation() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-retry-cache-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_retry_cache").unwrap();
        let resource = ResourceId::new("res_retry_cache").unwrap();
        let profile_path = root.join("browser-profile");
        fs::create_dir_all(&profile_path).unwrap();
        fs::write(profile_path.join("Preferences"), b"retry-me").unwrap();
        let previous = SyncManifest::new(workspace, resource, 1, 1, None, b"", Vec::new()).unwrap();
        let client_key = ClientKey::from_bytes([0x51; 32]);
        let recovery_key = RecoveryKey::from_bytes([0x61; 32]);
        let snapshot = || {
            projection
                .snapshot_with_proxy_encryption(
                    &profile_path,
                    &previous,
                    2,
                    None,
                    &client_key,
                    &recovery_key,
                )
                .unwrap()
        };

        let (_, first_files, first_manifest) = snapshot();
        let first = first_files
            .iter()
            .find(|blob| blob.key == "Preferences")
            .unwrap();
        let first_bytes = first.read_bytes().unwrap();
        let first_checksum = checksum(&first_bytes);
        assert_eq!(
            first_manifest
                .objects
                .iter()
                .find(|object| object.key == "Preferences")
                .unwrap()
                .source_generation,
            None
        );

        let (_, retry_files, retry_manifest) = snapshot();
        let retry = retry_files
            .iter()
            .find(|blob| blob.key == "Preferences")
            .unwrap();
        let retry_bytes = retry.read_bytes().unwrap();
        assert_eq!(checksum(&retry_bytes), first_checksum);
        assert_eq!(
            retry_manifest
                .objects
                .iter()
                .find(|object| object.key == "Preferences")
                .unwrap()
                .source_generation,
            None
        );
        assert_eq!(
            AuthenticatedChunk::decode(&retry_bytes)
                .unwrap()
                .decrypt(
                    &client_key,
                    &profile_object_aad_v2(
                        &previous.workspace_id,
                        &previous.resource_id,
                        "Preferences"
                    ),
                )
                .unwrap(),
            b"retry-me"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn encrypted_snapshot_validates_manifest_and_fence_before_scanning_source() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-snapshot-preflight-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_snapshot_preflight").unwrap();
        let resource = ResourceId::new("res_snapshot_preflight").unwrap();
        let previous = SyncManifest::new(workspace, resource, 1, 1, None, b"", Vec::new()).unwrap();
        let missing_source = root.join("source-does-not-exist");
        let client_key = ClientKey::from_bytes([0x13; 32]);
        let recovery_key = RecoveryKey::from_bytes([0x14; 32]);

        let mut invalid_previous = previous.clone();
        invalid_previous.root_checksum = "invalid".to_owned();
        let error = projection
            .snapshot_with_proxy_encryption(
                &missing_source,
                &invalid_previous,
                2,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap_err();
        assert_eq!(error.code, "invalid_manifest_checksum");

        let error = projection
            .snapshot_with_proxy_encryption(
                &missing_source,
                &previous,
                0,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap_err();
        assert_eq!(error.code, "invalid_manifest_fencing_token");

        let missing_source_error = projection
            .snapshot_with_proxy_encryption(
                &missing_source,
                &previous,
                2,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap_err();
        assert_eq!(missing_source_error.code, "profile_source_missing");
        assert_eq!(missing_source_error.status, 409);
        assert!(!missing_source_error.retryable);
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn encrypted_snapshot_retry_keeps_delete_tombstone_identity() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-tombstone-retry-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_tombstone_retry").unwrap();
        let resource = ResourceId::new("res_tombstone_retry").unwrap();
        let previous_object = PayloadObject::new("Default/Cookies", b"old-cookie").unwrap();
        let previous_payload = PayloadDescriptor::new(
            workspace.clone(),
            resource.clone(),
            1,
            vec![previous_object.clone()],
        )
        .unwrap();
        let previous = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            &previous_payload.canonical_root_bytes(),
            vec![ManifestObject {
                key: previous_object.path,
                size: previous_object.size,
                checksum: previous_object.checksum,
                source_generation: None,
            }],
        )
        .unwrap();
        let profile_path = root.join("browser-profile");
        fs::create_dir_all(&profile_path).unwrap();
        fs::write(profile_path.join("Preferences"), b"updated").unwrap();
        let client_key = ClientKey::from_bytes([0x71; 32]);
        let recovery_key = RecoveryKey::from_bytes([0x72; 32]);
        let snapshot = || {
            projection
                .snapshot_with_proxy_encryption(
                    &profile_path,
                    &previous,
                    2,
                    None,
                    &client_key,
                    &recovery_key,
                )
                .unwrap()
        };

        let (_, _, first) = snapshot();
        let (_, _, retry) = snapshot();

        assert_eq!(first.tombstones, retry.tombstones);
        assert_eq!(first.root_checksum, retry.root_checksum);
        assert_eq!(first.generation, retry.generation);
        let cache_root = root.join(".sync-cache");
        let cache_path = fs::read_dir(cache_root)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .flat_map(|scope| {
                fs::read_dir(scope)
                    .unwrap()
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .collect::<Vec<_>>()
            })
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("tombstones-"))
            })
            .unwrap();
        fs::write(&cache_path, b"corrupt tombstone cache").unwrap();
        let corrupted = projection
            .snapshot_with_proxy_encryption(
                &profile_path,
                &previous,
                2,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap_err();
        assert_eq!(corrupted.code, "profile_snapshot_cache_invalid");
        drop(projection);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn newer_snapshot_keeps_older_staged_upload_immutable() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-immutable-snapshot-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_immutable_snapshot").unwrap();
        let resource = ResourceId::new("res_immutable_snapshot").unwrap();
        let profile_path = root.join("browser-profile");
        fs::create_dir_all(&profile_path).unwrap();
        let source = profile_path.join("Preferences");
        fs::write(&source, b"first generation").unwrap();
        let previous = SyncManifest::new(workspace, resource, 1, 1, None, b"", Vec::new()).unwrap();
        let client_key = ClientKey::from_bytes([0x31; 32]);
        let recovery_key = RecoveryKey::from_bytes([0x42; 32]);
        let (_, first_files, _) = projection
            .snapshot_with_proxy_encryption(
                &profile_path,
                &previous,
                2,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap();
        let first_blob = first_files
            .iter()
            .find(|blob| blob.key == "Preferences")
            .unwrap()
            .clone();
        let first_ciphertext = first_blob.read_bytes().unwrap();

        fs::write(&source, b"second generation").unwrap();
        let (_, _, _) = projection
            .snapshot_with_proxy_encryption(
                &profile_path,
                &previous,
                2,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap();

        assert_eq!(first_blob.read_bytes().unwrap(), first_ciphertext);
        assert_eq!(
            AuthenticatedChunk::decode(&first_ciphertext)
                .unwrap()
                .decrypt(
                    &client_key,
                    &profile_object_aad_v2(
                        &previous.workspace_id,
                        &previous.resource_id,
                        "Preferences"
                    ),
                )
                .unwrap(),
            b"first generation"
        );
        drop(projection);
        let _ = fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn large_profile_file_is_split_into_authenticated_four_mib_chunks() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-chunked-file-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_chunked_projection").unwrap();
        let resource = ResourceId::new("res_chunked_projection").unwrap();
        let profile_path = root.join("browser-profile");
        fs::create_dir_all(profile_path.join("Default")).unwrap();
        let plaintext = vec![0x5a; (MAX_PROFILE_PLAINTEXT_BYTES + 1) as usize];
        fs::write(profile_path.join("Default/Cookies"), &plaintext).unwrap();
        let previous = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            b"",
            Vec::new(),
        )
        .unwrap();
        let client_key = ClientKey::from_bytes([0x37; 32]);
        let recovery_key = RecoveryKey::from_bytes([0x48; 32]);
        let (descriptor, objects, manifest) = projection
            .snapshot_with_proxy_encryption(
                &profile_path,
                &previous,
                2,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap();

        assert_eq!(manifest.schema_version, 3);
        assert_eq!(manifest.file_chunks.len(), 1);
        let file = &manifest.file_chunks[0];
        assert_eq!(file.path, "Default/Cookies");
        assert_eq!(file.plaintext_size, plaintext.len() as u64);
        assert_eq!(file.plaintext_checksum, checksum(&plaintext));
        assert!(file.chunks.len() > 1);
        assert!(
            file.chunks
                .iter()
                .all(|chunk| chunk.object.size <= MAX_PROFILE_SYNC_OBJECT_BYTES)
        );
        manifest.validate().unwrap();
        descriptor.verify_manifest(&manifest).unwrap();

        let (next_descriptor, next_objects, next_manifest) = projection
            .snapshot_with_proxy_encryption(
                &profile_path,
                &manifest,
                2,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap();
        next_descriptor.verify_manifest(&next_manifest).unwrap();
        let (retry_descriptor, retry_objects, retry_manifest) = projection
            .snapshot_with_proxy_encryption(
                &profile_path,
                &manifest,
                2,
                None,
                &client_key,
                &recovery_key,
            )
            .unwrap();
        retry_descriptor.verify_manifest(&retry_manifest).unwrap();
        assert_eq!(
            retry_descriptor.root_checksum,
            next_descriptor.root_checksum
        );
        assert!(retry_manifest.file_chunks[0].chunks.iter().all(|chunk| {
            let retry_object = retry_manifest
                .objects
                .iter()
                .find(|object| object.key == chunk.object.key)
                .unwrap();
            let first_object = next_manifest
                .objects
                .iter()
                .find(|object| object.key == chunk.object.key)
                .unwrap();
            retry_object.source_generation == first_object.source_generation
        }));
        assert!(
            retry_objects
                .iter()
                .all(|blob| next_objects.iter().any(|prior| prior.key == blob.key))
        );

        let mut restored = Vec::with_capacity(plaintext.len());
        for chunk in &file.chunks {
            let encrypted = objects
                .iter()
                .find(|blob| blob.key == chunk.object.key)
                .unwrap()
                .read_bytes()
                .unwrap();
            restored.extend_from_slice(
                &AuthenticatedChunk::decode(&encrypted)
                    .unwrap()
                    .decrypt(
                        &client_key,
                        &profile_file_chunk_aad(
                            &workspace,
                            &resource,
                            &file.path,
                            chunk.index,
                            &chunk.plaintext_checksum,
                        ),
                    )
                    .unwrap(),
            );
        }
        assert_eq!(restored, plaintext);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let remote_objects: BTreeMap<_, _> = objects
            .iter()
            .map(|blob| (blob.key.clone(), blob.read_bytes().unwrap()))
            .collect();
        let interrupt_next_chunk = Arc::new(AtomicBool::new(false));
        let server_interrupt_next_chunk = Arc::clone(&interrupt_next_chunk);
        let corrupt_next_chunk = Arc::new(AtomicBool::new(false));
        let server_corrupt_next_chunk = Arc::clone(&corrupt_next_chunk);
        let (shutdown_server, mut shutdown_server_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let mut paths = Vec::new();
            loop {
                let accepted = tokio::select! {
                    _ = &mut shutdown_server_rx => break,
                    accepted = listener.accept() => accepted,
                };
                let Ok((mut stream, _)) = accepted else {
                    break;
                };
                let request = read_http_request(&mut stream).await;
                let path = request
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or_default()
                    .to_owned();
                let object_key = remote_objects
                    .keys()
                    .map(String::as_str)
                    .find(|key| path.contains(&key.replace('/', "%2F")))
                    .unwrap_or_else(|| panic!("unexpected blob request: {path}"));
                let mut body = remote_objects.get(object_key).unwrap().clone();
                if object_key.starts_with(".buglogin/file-chunks/")
                    && server_interrupt_next_chunk.swap(false, Ordering::SeqCst)
                {
                    let response = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    // This response is intentionally truncated. The client may
                    // close as soon as it detects the short body, so a broken
                    // pipe here is part of the scenario rather than a server
                    // failure that should kill the retry fixture.
                    let _ = stream.write_all(response.as_bytes()).await;
                    let _ = stream.write_all(&body[..body.len() / 2]).await;
                    paths.push(path);
                    continue;
                }
                if object_key.starts_with(".buglogin/file-chunks/")
                    && server_corrupt_next_chunk.swap(false, Ordering::SeqCst)
                {
                    body[0] ^= 1;
                }
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                paths.push(path);
                // A failed chunk cancels sibling downloads. Their client
                // sockets may already be closed when this fixture writes a
                // full response, so a broken pipe is expected here too.
                if stream.write_all(response.as_bytes()).await.is_err() {
                    continue;
                }
                let _ = stream.write_all(&body).await;
            }
            paths
        });
        let client =
            RemoteApiClient::new(&format!("http://{address}"), "device-token".into()).unwrap();
        let auth = CoordinationAuth::new("desktop-chunk-test", "lease-token").unwrap();
        interrupt_next_chunk.store(true, Ordering::SeqCst);
        let retry_profile = ProfileId::new("prf_chunked_retry_hydration").unwrap();
        let interrupted_result = projection
            .hydrate_encrypted_with_proxy_credentials(
                &client,
                &auth,
                &retry_profile,
                &manifest,
                Some(&client_key),
                Some(&recovery_key),
                "chunked-interrupted-hydration-test",
            )
            .await;
        assert!(interrupted_result.is_err());
        assert!(
            !decrypted_generation_path(&root, &retry_profile, &manifest)
                .unwrap()
                .exists()
        );
        let retry_staging = root.join(retry_profile.as_str()).join("staging");
        if retry_staging.exists() {
            assert_eq!(fs::read_dir(&retry_staging).unwrap().count(), 0);
        }
        let (retry_receipt, _) = projection
            .hydrate_encrypted_with_proxy_credentials(
                &client,
                &auth,
                &retry_profile,
                &manifest,
                Some(&client_key),
                Some(&recovery_key),
                "chunked-hydration-retry-test",
            )
            .await
            .unwrap();
        assert_eq!(
            fs::read(retry_receipt.path.join("Default/Cookies")).unwrap(),
            plaintext
        );
        corrupt_next_chunk.store(true, Ordering::SeqCst);
        let corrupt_profile = ProfileId::new("prf_chunked_corrupt_hydration").unwrap();
        let corrupt_result = projection
            .hydrate_encrypted_with_proxy_credentials(
                &client,
                &auth,
                &corrupt_profile,
                &manifest,
                Some(&client_key),
                Some(&recovery_key),
                "chunked-corrupt-hydration-test",
            )
            .await;
        assert_eq!(
            corrupt_result.err().unwrap().code,
            "api_blob_integrity_failed"
        );
        assert!(
            !decrypted_generation_path(&root, &corrupt_profile, &manifest)
                .unwrap()
                .exists()
        );
        let corrupt_staging = root.join(corrupt_profile.as_str()).join("staging");
        if corrupt_staging.exists() {
            assert_eq!(fs::read_dir(corrupt_staging).unwrap().count(), 0);
        }
        let _ = shutdown_server.send(());
        let requested = server.await.unwrap();
        for chunk in &file.chunks {
            let encoded_key = chunk.object.key.replace('/', "%2F");
            assert!(
                requested
                    .iter()
                    .filter(|path| path.contains(&encoded_key))
                    .count()
                    >= 1,
                "successful hydration must request every manifest chunk"
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn encrypted_snapshot_seals_proxy_credentials_as_manifest_dependency_objects() {
        let root = std::env::temp_dir().join(format!(
            "buglogin-projection-proxy-secret-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let projection = ProfileProjection::open(&root).unwrap();
        let workspace = WorkspaceId::new("ws_proxy_secret").unwrap();
        let resource = ResourceId::new("res_proxy_secret").unwrap();
        let proxy_id = ProxyId::new("prx_proxy_secret").unwrap();
        let profile_path = root.join("browser-profile");
        fs::create_dir_all(&profile_path).unwrap();
        fs::write(profile_path.join("Preferences"), b"profile").unwrap();
        let previous = SyncManifest::new(
            workspace.clone(),
            resource.clone(),
            1,
            1,
            None,
            b"",
            Vec::new(),
        )
        .unwrap();
        let client_key = ClientKey::from_bytes([0x29; 32]);
        let recovery_key = RecoveryKey::from_bytes([0x39; 32]);
        let clear_secret = br#"{"username":"proxy-user","password":"proxy-pass"}"#;

        let (descriptor, files, manifest) = projection
            .snapshot_with_proxy_encryption(
                &profile_path,
                &previous,
                2,
                Some(RemoteProxySyncSnapshot {
                    proxy_id: proxy_id.clone(),
                    revision: 5,
                    credential_version: Some(
                        buglogin_domain::ids::CredentialId::new("credential-version").unwrap(),
                    ),
                    credential: Some(SecretValue::new(clear_secret.to_vec()).unwrap()),
                }),
                &client_key,
                &recovery_key,
            )
            .unwrap();

        let ManifestDependency::Proxy {
            credential_envelope_object: Some(reference),
            proxy_id: manifest_proxy,
            revision,
            ..
        } = &manifest.dependencies[0]
        else {
            panic!("proxy credential envelope reference missing")
        };
        assert_eq!(manifest_proxy, &proxy_id);
        assert_eq!(*revision, 5);
        let object = files.iter().find(|blob| blob.key == reference.key).unwrap();
        let object_bytes = object.read_bytes().unwrap();
        assert_eq!(object_bytes.len() as u64, reference.size);
        let cache_scope = checksum(
            format!(
                "{}/{}/{}",
                workspace.as_str(),
                resource.as_str(),
                checksum(client_key.as_bytes())
            )
            .as_bytes(),
        );
        let proxy_cache_alias = format!(
            "proxy-{}-{}-envelope.json",
            checksum(proxy_id.as_str().as_bytes()),
            revision
        );
        assert_eq!(
            read_cached_manifest_object_bytes(
                &root,
                &cache_scope,
                reference,
                Some(&proxy_cache_alias)
            )
            .unwrap(),
            Some(object_bytes.clone())
        );
        assert!(
            !object_bytes
                .windows(10)
                .any(|window| window == b"proxy-pass")
        );
        let envelope: ProxyCredentialEnvelope = serde_json::from_slice(&object_bytes).unwrap();
        assert_eq!(
            envelope
                .open(&workspace, &resource, &proxy_id, 5, &client_key)
                .unwrap(),
            clear_secret
        );
        manifest.validate().unwrap();
        descriptor.verify_manifest(&manifest).unwrap();
        let (_, retry_files, retry_manifest) = projection
            .snapshot_with_proxy_encryption(
                &profile_path,
                &previous,
                2,
                Some(RemoteProxySyncSnapshot {
                    proxy_id: proxy_id.clone(),
                    revision: 5,
                    credential_version: Some(
                        buglogin_domain::ids::CredentialId::new("credential-version").unwrap(),
                    ),
                    credential: Some(SecretValue::new(clear_secret.to_vec()).unwrap()),
                }),
                &client_key,
                &recovery_key,
            )
            .unwrap();
        let retry_reference = retry_manifest
            .dependencies
            .iter()
            .find_map(|dependency| match dependency {
                ManifestDependency::Proxy {
                    credential_envelope_object: Some(reference),
                    ..
                } => Some(reference),
                _ => None,
            })
            .unwrap();
        let retry_object = retry_files
            .iter()
            .find(|blob| blob.key == retry_reference.key)
            .unwrap();
        assert_eq!(retry_object.checksum, reference.checksum);
        let _ = fs::remove_dir_all(root);
    }
}
