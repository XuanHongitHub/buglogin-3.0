use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use crate::clock::{checked_add_millis, unix_time_millis};
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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::remote_api::{RemoteApiClient, RemoteProxySyncSnapshot};

const MARKER: &str = ".buglogin-projection.json";
const ACTIVE_SESSION_MARKER: &str = ".buglogin-active-session.json";
const SYNC_COMMIT_MARKER: &str = ".buglogin-sync-commit.json";
const MAX_ENCRYPTED_OBJECT_BYTES: usize = MAX_PROFILE_SYNC_OBJECT_BYTES as usize;
const MAX_PROFILE_PLAINTEXT_BYTES: u64 =
    MAX_PROFILE_SYNC_OBJECT_BYTES - AUTHENTICATED_CHUNK_OVERHEAD_BYTES as u64;

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

impl SnapshotBlob {
    pub fn read_bytes(&self) -> Result<Vec<u8>, Problem> {
        let bytes = fs::read(&self.path).map_err(|_| unavailable())?;
        if bytes.len() as u64 != self.size || checksum(&bytes) != self.checksum {
            return Err(problem(
                "profile_snapshot_blob_corrupt",
                "Staged profile snapshot object failed its checksum",
            ));
        }
        Ok(bytes)
    }
}

#[derive(Clone)]
pub struct ProfileProjection {
    root: PathBuf,
    _root_lock: Arc<fs::File>,
}

impl ProfileProjection {
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn open(root: impl Into<PathBuf>) -> Result<Self, Problem> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|_| unavailable())?;
        let root = root.canonicalize().map_err(|_| unavailable())?;
        let lock_path = confined_path(&root, Path::new(".projection.lock"))?;
        let root_lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|_| unavailable())?;
        root_lock.try_lock_exclusive().map_err(|error| {
            if is_lock_contention(&error) {
                Problem::new(
                    "profile_projection_in_use",
                    "Profile data is already open by another BugLogin process",
                    409,
                    "desktop-projection",
                )
            } else {
                unavailable()
            }
        })?;
        cleanup_abandoned_staging(&root)?;
        Ok(Self {
            root,
            _root_lock: Arc::new(root_lock),
        })
    }

    pub fn dirty_local_generation(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
    ) -> Result<Option<PathBuf>, Problem> {
        let path = decrypted_generation_path(&self.root, profile_id, manifest)?;
        if !path.exists() {
            return Ok(None);
        }
        if verify_decrypted_generation(profile_id, manifest, &path).is_ok() {
            Ok(None)
        } else {
            Ok(Some(path))
        }
    }

    pub fn existing_local_generation(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
    ) -> Result<Option<PathBuf>, Problem> {
        let path = decrypted_generation_path(&self.root, profile_id, manifest)?;
        Ok(path.exists().then_some(path))
    }

    /// Stages a verified legacy browser tree as dirty local profile data for
    /// the ordinary encrypted sync commit. The legacy bytes never become a
    /// trusted projection generation or a server-readable manifest.
    pub fn stage_legacy_profile_data(
        &self,
        profile_id: &ProfileId,
        genesis: &SyncManifest,
        source_root: &Path,
    ) -> Result<PathBuf, Problem> {
        genesis.validate()?;
        if genesis.generation != 1
            || genesis.parent_generation.is_some()
            || genesis.payload_encryption.is_some()
            || !genesis.objects.is_empty()
            || !genesis.file_chunks.is_empty()
        {
            return Err(problem(
                "migration_profile_genesis_required",
                "Legacy profile data can only be staged against an empty genesis manifest",
            ));
        }
        let source_metadata = fs::symlink_metadata(source_root).map_err(|_| {
            problem(
                "migration_profile_source_invalid",
                "Legacy profile source directory is unavailable",
            )
        })?;
        if !source_metadata.is_dir() || source_metadata.file_type().is_symlink() {
            return Err(problem(
                "migration_profile_source_invalid",
                "Legacy profile source directory is invalid",
            ));
        }
        let source_root = source_root.canonicalize().map_err(|_| {
            problem(
                "migration_profile_source_invalid",
                "Legacy profile source directory is unavailable",
            )
        })?;
        if source_root.starts_with(&self.root) {
            return Err(problem(
                "migration_profile_source_invalid",
                "Legacy profile source directory is invalid",
            ));
        }
        let mut files = Vec::new();
        collect_file_paths(&source_root, &source_root, &mut files)?;
        files.sort_by(|left, right| left.0.cmp(&right.0));
        if files.is_empty() {
            return Err(problem(
                "migration_profile_source_empty",
                "Legacy profile data is empty",
            ));
        }

        let destination = decrypted_generation_path(&self.root, profile_id, genesis)?;
        if destination.exists() {
            if destination.join(MARKER).exists() {
                return Err(problem(
                    "migration_profile_already_staged",
                    "A trusted profile generation already exists; preserve it before retrying migration",
                ));
            }
            let mut existing_files = Vec::new();
            collect_file_paths(&destination, &destination, &mut existing_files)?;
            existing_files.sort_by(|left, right| left.0.cmp(&right.0));
            if existing_files.len() != files.len() {
                return Err(problem(
                    "migration_profile_stage_conflict",
                    "Existing staged profile data differs from the migration source",
                ));
            }
            for (
                (expected_path, expected_source, expected_size),
                (actual_path, actual_source, actual_size),
            ) in files.iter().zip(existing_files.iter())
            {
                if expected_path != actual_path || expected_size != actual_size {
                    return Err(problem(
                        "migration_profile_stage_conflict",
                        "Existing staged profile data differs from the migration source",
                    ));
                }
                if checksum_file(expected_source)? != checksum_file(actual_source)? {
                    return Err(problem(
                        "migration_profile_stage_conflict",
                        "Existing staged profile data differs from the migration source",
                    ));
                }
            }
            return Ok(destination);
        }
        let profile_root = confined_path(&self.root, Path::new(profile_id.as_str()))?;
        fs::create_dir_all(&profile_root).map_err(|_| unavailable())?;
        let staging_root = profile_root.join("staging");
        fs::create_dir_all(&staging_root).map_err(|_| unavailable())?;
        let staging_relative = PathBuf::from(profile_id.as_str())
            .join("staging")
            .join(format!("legacy-import-{}", unique_suffix()));
        let staging = confined_path(&self.root, &staging_relative)?;
        fs::create_dir(&staging).map_err(|_| unavailable())?;
        let stage_result = (|| {
            for (relative, source, expected_size) in &files {
                let source_metadata = fs::symlink_metadata(source).map_err(|_| unavailable())?;
                if source_metadata.file_type().is_symlink()
                    || !source_metadata.is_file()
                    || source_metadata.len() != *expected_size
                {
                    return Err(problem(
                        "migration_profile_source_changed",
                        "Legacy profile data changed while it was being staged",
                    ));
                }
                copy_profile_file_verified(source, &staging, relative, *expected_size)?;
            }
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent).map_err(|_| unavailable())?;
            }
            fs::rename(&staging, &destination).map_err(|_| unavailable())?;
            Ok(destination.clone())
        })();
        if stage_result.is_err() {
            let _ = fs::remove_dir_all(&staging);
        }
        stage_result
    }

    pub fn active_session_recovery_path(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
    ) -> Result<Option<PathBuf>, Problem> {
        let commit_relative = PathBuf::from(profile_id.as_str()).join(SYNC_COMMIT_MARKER);
        let commit_path = confined_path(&self.root, &commit_relative)?;
        let commit_bytes = match fs::read(&commit_path) {
            Ok(bytes) => Some(bytes),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(unavailable()),
        };
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
                self.clear_active_session(profile_id)?;
                return Ok(None);
            }
            if manifest.generation != marker.base_generation
                || manifest.root_checksum != marker.base_root_checksum
            {
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
            if !generation_path.exists() {
                return Err(problem(
                    "profile_local_generation_missing",
                    "A pending local profile sync was recorded, but its profile data is missing",
                ));
            }
            if verify_decrypted_generation(profile_id, manifest, &generation_path).is_ok() {
                self.clear_active_session(profile_id)?;
                return Ok(None);
            }
            return Ok(Some(generation_path));
        }
        let relative = PathBuf::from(profile_id.as_str()).join(ACTIVE_SESSION_MARKER);
        let marker_path = confined_path(&self.root, &relative)?;
        let bytes = match fs::read(&marker_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable()),
        };
        let marker: ActiveSessionMarker = serde_json::from_slice(&bytes).map_err(|_| {
            problem(
                "profile_session_marker_invalid",
                "The local profile session marker is invalid; preserve the profile data and contact support",
            )
        })?;
        if marker.state != "active"
            || marker.profile_id != *profile_id
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
            return Err(problem(
                "profile_local_sync_conflict",
                "This profile changed on another device while local browser changes were pending. Local data is preserved; resolve the sync conflict before launching",
            ));
        }
        let generation_path = decrypted_generation_path(&self.root, profile_id, manifest)?;
        if !generation_path.exists() {
            return Err(problem(
                "profile_local_generation_missing",
                "A pending local profile session was recorded, but its profile data is missing",
            ));
        }
        if verify_decrypted_generation(profile_id, manifest, &generation_path).is_ok() {
            self.clear_active_session(profile_id)?;
            Ok(None)
        } else {
            Ok(Some(generation_path))
        }
    }

    pub fn begin_active_session(
        &self,
        profile_id: &ProfileId,
        generation: u64,
        root_checksum: &str,
    ) -> Result<(), Problem> {
        if !valid_manifest_checksum(root_checksum) {
            return Err(problem(
                "profile_session_marker_invalid",
                "The active profile session has an invalid root checksum",
            ));
        }
        let profile_root = confined_path(&self.root, Path::new(profile_id.as_str()))?;
        fs::create_dir_all(&profile_root).map_err(|_| unavailable())?;
        let relative = PathBuf::from(profile_id.as_str()).join(ACTIVE_SESSION_MARKER);
        let marker_path = confined_path(&self.root, &relative)?;
        if marker_path.exists() {
            return Err(problem(
                "profile_session_already_active",
                "A local profile session is already marked active",
            ));
        }
        let temporary_relative = PathBuf::from(profile_id.as_str())
            .join(format!("{ACTIVE_SESSION_MARKER}.{}", unique_suffix()));
        let temporary = confined_path(&self.root, &temporary_relative)?;
        let marker = ActiveSessionMarker {
            state: "active".to_owned(),
            profile_id: profile_id.clone(),
            generation,
            root_checksum: root_checksum.to_owned(),
        };
        let bytes = serde_json::to_vec(&marker).map_err(|_| unavailable())?;
        let write_result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|_| unavailable())?;
            file.write_all(&bytes).map_err(|_| unavailable())?;
            file.sync_all().map_err(|_| unavailable())?;
            fs::rename(&temporary, &marker_path).map_err(|_| unavailable())
        })();
        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }

    pub fn clear_active_session(&self, profile_id: &ProfileId) -> Result<(), Problem> {
        for name in [ACTIVE_SESSION_MARKER, SYNC_COMMIT_MARKER] {
            let relative = PathBuf::from(profile_id.as_str()).join(name);
            let marker_path = confined_path(&self.root, &relative)?;
            match fs::remove_file(marker_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(unavailable()),
            }
        }
        Ok(())
    }

    pub fn prepare_sync_commit(
        &self,
        profile_id: &ProfileId,
        base_manifest: &SyncManifest,
        target_manifest: &SyncManifest,
    ) -> Result<(), Problem> {
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
            || !valid_manifest_checksum(&base_manifest.root_checksum)
            || !valid_manifest_checksum(&target_manifest.root_checksum)
        {
            return Err(problem(
                "profile_sync_commit_marker_invalid",
                "The profile sync commit does not follow the active manifest generation",
            ));
        }
        let profile_root = confined_path(&self.root, Path::new(profile_id.as_str()))?;
        fs::create_dir_all(&profile_root).map_err(|_| unavailable())?;
        let marker = SyncCommitMarker {
            profile_id: profile_id.clone(),
            base_generation: base_manifest.generation,
            base_root_checksum: base_manifest.root_checksum.clone(),
            target_generation: target_manifest.generation,
            target_root_checksum: target_manifest.root_checksum.clone(),
        };
        let bytes = serde_json::to_vec(&marker).map_err(|_| unavailable())?;
        let relative = PathBuf::from(profile_id.as_str()).join(SYNC_COMMIT_MARKER);
        let marker_path = confined_path(&self.root, &relative)?;
        let temporary_relative = PathBuf::from(profile_id.as_str())
            .join(format!("{SYNC_COMMIT_MARKER}.{}", unique_suffix()));
        let temporary = confined_path(&self.root, &temporary_relative)?;
        let write_result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|_| unavailable())?;
            file.write_all(&bytes).map_err(|_| unavailable())?;
            file.sync_all().map_err(|_| unavailable())?;
            // `rename` replaces the old marker atomically on the same filesystem.
            // Removing it first creates a crash window where neither commit record exists.
            fs::rename(&temporary, &marker_path).map_err(|_| unavailable())
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
        manifest.validate()?;
        if manifest.objects.is_empty() {
            let descriptor = descriptor_from_manifest(manifest)?;
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
        let generation_path = decrypted_generation_path(&self.root, profile_id, manifest)?;
        let generation_exists = generation_path.exists();
        let cached_generation = generation_exists
            && verify_decrypted_generation(profile_id, manifest, &generation_path).is_ok();
        if generation_exists && !cached_generation {
            return Err(problem(
                "profile_local_changes_recovery_required",
                "This device has local profile data that differs from the last synced generation. Preserve the local profile data and retry synchronization before launching it again.",
            ));
        }
        let clear_metadata = proxy_metadata_keys(manifest);
        let proxy_envelopes = proxy_credential_keys(manifest);
        for object in remote_descriptor
            .objects
            .iter()
            .filter(|object| clear_metadata.contains(&object.path))
        {
            download_verified_manifest_object(
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
        }
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
        let envelope_bytes = download_verified_manifest_object(
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
        let envelope: ResourceKeyEnvelope =
            serde_json::from_slice(&envelope_bytes).map_err(|_| {
                problem(
                    "resource_key_envelope_invalid",
                    "Resource key envelope is invalid",
                )
            })?;
        if !matches_manifest_object(
            &ManifestObject::new(RESOURCE_KEY_ENVELOPE_KEY, &envelope_bytes)?,
            &encryption.resource_key_envelope,
        ) {
            return Err(problem(
                "resource_key_envelope_mismatch",
                "Resource key envelope does not match the manifest",
            ));
        }
        let recovery_key = recovery_key.ok_or_else(|| {
            problem(
                "sync_key_recovery_required",
                "Enter this profile's recovery code on this device to open it",
            )
        })?;
        let unwrapped_key =
            envelope.unwrap(&manifest.workspace_id, &manifest.resource_id, recovery_key)?;
        if client_key.is_some_and(|key| key != &unwrapped_key) {
            return Err(problem(
                "workspace_key_vault_mismatch",
                "Stored profile key does not match the active manifest",
            ));
        }
        let key = client_key.unwrap_or(&unwrapped_key);
        if let PayloadEncryptionAlgorithm::Aes256GcmChunkV2 = encryption.algorithm {
            let expected_key_version = checksum(key.as_bytes());
            if encryption.key_version.as_deref() != Some(expected_key_version.as_str()) {
                return Err(problem(
                    "profile_payload_key_version_mismatch",
                    "Profile payload key version does not match the recovery key",
                ));
            }
        }
        let mut credentials = Vec::with_capacity(proxy_envelopes.len());
        for (proxy_id, revision, reference) in &proxy_envelopes {
            let bytes = download_verified_manifest_object(
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
            if !matches_manifest_object(
                &ManifestObject::new(reference.key.clone(), &bytes)?,
                reference,
            ) {
                return Err(problem(
                    "proxy_credential_envelope_mismatch",
                    "Proxy credential envelope does not match the manifest",
                ));
            }
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
            return verify_decrypted_generation(profile_id, manifest, &generation_path)
                .map(|receipt| (receipt, credentials));
        }

        let mut staging = DecryptedGenerationStaging::new(&self.root, profile_id, manifest)?;
        let chunk_object_keys = manifest
            .file_chunks
            .iter()
            .flat_map(|file| file.chunks.iter().map(|chunk| chunk.object.key.as_str()))
            .collect::<std::collections::BTreeSet<_>>();
        for object in &remote_descriptor.objects {
            if should_download_manifest_object(&object.path, &clear_metadata, &proxy_envelopes)
                || chunk_object_keys.contains(object.path.as_str())
            {
                continue;
            }
            let bytes = client
                .download_blob(
                    CoordinationScope {
                        workspace_id: &manifest.workspace_id,
                        auth,
                    },
                    &manifest.resource_id,
                    &object.path,
                    manifest.generation,
                    &object.checksum,
                    object.size,
                    correlation_id,
                )
                .await?;
            if bytes.len() as u64 != object.size || checksum(&bytes) != object.checksum {
                return Err(problem(
                    "profile_object_checksum_mismatch",
                    "Profile object failed its ciphertext checksum",
                ));
            }
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
            for (expected_index, chunk) in file.chunks.iter().enumerate() {
                if chunk.index != expected_index as u64 {
                    return Err(problem(
                        "profile_file_chunks_invalid",
                        "Chunked profile file sequence is invalid",
                    ));
                }
                let encrypted = client
                    .download_blob(
                        CoordinationScope {
                            workspace_id: &manifest.workspace_id,
                            auth,
                        },
                        &manifest.resource_id,
                        &chunk.object.key,
                        manifest.generation,
                        &chunk.object.checksum,
                        chunk.object.size,
                        correlation_id,
                    )
                    .await?;
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
                    .map_err(|_| unavailable())?;
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
        staging
            .publish(profile_id, manifest)
            .map(|receipt| (receipt, credentials))
    }

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
        fs::create_dir_all(&profile_root).map_err(|_| unavailable())?;
        let generation_name = format!(
            "generations/{}-{}",
            descriptor.generation, descriptor.root_checksum
        );
        let generation_path = confined_path(&profile_root, Path::new(&generation_name))?;
        if generation_path.exists() {
            return verify_generation(profile_id, descriptor, &generation_path);
        }
        let staging_name = format!(
            "staging/{}-{}-{}",
            descriptor.generation,
            std::process::id(),
            unique_suffix()
        );
        let staging_path = confined_path(&profile_root, Path::new(&staging_name))?;
        fs::create_dir_all(&staging_path).map_err(|_| unavailable())?;
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
            write_verified(
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
        write_verified(
            &staging_path,
            Path::new(MARKER),
            &marker_bytes,
            &checksum(&marker_bytes),
        )?;
        if let Some(parent) = generation_path.parent() {
            fs::create_dir_all(parent).map_err(|_| unavailable())?;
        }
        fs::rename(&staging_path, &generation_path).map_err(|_| unavailable())?;
        verify_generation(profile_id, descriptor, &generation_path)
    }

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
            .filter(|tombstone| tombstone.retention_deadline_unix_ms > now)
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
        let mut active_cache_files = std::collections::BTreeSet::new();
        for (path, source_path, expected_size) in plaintext_files {
            live_file_paths.insert(path.clone());
            if expected_size > MAX_PROFILE_PLAINTEXT_BYTES {
                let metadata_before = fs::metadata(&source_path).map_err(|_| unavailable())?;
                validate_profile_file_size(metadata_before.len())?;
                if metadata_before.len() != expected_size {
                    return Err(profile_changed_during_snapshot());
                }
                let mut source = fs::File::open(&source_path).map_err(|_| unavailable())?;
                let mut aggregate = Sha256::new();
                let mut chunks = Vec::new();
                let mut remaining = expected_size;
                let mut index = 0usize;
                while remaining > 0 {
                    let chunk_len = remaining.min(PROFILE_FILE_CHUNK_BYTES) as usize;
                    let mut plaintext_chunk = vec![0; chunk_len];
                    source
                        .read_exact(&mut plaintext_chunk)
                        .map_err(|_| profile_changed_during_snapshot())?;
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
                        previous,
                    )?;
                    if let Some(source_generation) = source_generation {
                        reused_sources.insert(blob.key.clone(), source_generation);
                    }
                    active_cache_files.insert(
                        blob.path
                            .file_name()
                            .ok_or_else(unavailable)?
                            .to_string_lossy()
                            .into_owned(),
                    );
                    files.push(blob);
                    chunks.push(chunk);
                    remaining -= chunk_len as u64;
                    index += 1;
                }
                let mut trailing = [0u8; 1];
                if source.read(&mut trailing).map_err(|_| unavailable())? != 0
                    || fs::metadata(&source_path).map_err(|_| unavailable())?.len() != expected_size
                {
                    return Err(profile_changed_during_snapshot());
                }
                file_chunks.push(ManifestFileChunks {
                    path,
                    plaintext_size: expected_size,
                    plaintext_checksum: format!("{:x}", aggregate.finalize()),
                    chunks,
                });
                continue;
            }
            let plaintext = read_profile_file(&source_path)?;
            if plaintext.len() as u64 != expected_size {
                return Err(Problem::new(
                    "profile_changed_during_snapshot",
                    "Profile files changed while the sync snapshot was being prepared",
                    409,
                    "desktop-projection",
                )
                .retryable(true));
            }
            live_file_paths.insert(path.clone());
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
                    active_cache_files.insert(cache_name);
                    let prior_chunk = previous
                        .file_chunks
                        .iter()
                        .find(|file| file.path == path)
                        .and_then(|file| file.chunks.get(index))
                        .filter(|chunk| {
                            chunk.index == index as u64
                                && chunk.plaintext_size == plaintext_chunk.len() as u64
                                && chunk.plaintext_checksum == plaintext_checksum
                        });
                    let cached = (|| {
                        let cache_path = confined_path(&self.root, &cache_path).ok()?;
                        let metadata = fs::metadata(&cache_path).ok()?;
                        if metadata.len() > MAX_ENCRYPTED_OBJECT_BYTES as u64 {
                            return None;
                        }
                        let cached = fs::read(cache_path).ok()?;
                        let decrypted = AuthenticatedChunk::decode(&cached)
                            .ok()?
                            .decrypt(client_key, &chunk_aad)
                            .ok()?;
                        if decrypted != plaintext_chunk {
                            return None;
                        }
                        let source_generation = prior_chunk
                            .and_then(|chunk| previous_objects.get(chunk.object.key.as_str()))
                            .filter(|object| {
                                object.size == cached.len() as u64
                                    && object.checksum == checksum(&cached)
                            })
                            .map(|object| object.source_generation.unwrap_or(previous.generation));
                        Some((cached, source_generation))
                    })();
                    let (encrypted, source_generation) = match cached {
                        Some((cached, source_generation)) => (cached, source_generation),
                        None => {
                            let encrypted = AuthenticatedChunk::encrypt(
                                client_key,
                                plaintext_chunk,
                                &chunk_aad,
                            )?
                            .encoded();
                            let cache_checksum = checksum(&encrypted);
                            write_verified(&self.root, &cache_path, &encrypted, &cache_checksum)
                                .map_err(|_| unavailable())?;
                            (encrypted, None)
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
                    let cache_path =
                        confined_path(&self.root, &cache_path).map_err(|_| unavailable())?;
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
                    "{}/{}/{}/{}",
                    previous.workspace_id.as_str(),
                    previous.resource_id.as_str(),
                    key_version,
                    path
                );
                let cache_name = format!("{}.bin", checksum(cache_identity.as_bytes()));
                let cache_path =
                    PathBuf::from(format!(".sync-cache/{}/{}", cache_scope, cache_name));
                active_cache_files.insert(cache_name);
                let prior = previous_objects.get(path.as_str()).copied();
                let cached = (|| {
                    let cache_path = confined_path(&self.root, &cache_path).ok()?;
                    let metadata = fs::metadata(&cache_path).ok()?;
                    if metadata.len() > MAX_ENCRYPTED_OBJECT_BYTES as u64 {
                        return None;
                    }
                    let cached = fs::read(cache_path).ok()?;
                    let chunk = AuthenticatedChunk::decode(&cached).ok()?;
                    let decrypted = chunk
                        .decrypt(
                            client_key,
                            &profile_object_aad_v2(
                                &previous.workspace_id,
                                &previous.resource_id,
                                &path,
                            ),
                        )
                        .ok()?;
                    if checksum(&decrypted) != plaintext_checksum || decrypted != plaintext {
                        return None;
                    }
                    let source_generation = prior
                        .filter(|prior| {
                            prior.size == cached.len() as u64 && prior.checksum == checksum(&cached)
                        })
                        .map(|prior| prior.source_generation.unwrap_or(previous.generation));
                    Some((cached, source_generation))
                })();
                let (encrypted, source_generation) = match cached {
                    Some((cached, source_generation)) => (cached, source_generation),
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
                        let cache_checksum = checksum(&encrypted);
                        write_verified(&self.root, &cache_path, &encrypted, &cache_checksum)
                            .map_err(|_| unavailable())?;
                        (encrypted, None)
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
                let cache_name = format!(
                    "{}.bin",
                    checksum(
                        format!(
                            "{}/{}/{}/{}",
                            previous.workspace_id.as_str(),
                            previous.resource_id.as_str(),
                            key_version,
                            path
                        )
                        .as_bytes()
                    )
                );
                let cache_path = PathBuf::from(format!(".sync-cache/{cache_scope}/{cache_name}"));
                let cached_path =
                    confined_path(&self.root, &cache_path).map_err(|_| unavailable())?;
                let encrypted_checksum = checksum(&encrypted);
                write_verified(&self.root, &cache_path, &encrypted, &encrypted_checksum)
                    .map_err(|_| unavailable())?;
                files.push(SnapshotBlob {
                    key: path.clone(),
                    path: cached_path,
                    size: encrypted.len() as u64,
                    checksum: encrypted_checksum,
                });
            }
        }
        let envelope = ResourceKeyEnvelope::wrap(
            previous.workspace_id.clone(),
            previous.resource_id.clone(),
            client_key,
            recovery_key,
        )?;
        let envelope_bytes = serde_json::to_vec(&envelope).map_err(|_| unavailable())?;
        files.push(stage_snapshot_metadata(
            &self.root,
            &cache_scope,
            RESOURCE_KEY_ENVELOPE_KEY,
            &envelope_bytes,
        )?);
        active_cache_files.insert(
            files
                .last()
                .and_then(|blob| blob.path.file_name())
                .ok_or_else(unavailable)?
                .to_string_lossy()
                .into_owned(),
        );

        if let Some((proxy_id, revision, secret)) = proxy_credential {
            let proxy_envelope = ProxyCredentialEnvelope::seal(
                previous.workspace_id.clone(),
                previous.resource_id.clone(),
                proxy_id.clone(),
                revision,
                client_key,
                secret.as_bytes(),
            )?;
            let proxy_envelope_bytes =
                serde_json::to_vec(&proxy_envelope).map_err(|_| unavailable())?;
            let proxy_envelope_key = format!(
                ".buglogin/proxy-dependencies/{}/{}/credential-envelope.json",
                proxy_id.as_str(),
                revision
            );
            files.push(stage_snapshot_metadata(
                &self.root,
                &cache_scope,
                &proxy_envelope_key,
                &proxy_envelope_bytes,
            )?);
            active_cache_files.insert(
                files
                    .last()
                    .and_then(|blob| blob.path.file_name())
                    .ok_or_else(unavailable)?
                    .to_string_lossy()
                    .into_owned(),
            );
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
        let now = unix_time_millis("profile_sync_snapshot")?;
        let mut tombstones = previous
            .tombstones
            .iter()
            .filter(|tombstone| tombstone.retention_deadline_unix_ms > now)
            .cloned()
            .collect::<Vec<_>>();
        for object in &previous.objects {
            if !live_object_keys.contains(object.key.as_str())
                && (!live_file_paths.contains(&object.key)
                    || descriptor
                        .file_chunks
                        .iter()
                        .any(|file| file.path == object.key))
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
        let cache_directory = PathBuf::from(format!(".sync-cache/{cache_scope}"));
        if let Ok(directory) = confined_path(&self.root, &cache_directory) {
            if let Ok(entries) = fs::read_dir(directory) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !active_cache_files.contains(&name)
                        && entry.file_type().is_ok_and(|kind| kind.is_file())
                    {
                        let _ = fs::remove_file(entry.path());
                    }
                }
            }
        }
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
        fs::create_dir_all(&profile_root).map_err(|_| unavailable())?;
        let generation_path = decrypted_generation_path(root, profile_id, manifest)?;
        let staging_name = format!(
            "staging/{}-{}-{}",
            manifest.generation,
            std::process::id(),
            unique_suffix()
        );
        let staging_path = confined_path(&profile_root, Path::new(&staging_name))?;
        fs::create_dir_all(&staging_path).map_err(|_| unavailable())?;
        Ok(Self {
            staging_path,
            generation_path,
            local_objects: Vec::new(),
            published: false,
        })
    }

    fn write_object(&mut self, path: &str, bytes: &[u8]) -> Result<(), Problem> {
        let object = PayloadObject::new(path.to_owned(), bytes)?;
        write_verified(&self.staging_path, Path::new(path), bytes, &object.checksum)?;
        self.local_objects.push(object);
        Ok(())
    }

    fn create_file(&self, path: &str) -> Result<(fs::File, PathBuf), Problem> {
        let destination = confined_path(&self.staging_path, Path::new(path))?;
        let parent = destination.parent().ok_or_else(unavailable)?;
        fs::create_dir_all(parent).map_err(|_| unavailable())?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&destination)
            .map_err(|_| unavailable())?;
        Ok((file, destination))
    }

    fn finish_file(
        &mut self,
        path: &str,
        file: fs::File,
        size: u64,
        checksum: String,
    ) -> Result<(), Problem> {
        file.sync_all().map_err(|_| unavailable())?;
        let destination = confined_path(&self.staging_path, Path::new(path))?;
        if fs::metadata(destination).map_err(|_| unavailable())?.len() != size {
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
        write_verified(
            &self.staging_path,
            Path::new(MARKER),
            &marker_bytes,
            &checksum(&marker_bytes),
        )?;
        if let Some(parent) = self.generation_path.parent() {
            fs::create_dir_all(parent).map_err(|_| unavailable())?;
        }
        if self.generation_path.exists() {
            return verify_decrypted_generation(profile_id, manifest, &self.generation_path);
        }
        fs::rename(&self.staging_path, &self.generation_path).map_err(|_| unavailable())?;
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
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncCommitMarker {
    profile_id: ProfileId,
    base_generation: u64,
    base_root_checksum: String,
    target_generation: u64,
    target_root_checksum: String,
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
    let marker_bytes = fs::read(generation_path.join(MARKER)).map_err(|_| {
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

fn verify_staged_file(root: &Path, object: &PayloadObject) -> Result<(), Problem> {
    let path = confined_path(root, Path::new(&object.path))?;
    let metadata = fs::symlink_metadata(&path).map_err(|_| unavailable())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != object.size {
        return Err(problem(
            "profile_projection_file_invalid",
            "Profile projection file failed verification",
        ));
    }
    let mut file = fs::File::open(path).map_err(|_| unavailable())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|_| unavailable())?;
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

fn valid_manifest_checksum(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn matches_manifest_object(object: &ManifestObject, reference: &ManifestObjectReference) -> bool {
    object.key == reference.key
        && object.size == reference.size
        && object.checksum == reference.checksum
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
) -> Result<Vec<u8>, Problem> {
    let scope = CoordinationScope { workspace_id, auth };
    if repair_stale_proxy {
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
    }
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
    let marker_bytes = fs::read(&marker_path).map_err(|_| {
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

fn collect_files(
    root: &Path,
    current: &Path,
    output: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), Problem> {
    let entries = fs::read_dir(current).map_err(|_| unavailable())?;
    for entry in entries {
        let entry = entry.map_err(|_| unavailable())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| unavailable())?;
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
            let relative = path.strip_prefix(root).map_err(|_| unavailable())?;
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
            .map_err(|_| unavailable())?
            .to_string_lossy()
            .replace('\\', "/");
        if is_ephemeral_profile_file(&relative) {
            continue;
        }
        validate_profile_file_size(metadata.len())?;
        output.push((relative, read_profile_file(&path)?));
    }
    Ok(())
}

fn collect_file_paths(
    root: &Path,
    current: &Path,
    output: &mut Vec<(String, PathBuf, u64)>,
) -> Result<(), Problem> {
    let entries = fs::read_dir(current).map_err(|_| unavailable())?;
    for entry in entries {
        let entry = entry.map_err(|_| unavailable())?;
        let path = entry.path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| unavailable())?;
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
            let relative = path.strip_prefix(root).map_err(|_| unavailable())?;
            if is_ephemeral_profile_directory(relative) {
                continue;
            }
            collect_file_paths(root, &path, output)?;
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
            .map_err(|_| unavailable())?
            .to_string_lossy()
            .replace('\\', "/");
        if is_ephemeral_profile_file(&relative) {
            continue;
        }
        validate_profile_file_size(metadata.len())?;
        output.push((relative, path, metadata.len()));
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
    let entries = fs::read_dir(root).map_err(|_| unavailable())?;
    for entry in entries {
        let entry = entry.map_err(|_| unavailable())?;
        let profile_root = entry.path();
        let file_type = entry.file_type().map_err(|_| unavailable())?;
        if !file_type.is_dir()
            || file_type.is_symlink()
            || ProfileId::new(entry.file_name().to_string_lossy().as_ref()).is_err()
        {
            continue;
        }
        let staging_path = profile_root.join("staging");
        let Ok(staging_type) = fs::symlink_metadata(&staging_path) else {
            continue;
        };
        if staging_type.is_dir() && !staging_type.file_type().is_symlink() {
            fs::remove_dir_all(staging_path).map_err(|_| unavailable())?;
        }
    }
    Ok(())
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
    let path = confined_path(root, &relative_path).map_err(|_| unavailable())?;
    write_verified(root, &relative_path, bytes, &object_checksum).map_err(|_| unavailable())?;
    Ok(SnapshotBlob {
        key: key.to_owned(),
        path,
        size: bytes.len() as u64,
        checksum: object_checksum,
    })
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
    previous: &SyncManifest,
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
    let cached = (|| {
        let cache_path = confined_path(root, &cache_relative_path).ok()?;
        let metadata = fs::metadata(&cache_path).ok()?;
        if metadata.len() > MAX_ENCRYPTED_OBJECT_BYTES as u64 {
            return None;
        }
        let bytes = fs::read(cache_path).ok()?;
        let decoded = AuthenticatedChunk::decode(&bytes).ok()?;
        if decoded.decrypt(client_key, &aad).ok()?.as_slice() != plaintext {
            return None;
        }
        let prior_chunk = previous
            .file_chunks
            .iter()
            .find(|file| file.path == path)
            .and_then(|file| file.chunks.get(index))
            .filter(|chunk| {
                chunk.index == index as u64
                    && chunk.plaintext_size == plaintext.len() as u64
                    && chunk.plaintext_checksum == plaintext_checksum
            });
        let source_generation = prior_chunk
            .and_then(|chunk| {
                previous
                    .objects
                    .iter()
                    .find(|object| object.key == chunk.object.key)
            })
            .filter(|object| {
                object.size == bytes.len() as u64 && object.checksum == checksum(&bytes)
            })
            .map(|object| object.source_generation.unwrap_or(previous.generation));
        Some((bytes, source_generation))
    })();
    let (encrypted, source_generation) = match cached {
        Some((bytes, source_generation)) => (bytes, source_generation),
        None => {
            let bytes = AuthenticatedChunk::encrypt(client_key, plaintext, &aad)?.encoded();
            let expected_checksum = checksum(&bytes);
            write_verified(root, &cache_relative_path, &bytes, &expected_checksum)
                .map_err(|_| unavailable())?;
            (bytes, None)
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
    let cache_path = confined_path(root, &cache_relative_path).map_err(|_| unavailable())?;
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

fn unavailable() -> Problem {
    problem(
        "profile_projection_unavailable",
        "Profile projection is unavailable",
    )
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

fn read_profile_file(path: &Path) -> Result<Vec<u8>, Problem> {
    let file = fs::File::open(path).map_err(|_| unavailable())?;
    validate_profile_file_size(file.metadata().map_err(|_| unavailable())?.len())?;
    let mut bytes = Vec::new();
    file.take(MAX_CHUNKED_PROFILE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    validate_profile_file_size(bytes.len() as u64)?;
    Ok(bytes)
}

fn checksum_file(path: &Path) -> Result<String, Problem> {
    let metadata = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(problem(
            "profile_projection_entry_invalid",
            "Profile projection entry is invalid",
        ));
    }
    validate_profile_file_size(metadata.len())?;
    let mut file = fs::File::open(path).map_err(|_| unavailable())?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(|_| unavailable())?;
        if read == 0 {
            break;
        }
        total = total.checked_add(read as u64).ok_or_else(unavailable)?;
        validate_profile_file_size(total)?;
        digest.update(&buffer[..read]);
    }
    if total != metadata.len() || file.metadata().map_err(|_| unavailable())?.len() != total {
        return Err(problem(
            "migration_profile_source_changed",
            "Legacy profile data changed while it was being read",
        ));
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn copy_profile_file_verified(
    source: &Path,
    destination_root: &Path,
    relative: &str,
    expected_size: u64,
) -> Result<(), Problem> {
    let source_metadata = fs::symlink_metadata(source).map_err(|_| unavailable())?;
    if source_metadata.file_type().is_symlink()
        || !source_metadata.is_file()
        || source_metadata.len() != expected_size
    {
        return Err(problem(
            "migration_profile_source_changed",
            "Legacy profile data changed while it was being staged",
        ));
    }
    validate_profile_file_size(expected_size)?;
    let destination = confined_path(destination_root, Path::new(relative))?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|_| unavailable())?;
    }
    let mut input = fs::File::open(source).map_err(|_| unavailable())?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
        .map_err(|_| unavailable())?;
    let copy_result = (|| {
        let mut digest = Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let read = input.read(&mut buffer).map_err(|_| unavailable())?;
            if read == 0 {
                break;
            }
            total = total.checked_add(read as u64).ok_or_else(unavailable)?;
            if total > expected_size {
                return Err(problem(
                    "migration_profile_source_changed",
                    "Legacy profile data changed while it was being staged",
                ));
            }
            digest.update(&buffer[..read]);
            output
                .write_all(&buffer[..read])
                .map_err(|_| unavailable())?;
        }
        if total != expected_size || input.metadata().map_err(|_| unavailable())?.len() != total {
            return Err(problem(
                "migration_profile_source_changed",
                "Legacy profile data changed while it was being staged",
            ));
        }
        output.sync_all().map_err(|_| unavailable())?;
        Ok(format!("{:x}", digest.finalize()))
    })();
    let copied_checksum = match copy_result {
        Ok(checksum) => checksum,
        Err(error) => {
            let _ = fs::remove_file(&destination);
            return Err(error);
        }
    };
    let source_checksum = match checksum_file(source) {
        Ok(checksum) => checksum,
        Err(error) => {
            let _ = fs::remove_file(&destination);
            return Err(error);
        }
    };
    if source_checksum != copied_checksum {
        let _ = fs::remove_file(&destination);
        return Err(problem(
            "migration_profile_source_changed",
            "Legacy profile data changed while it was being staged",
        ));
    }
    Ok(())
}

fn problem(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 503, "desktop-projection")
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use super::*;

    #[test]
    fn legacy_empty_genesis_manifest_is_hydratable() {
        let workspace = WorkspaceId::new("ws_legacy_empty_genesis").unwrap();
        let resource = ResourceId::new("res_legacy_empty_genesis").unwrap();
        let manifest = SyncManifest::new(workspace, resource, 1, 1, None, b"", Vec::new()).unwrap();

        let descriptor = descriptor_from_manifest(&manifest).unwrap();
        descriptor.verify_manifest(&manifest).unwrap();
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
        drop(projection);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn sync_commit_recovery_preserves_base_on_remote_conflict_and_clears_committed_marker() {
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
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap();

        let recovery = projection
            .active_session_recovery_path(&profile, &conflict)
            .unwrap_err();
        assert_eq!(recovery.code, "profile_local_sync_conflict");
        assert_eq!(
            fs::read(receipt.path.join("Default/Cookies")).unwrap(),
            b"base"
        );
        assert!(receipt.path.join(MARKER).exists());

        assert!(
            projection
                .active_session_recovery_path(&profile, &target)
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
        projection
            .publish_decrypted(
                &profile,
                &base,
                vec![("Default/Cookies".to_owned(), b"base".to_vec())],
            )
            .unwrap();
        projection
            .prepare_sync_commit(&profile, &base, &target)
            .unwrap();

        assert!(
            projection
                .active_session_recovery_path(&profile, &base)
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

    #[tokio::test]
    async fn hydration_reuses_verified_generation_without_fetching_profile_blobs() {
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

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let objects: BTreeMap<_, _> = encrypted_objects
            .into_iter()
            .map(|blob| (blob.key.clone(), blob.read_bytes().unwrap()))
            .collect();
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
        assert_eq!(requested_paths.len(), 1);
        assert!(requested_paths[0].contains("resource-key-envelope.json"));
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
        let server = tokio::spawn(async move {
            loop {
                let accepted =
                    tokio::time::timeout(Duration::from_secs(5), listener.accept()).await;
                let Ok(Ok((mut stream, _))) = accepted else {
                    break;
                };
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
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/octet-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(body).await.unwrap();
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
        let (receipt_a, _) = projection_a
            .hydrate_encrypted_with_proxy_credentials(
                &client_a,
                &auth_a,
                &profile_id,
                &manifest,
                Some(&device_a_key),
                Some(&device_a_recovery),
                "cross-device-recovery-a",
            )
            .await
            .unwrap();
        let (receipt_b, _) = projection_b
            .hydrate_encrypted_with_proxy_credentials(
                &client_b,
                &auth_b,
                &profile_id,
                &manifest,
                Some(&device_b_key),
                Some(&device_b_recovery),
                "cross-device-recovery-b",
            )
            .await
            .unwrap();

        let restored_a = fs::read(receipt_a.path.join("Default/Preferences")).unwrap();
        let restored_b = fs::read(receipt_b.path.join("Default/Preferences")).unwrap();
        assert_eq!(checksum(&restored_a), checksum(&restored_b));
        assert_eq!(restored_a, b"same encrypted profile state on each device");
        assert_eq!(receipt_a.root_checksum, receipt_b.root_checksum);

        server.abort();
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
        let server = tokio::spawn(async move {
            let mut paths = Vec::new();
            loop {
                let accepted =
                    tokio::time::timeout(Duration::from_secs(2), listener.accept()).await;
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
                    stream.write_all(response.as_bytes()).await.unwrap();
                    stream.write_all(&body[..body.len() / 2]).await.unwrap();
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
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&body).await.unwrap();
                paths.push(path);
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
        let requested = server.await.unwrap();
        assert_eq!(requested.len(), manifest.objects.len() + 4);
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
        let _ = fs::remove_dir_all(root);
    }
}
