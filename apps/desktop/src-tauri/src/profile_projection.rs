use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
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
        MAX_PROFILE_SYNC_OBJECT_BYTES, ManifestDependency, ManifestObject, ManifestObjectReference,
        ManifestPayloadEncryption, ManifestTombstone, PayloadEncryptionAlgorithm,
        RESOURCE_KEY_ENVELOPE_KEY, SyncManifest,
    },
    payload::{PAYLOAD_SCHEMA_VERSION, PayloadDescriptor, PayloadObject},
};
use futures_util::{StreamExt, TryStreamExt, stream};
use serde::{Deserialize, Serialize};

use crate::remote_api::{RemoteApiClient, RemoteProxySyncSnapshot};

const MARKER: &str = ".buglogin-projection.json";
const MAX_ENCRYPTED_OBJECT_BYTES: usize = MAX_PROFILE_SYNC_OBJECT_BYTES as usize;
const MAX_CONCURRENT_BLOB_DOWNLOADS: usize = 4;
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

#[derive(Clone)]
pub struct ProfileProjection {
    root: PathBuf,
}

impl ProfileProjection {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, Problem> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|_| unavailable())?;
        let root = root.canonicalize().map_err(|_| unavailable())?;
        Ok(Self { root })
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
        let cached_generation = generation_path.exists()
            && verify_decrypted_generation(profile_id, manifest, &generation_path).is_ok();
        let clear_metadata = proxy_metadata_keys(manifest);
        let proxy_envelopes = proxy_credential_keys(manifest);
        let encrypted_objects = stream::iter(
            remote_descriptor
                .objects
                .clone()
                .into_iter()
                .filter(|object| {
                    should_download_manifest_object(
                        cached_generation,
                        &object.path,
                        &clear_metadata,
                        &proxy_envelopes,
                    )
                })
                .map(|object| {
                    let client = client.clone();
                    let scope = CoordinationScope {
                        workspace_id: &manifest.workspace_id,
                        auth,
                    };
                    let resource_id = manifest.resource_id.clone();
                    let path = object.path;
                    let checksum = object.checksum;
                    let generation = manifest.generation;
                    let size = object.size;
                    let correlation_id = correlation_id.to_owned();
                    async move {
                        let bytes = client
                            .download_blob(
                                scope,
                                &resource_id,
                                &path,
                                generation,
                                &checksum,
                                size,
                                &correlation_id,
                            )
                            .await?;
                        Ok::<_, Problem>((path, bytes))
                    }
                }),
        )
        .buffer_unordered(MAX_CONCURRENT_BLOB_DOWNLOADS)
        .try_collect::<Vec<_>>()
        .await?;
        let supplied: BTreeMap<_, _> = encrypted_objects.into_iter().collect();
        let envelope_bytes = supplied.get(RESOURCE_KEY_ENVELOPE_KEY).ok_or_else(|| {
            problem(
                "resource_key_envelope_missing",
                "Resource key envelope is missing",
            )
        })?;
        let envelope: ResourceKeyEnvelope =
            serde_json::from_slice(envelope_bytes).map_err(|_| {
                problem(
                    "resource_key_envelope_invalid",
                    "Resource key envelope is invalid",
                )
            })?;
        if !matches_manifest_object(
            &ManifestObject::new(RESOURCE_KEY_ENVELOPE_KEY, envelope_bytes)?,
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
            let bytes = supplied.get(&reference.key).ok_or_else(|| {
                problem(
                    "proxy_credential_envelope_missing",
                    "Proxy credential envelope is missing",
                )
            })?;
            if !matches_manifest_object(
                &ManifestObject::new(reference.key.clone(), bytes)?,
                reference,
            ) {
                return Err(problem(
                    "proxy_credential_envelope_mismatch",
                    "Proxy credential envelope does not match the manifest",
                ));
            }
            let envelope: ProxyCredentialEnvelope =
                serde_json::from_slice(bytes).map_err(|_| {
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
        let mut profile_files = Vec::new();
        if !cached_generation {
            for object in &remote_descriptor.objects {
                if object.path == RESOURCE_KEY_ENVELOPE_KEY
                    || clear_metadata.contains(&object.path)
                    || proxy_envelopes
                        .iter()
                        .any(|(_, _, reference)| reference.key == object.path)
                {
                    continue;
                }
                let bytes = supplied.get(&object.path).ok_or_else(|| {
                    problem(
                        "profile_projection_incomplete",
                        "Profile projection is incomplete",
                    )
                })?;
                let chunk = AuthenticatedChunk::decode(bytes)?;
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
                profile_files.push((object.path.clone(), plaintext));
            }
        }
        self.publish_decrypted(profile_id, manifest, profile_files)
            .map(|receipt| (receipt, credentials))
    }

    fn publish_decrypted(
        &self,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
        objects: Vec<(String, Vec<u8>)>,
    ) -> Result<ProjectionReceipt, Problem> {
        let local_descriptor = PayloadDescriptor::new(
            manifest.workspace_id.clone(),
            manifest.resource_id.clone(),
            manifest.generation,
            objects
                .iter()
                .map(|(path, bytes)| PayloadObject::new(path.clone(), bytes))
                .collect::<Result<Vec<_>, _>>()?,
        )?;
        let generation_path = decrypted_generation_path(&self.root, profile_id, manifest)?;
        let profile_root = confined_path(&self.root, Path::new(profile_id.as_str()))?;
        fs::create_dir_all(&profile_root).map_err(|_| unavailable())?;
        if generation_path.exists() {
            return verify_decrypted_generation(profile_id, manifest, &generation_path);
        }
        let staging_name = format!(
            "staging/{}-{}-{}",
            manifest.generation,
            std::process::id(),
            unique_suffix()
        );
        let staging_path = confined_path(&profile_root, Path::new(&staging_name))?;
        fs::create_dir_all(&staging_path).map_err(|_| unavailable())?;
        let supplied: BTreeMap<_, _> = objects.into_iter().collect();
        if supplied.len() != local_descriptor.objects.len() {
            return Err(problem(
                "profile_projection_incomplete",
                "Profile projection is incomplete",
            ));
        }
        for object in &local_descriptor.objects {
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
        let marker = EncryptedProjectionMarker {
            state: "complete".to_owned(),
            profile_id: profile_id.clone(),
            generation: manifest.generation,
            remote_root_checksum: manifest.root_checksum.clone(),
            local_objects: local_descriptor.objects,
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
        verify_decrypted_generation(profile_id, manifest, &generation_path)
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
        let descriptor = PayloadDescriptor::new(
            previous.workspace_id.clone(),
            previous.resource_id.clone(),
            previous.generation + 1,
            objects,
        )?;
        let live: BTreeMap<_, _> = descriptor
            .objects
            .iter()
            .map(|object| (object.path.as_str(), object))
            .collect();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
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
                    now.saturating_add(30 * 24 * 60 * 60 * 1_000),
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
    ) -> Result<(PayloadDescriptor, Vec<(String, Vec<u8>)>, SyncManifest), Problem> {
        let (_, plaintext_files, mut plaintext_manifest) = self.snapshot_with_proxy(
            profile_path,
            previous,
            fencing_token,
            proxy.clone().map(|mut proxy| {
                proxy.credential = None;
                proxy
            }),
        )?;
        let proxy_credential = proxy.and_then(|proxy| {
            proxy
                .credential
                .map(|secret| (proxy.proxy_id, proxy.revision, secret))
        });
        let clear_metadata = proxy_metadata_keys(&plaintext_manifest);
        let mut files = Vec::with_capacity(plaintext_files.len() + 1);
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
        for (path, plaintext) in plaintext_files {
            let bytes = if clear_metadata.contains(&path) {
                plaintext
            } else {
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
                        if encrypted.len() <= MAX_ENCRYPTED_OBJECT_BYTES {
                            let cache_checksum = checksum(&encrypted);
                            let _ = write_verified(
                                &self.root,
                                &cache_path,
                                &encrypted,
                                &cache_checksum,
                            );
                        }
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
                encrypted
            };
            files.push((path, bytes));
        }
        let envelope = ResourceKeyEnvelope::wrap(
            previous.workspace_id.clone(),
            previous.resource_id.clone(),
            client_key,
            recovery_key,
        )?;
        let envelope_bytes = serde_json::to_vec(&envelope).map_err(|_| unavailable())?;
        files.push((RESOURCE_KEY_ENVELOPE_KEY.to_owned(), envelope_bytes));

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
            files.push((proxy_envelope_key.clone(), proxy_envelope_bytes));
            let dependency_matches = plaintext_manifest.dependencies.iter().any(|dependency| {
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
            let object = ManifestObject::new(proxy_envelope_key, &files.last().unwrap().1)?;
            let Some(ManifestDependency::Proxy {
                credential_envelope_object,
                ..
            }) = plaintext_manifest
                .dependencies
                .iter_mut()
                .find(|dependency| {
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
            .map(|(path, bytes)| PayloadObject::new(path.clone(), bytes))
            .collect::<Result<Vec<_>, _>>()?;
        let descriptor = PayloadDescriptor::new(
            previous.workspace_id.clone(),
            previous.resource_id.clone(),
            plaintext_manifest.generation,
            objects,
        )?;
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
            plaintext_manifest
                .tombstones
                .into_iter()
                .filter(|tombstone| tombstone.key != RESOURCE_KEY_ENVELOPE_KEY)
                .collect(),
        )?;
        manifest.dependencies = plaintext_manifest.dependencies;
        let envelope_object = manifest
            .objects
            .iter()
            .find(|object| object.key == RESOURCE_KEY_ENVELOPE_KEY)
            .ok_or_else(|| unavailable())?;
        manifest.payload_encryption = Some(ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV2,
            key_version: Some(key_version),
            resource_key_envelope: ManifestObjectReference::from(envelope_object),
        });
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

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectionMarker {
    state: String,
    profile_id: ProfileId,
    generation: u64,
    root_checksum: String,
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
        let bytes = read_verified(generation_path, Path::new(&object.path), &object.checksum)?;
        object.verify(&bytes)?;
    }
    Ok(ProjectionReceipt {
        profile_id: profile_id.clone(),
        generation: manifest.generation,
        root_checksum: manifest.root_checksum.clone(),
        path: generation_path.to_path_buf(),
    })
}

fn decrypted_generation_path(
    root: &Path,
    profile_id: &ProfileId,
    manifest: &SyncManifest,
) -> Result<PathBuf, Problem> {
    let relative = PathBuf::from(profile_id.as_str())
        .join("generations")
        .join(format!(
            "{}-{}",
            manifest.generation, manifest.root_checksum
        ));
    confined_path(root, &relative)
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
    cached_generation: bool,
    path: &str,
    clear_metadata: &std::collections::BTreeSet<String>,
    proxy_envelopes: &[(buglogin_domain::ids::ProxyId, u64, ManifestObjectReference)],
) -> bool {
    !cached_generation
        || path == RESOURCE_KEY_ENVELOPE_KEY
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
        if metadata.file_type().is_symlink() {
            return Err(problem(
                "profile_projection_symlink_rejected",
                "Profile projection symlink is rejected",
            ));
        }
        if metadata.is_dir() {
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
        if relative == MARKER {
            continue;
        }
        validate_profile_file_size(metadata.len())?;
        output.push((relative, read_profile_file(&path)?));
    }
    Ok(())
}

fn unavailable() -> Problem {
    problem(
        "profile_projection_unavailable",
        "Profile projection is unavailable",
    )
}

fn validate_profile_file_size(size: u64) -> Result<(), Problem> {
    if size > MAX_PROFILE_PLAINTEXT_BYTES {
        return Err(Problem::new(
            "profile_object_too_large",
            "A profile file exceeds the encrypted sync object limit",
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
    file.take(MAX_PROFILE_PLAINTEXT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    validate_profile_file_size(bytes.len() as u64)?;
    Ok(bytes)
}

fn problem(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 503, "desktop-projection")
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use buglogin_domain::ids::{ProxyId, ResourceId, WorkspaceId};
    use buglogin_sync::coordination::CoordinationAuth;
    use buglogin_sync::crypto::{AuthenticatedChunk, ClientKey, RecoveryKey};
    use buglogin_sync::manifest::{ManifestObject, ManifestObjectReference};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
        let objects: BTreeMap<_, _> = encrypted_objects.into_iter().collect();
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
            true,
            "Default/Cookies",
            &metadata,
            &credentials,
        ));
        assert!(should_download_manifest_object(
            true,
            RESOURCE_KEY_ENVELOPE_KEY,
            &metadata,
            &credentials,
        ));
        assert!(should_download_manifest_object(
            true,
            metadata_key,
            &metadata,
            &credentials,
        ));
        assert!(should_download_manifest_object(
            true,
            credential_key,
            &metadata,
            &credentials,
        ));
        assert!(should_download_manifest_object(
            false,
            "Default/Cookies",
            &metadata,
            &credentials,
        ));
    }

    #[test]
    fn rejects_profile_file_size_that_would_exceed_encrypted_object_limit() {
        assert!(validate_profile_file_size(MAX_PROFILE_PLAINTEXT_BYTES).is_ok());
        let error = validate_profile_file_size(MAX_PROFILE_PLAINTEXT_BYTES + 1).unwrap_err();
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
        let encrypted = files
            .iter()
            .find(|(path, _)| path == "Preferences")
            .unwrap();
        assert!(
            !encrypted
                .1
                .windows(b"private-profile-data".len())
                .any(|window| { window == b"private-profile-data" })
        );
        let decrypted = AuthenticatedChunk::decode(&encrypted.1)
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
            .find(|(path, _)| path == RESOURCE_KEY_ENVELOPE_KEY)
            .unwrap();
        assert_eq!(envelope_ref.checksum, checksum(&envelope_bytes.1));
        assert!(
            manifest
                .tombstones
                .iter()
                .all(|tombstone| tombstone.key != RESOURCE_KEY_ENVELOPE_KEY)
        );
        let envelope: ResourceKeyEnvelope = serde_json::from_slice(&envelope_bytes.1).unwrap();
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
        assert!(
            next_files.iter().any(|(path, bytes)| {
                path == "Preferences" && checksum(bytes) == reused.checksum
            })
        );
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
            .find(|(path, _)| path == "Preferences")
            .unwrap();
        let first_checksum = checksum(&first.1);
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
            .find(|(path, _)| path == "Preferences")
            .unwrap();
        assert_eq!(checksum(&retry.1), first_checksum);
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
            AuthenticatedChunk::decode(&retry.1)
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
        let object = files
            .iter()
            .find(|(path, _)| path == &reference.key)
            .unwrap();
        assert_eq!(object.1.len() as u64, reference.size);
        assert!(!object.1.windows(10).any(|window| window == b"proxy-pass"));
        let envelope: ProxyCredentialEnvelope = serde_json::from_slice(&object.1).unwrap();
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
