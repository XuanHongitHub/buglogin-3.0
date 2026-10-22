use std::collections::{BTreeMap, BTreeSet};

use buglogin_domain::{
    errors::Problem,
    ids::{CredentialId, ProxyId, ResourceId, WorkspaceId},
};
use buglogin_storage::{blob::checksum, remote::MAX_REMOTE_OBJECT_BYTES};
use serde::{Deserialize, Serialize};

pub const MANIFEST_SCHEMA_VERSION: u16 = 2;
pub const CHUNKED_MANIFEST_SCHEMA_VERSION: u16 = 3;
pub const RESOURCE_KEY_ENVELOPE_KEY: &str = ".buglogin/resource-key-envelope.json";
pub const MAX_PROFILE_SYNC_OBJECT_BYTES: u64 = MAX_REMOTE_OBJECT_BYTES;
pub const PROFILE_FILE_CHUNK_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_CHUNKED_PROFILE_FILE_BYTES: u64 = 1024 * 1024 * 1024;
const EPHEMERAL_PROFILE_DIRECTORIES: &[&str] = &[
    "cache",
    "code cache",
    "gpu cache",
    "grshadercache",
    "dawncache",
    "shadercache",
    "scriptcache",
    "media cache",
    "crashpad",
    "crash_reports",
    "crash reports",
];

pub fn is_ephemeral_profile_path(path: &str) -> bool {
    path.split('/').any(|segment| {
        EPHEMERAL_PROFILE_DIRECTORIES
            .iter()
            .any(|directory| segment.eq_ignore_ascii_case(directory))
    })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestObject {
    pub key: String,
    pub size: u64,
    pub checksum: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_generation: Option<u64>,
}

impl ManifestObject {
    pub fn new(key: impl Into<String>, bytes: &[u8]) -> Result<Self, Problem> {
        let key = key.into();
        validate_key(&key)?;
        if bytes.len() as u64 > MAX_PROFILE_SYNC_OBJECT_BYTES {
            return Err(Problem::new(
                "manifest_object_too_large",
                "Manifest object exceeds the maximum size",
                413,
                "sync",
            ));
        }
        Ok(Self {
            key,
            size: bytes.len() as u64,
            checksum: checksum(bytes),
            source_generation: None,
        })
    }

    pub fn verify(&self, bytes: &[u8]) -> Result<(), Problem> {
        if self.size != bytes.len() as u64 || self.checksum != checksum(bytes) {
            return Err(checksum_problem("manifest_object_checksum_mismatch"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestTombstone {
    pub key: String,
    pub deleted_generation: u64,
    pub deleted_at_unix_ms: u64,
    pub retention_deadline_unix_ms: u64,
    #[serde(default, skip_serializing_if = "is_false")]
    pub object_retired: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestObjectReference {
    pub key: String,
    pub size: u64,
    pub checksum: String,
}

/// One immutable encrypted chunk that contributes to a logical profile file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestFileChunk {
    pub index: u64,
    pub plaintext_size: u64,
    pub plaintext_checksum: String,
    pub object: ManifestObjectReference,
}

/// Authenticated mapping from a logical profile path to its ordered chunks.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestFileChunks {
    pub path: String,
    pub plaintext_size: u64,
    pub plaintext_checksum: String,
    pub chunks: Vec<ManifestFileChunk>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PayloadEncryptionAlgorithm {
    #[serde(rename = "aes-256-gcm-chunk-v1")]
    Aes256GcmChunkV1,
    #[serde(rename = "aes-256-gcm-chunk-v2")]
    Aes256GcmChunkV2,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestPayloadEncryption {
    pub schema_version: u8,
    pub algorithm: PayloadEncryptionAlgorithm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_version: Option<String>,
    pub resource_key_envelope: ManifestObjectReference,
}

impl From<&ManifestObject> for ManifestObjectReference {
    fn from(object: &ManifestObject) -> Self {
        Self {
            key: object.key.clone(),
            size: object.size,
            checksum: object.checksum.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ManifestDependency {
    Proxy {
        proxy_id: ProxyId,
        revision: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        credential_version: Option<CredentialId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata_object: Option<ManifestObjectReference>,
        credential_envelope_object: Option<ManifestObjectReference>,
    },
}

impl ManifestTombstone {
    pub fn new(
        key: impl Into<String>,
        deleted_generation: u64,
        deleted_at_unix_ms: u64,
        retention_deadline_unix_ms: u64,
    ) -> Result<Self, Problem> {
        let tombstone = Self {
            key: key.into(),
            deleted_generation,
            deleted_at_unix_ms,
            retention_deadline_unix_ms,
            object_retired: false,
        };
        tombstone.validate(u64::MAX)?;
        Ok(tombstone)
    }

    pub fn new_object(
        key: impl Into<String>,
        deleted_generation: u64,
        deleted_at_unix_ms: u64,
        retention_deadline_unix_ms: u64,
    ) -> Result<Self, Problem> {
        let mut tombstone = Self::new(
            key,
            deleted_generation,
            deleted_at_unix_ms,
            retention_deadline_unix_ms,
        )?;
        tombstone.object_retired = true;
        Ok(tombstone)
    }

    fn validate(&self, manifest_generation: u64) -> Result<(), Problem> {
        validate_key(&self.key)?;
        if self.deleted_generation == 0 || self.deleted_generation > manifest_generation {
            return Err(Problem::new(
                "invalid_tombstone_generation",
                "Invalid tombstone generation",
                400,
                "sync",
            ));
        }
        if self.deleted_at_unix_ms == 0
            || self.retention_deadline_unix_ms <= self.deleted_at_unix_ms
        {
            return Err(Problem::new(
                "invalid_tombstone_retention",
                "Invalid tombstone retention",
                400,
                "sync",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncManifest {
    pub schema_version: u16,
    pub workspace_id: WorkspaceId,
    pub resource_id: ResourceId,
    pub fencing_token: u64,
    pub generation: u64,
    pub parent_generation: Option<u64>,
    pub root_checksum: String,
    pub objects: Vec<ManifestObject>,
    pub tombstones: Vec<ManifestTombstone>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<ManifestDependency>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_encryption: Option<ManifestPayloadEncryption>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_chunks: Vec<ManifestFileChunks>,
}

impl SyncManifest {
    pub fn new(
        workspace_id: WorkspaceId,
        resource_id: ResourceId,
        fencing_token: u64,
        generation: u64,
        parent_generation: Option<u64>,
        root_bytes: &[u8],
        objects: Vec<ManifestObject>,
    ) -> Result<Self, Problem> {
        Self::new_with_tombstones(
            workspace_id,
            resource_id,
            fencing_token,
            generation,
            parent_generation,
            root_bytes,
            objects,
            Vec::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_with_tombstones(
        workspace_id: WorkspaceId,
        resource_id: ResourceId,
        fencing_token: u64,
        generation: u64,
        parent_generation: Option<u64>,
        root_bytes: &[u8],
        objects: Vec<ManifestObject>,
        tombstones: Vec<ManifestTombstone>,
    ) -> Result<Self, Problem> {
        let manifest = Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            workspace_id,
            resource_id,
            fencing_token,
            generation,
            parent_generation,
            root_checksum: checksum(root_bytes),
            objects,
            tombstones,
            dependencies: Vec::new(),
            payload_encryption: None,
            file_chunks: Vec::new(),
        };
        manifest.validate()?;
        manifest.validate_new_profile_paths()?;
        Ok(manifest)
    }

    pub fn new_chunked(
        workspace_id: WorkspaceId,
        resource_id: ResourceId,
        fencing_token: u64,
        generation: u64,
        parent_generation: Option<u64>,
        root_bytes: &[u8],
        objects: Vec<ManifestObject>,
        payload_encryption: ManifestPayloadEncryption,
        file_chunks: Vec<ManifestFileChunks>,
    ) -> Result<Self, Problem> {
        Self::new_chunked_with_tombstones(
            workspace_id,
            resource_id,
            fencing_token,
            generation,
            parent_generation,
            root_bytes,
            objects,
            Vec::new(),
            payload_encryption,
            file_chunks,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_chunked_with_tombstones(
        workspace_id: WorkspaceId,
        resource_id: ResourceId,
        fencing_token: u64,
        generation: u64,
        parent_generation: Option<u64>,
        root_bytes: &[u8],
        objects: Vec<ManifestObject>,
        tombstones: Vec<ManifestTombstone>,
        payload_encryption: ManifestPayloadEncryption,
        file_chunks: Vec<ManifestFileChunks>,
    ) -> Result<Self, Problem> {
        let manifest = Self {
            schema_version: CHUNKED_MANIFEST_SCHEMA_VERSION,
            workspace_id,
            resource_id,
            fencing_token,
            generation,
            parent_generation,
            root_checksum: checksum(root_bytes),
            objects,
            tombstones,
            dependencies: Vec::new(),
            payload_encryption: Some(payload_encryption),
            file_chunks,
        };
        manifest.validate()?;
        manifest.validate_new_profile_paths()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), Problem> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION
            && self.schema_version != CHUNKED_MANIFEST_SCHEMA_VERSION
        {
            return Err(Problem::new(
                "unsupported_manifest_schema",
                "Unsupported manifest schema",
                409,
                "sync",
            ));
        }
        if self.generation == 0 {
            return Err(Problem::new(
                "invalid_manifest_generation",
                "Invalid manifest generation",
                400,
                "sync",
            ));
        }
        if self.fencing_token == 0 {
            return Err(Problem::new(
                "invalid_manifest_fencing_token",
                "Invalid manifest fencing token",
                400,
                "sync",
            ));
        }
        let expected_parent = self.generation.checked_sub(1).filter(|parent| *parent > 0);
        if self.parent_generation != expected_parent {
            return Err(Problem::new(
                "invalid_manifest_parent",
                "Invalid manifest parent",
                400,
                "sync",
            ));
        }
        if !is_checksum(&self.root_checksum) {
            return Err(checksum_problem("invalid_manifest_checksum"));
        }
        let mut keys: BTreeSet<&str> = BTreeSet::new();
        for object in &self.objects {
            validate_key(&object.key)?;
            if object.source_generation.is_some_and(|source_generation| {
                source_generation == 0 || source_generation >= self.generation
            }) {
                return Err(Problem::new(
                    "invalid_manifest_source_generation",
                    "Reused object source generation must precede this manifest",
                    400,
                    "sync",
                ));
            }
            if !keys.insert(object.key.as_str()) {
                return Err(Problem::new(
                    "duplicate_manifest_object",
                    "Duplicate manifest object",
                    409,
                    "sync",
                ));
            }
            if !is_checksum(&object.checksum) {
                return Err(checksum_problem("invalid_manifest_object_checksum"));
            }
            if object.size > MAX_PROFILE_SYNC_OBJECT_BYTES {
                return Err(Problem::new(
                    "manifest_object_too_large",
                    "Manifest object exceeds the maximum size",
                    413,
                    "sync",
                ));
            }
        }
        if self.schema_version == MANIFEST_SCHEMA_VERSION && !self.file_chunks.is_empty() {
            return Err(Problem::new(
                "chunked_manifest_schema_required",
                "File chunks require manifest schema version 3",
                400,
                "sync",
            ));
        }
        if self.schema_version == CHUNKED_MANIFEST_SCHEMA_VERSION && self.file_chunks.is_empty() {
            return Err(Problem::new(
                "manifest_file_chunks_required",
                "Manifest schema version 3 requires file chunk mappings",
                400,
                "sync",
            ));
        }
        let mut file_paths = BTreeSet::new();
        let mut chunk_object_keys = BTreeSet::new();
        let mut previous_file_path: Option<&str> = None;
        for file in &self.file_chunks {
            validate_file_path(&file.path)?;
            if file.path.starts_with(".buglogin/")
                || previous_file_path.is_some_and(|previous| previous >= file.path.as_str())
                || !file_paths.insert(file.path.as_str())
                || file.plaintext_size == 0
                || file.plaintext_size > MAX_CHUNKED_PROFILE_FILE_BYTES
                || file.chunks.len()
                    > (MAX_CHUNKED_PROFILE_FILE_BYTES / PROFILE_FILE_CHUNK_BYTES) as usize
                || !is_checksum(&file.plaintext_checksum)
                || file.chunks.is_empty()
            {
                return Err(Problem::new(
                    "invalid_manifest_file_chunks",
                    "Manifest file chunk mapping is invalid",
                    400,
                    "sync",
                ));
            }
            previous_file_path = Some(&file.path);
            let mut total_size = 0_u64;
            for (index, chunk) in file.chunks.iter().enumerate() {
                let expected_chunk_key = format!(
                    ".buglogin/file-chunks/{}/{:016x}-{}-{}",
                    checksum(file.path.as_bytes()),
                    chunk.index,
                    chunk.plaintext_checksum,
                    chunk.object.checksum
                );
                if chunk.index != index as u64
                    || chunk.plaintext_size == 0
                    || chunk.plaintext_size > PROFILE_FILE_CHUNK_BYTES
                    || (index + 1 < file.chunks.len()
                        && chunk.plaintext_size != PROFILE_FILE_CHUNK_BYTES)
                    || !is_checksum(&chunk.plaintext_checksum)
                    || chunk.object.key != expected_chunk_key
                    || !chunk_object_keys.insert(chunk.object.key.as_str())
                {
                    return Err(Problem::new(
                        "invalid_manifest_file_chunks",
                        "Manifest file chunk sequence is invalid",
                        400,
                        "sync",
                    ));
                }
                validate_dependency_object(&chunk.object, &self.objects)?;
                total_size = total_size
                    .checked_add(chunk.plaintext_size)
                    .ok_or_else(|| {
                        Problem::new(
                            "invalid_manifest_file_chunks",
                            "Manifest file length overflows",
                            400,
                            "sync",
                        )
                    })?;
            }
            if total_size != file.plaintext_size {
                return Err(Problem::new(
                    "invalid_manifest_file_chunks",
                    "Manifest file chunk lengths do not match the file length",
                    400,
                    "sync",
                ));
            }
        }
        if file_paths.iter().any(|path| keys.contains(path)) {
            return Err(Problem::new(
                "invalid_manifest_file_chunks",
                "A chunked logical file cannot also be a manifest object",
                400,
                "sync",
            ));
        }
        if file_paths.iter().any(|path| {
            let mut prefix = String::new();
            let mut parts = path.split('/').peekable();
            while let Some(part) = parts.next() {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(part);
                if parts.peek().is_some() && file_paths.contains(prefix.as_str()) {
                    return true;
                }
            }
            false
        }) {
            return Err(Problem::new(
                "invalid_manifest_file_chunks",
                "Manifest file paths cannot contain both a file and its child path",
                400,
                "sync",
            ));
        }
        if self
            .objects
            .iter()
            .filter(|object| object.key.starts_with(".buglogin/file-chunks/"))
            .any(|object| !chunk_object_keys.contains(object.key.as_str()))
        {
            return Err(Problem::new(
                "invalid_manifest_file_chunks",
                "Every file chunk object must be referenced exactly once",
                400,
                "sync",
            ));
        }
        if !self.file_chunks.is_empty()
            && !self.payload_encryption.as_ref().is_some_and(|encryption| {
                encryption.algorithm == PayloadEncryptionAlgorithm::Aes256GcmChunkV2
                    && encryption.key_version.as_deref().is_some_and(is_checksum)
            })
        {
            return Err(Problem::new(
                "chunked_manifest_encryption_required",
                "File chunks require versioned client-side encryption",
                400,
                "sync",
            ));
        }
        let mut tombstone_keys = BTreeSet::new();
        for tombstone in &self.tombstones {
            tombstone.validate(self.generation)?;
            if !tombstone_keys.insert(&tombstone.key) {
                return Err(Problem::new(
                    "duplicate_manifest_tombstone",
                    "Duplicate manifest tombstone",
                    409,
                    "sync",
                ));
            }
            if keys.contains(tombstone.key.as_str())
                || (!tombstone.object_retired && file_paths.contains(tombstone.key.as_str()))
            {
                return Err(Problem::new(
                    "stale_manifest_resurrection",
                    "Live object conflicts with tombstone",
                    409,
                    "sync",
                ));
            }
        }
        let mut dependency_ids = BTreeSet::new();
        if let Some(encryption) = &self.payload_encryption {
            if encryption.schema_version != 1 {
                return Err(Problem::new(
                    "unsupported_payload_encryption_schema",
                    "Unsupported payload encryption schema",
                    409,
                    "sync",
                ));
            }
            match encryption.algorithm {
                PayloadEncryptionAlgorithm::Aes256GcmChunkV1
                    if encryption.key_version.is_some() =>
                {
                    return Err(Problem::new(
                        "invalid_payload_key_version",
                        "Legacy payload encryption must not declare a key version",
                        400,
                        "sync",
                    ));
                }
                PayloadEncryptionAlgorithm::Aes256GcmChunkV2
                    if !encryption.key_version.as_deref().is_some_and(is_checksum) =>
                {
                    return Err(Problem::new(
                        "invalid_payload_key_version",
                        "Versioned payload encryption requires a valid key version",
                        400,
                        "sync",
                    ));
                }
                _ => {}
            }
            if encryption.resource_key_envelope.key != RESOURCE_KEY_ENVELOPE_KEY {
                return Err(Problem::new(
                    "invalid_resource_key_envelope_key",
                    "Resource key envelope must use its canonical manifest key",
                    400,
                    "sync",
                ));
            }
            validate_dependency_object(&encryption.resource_key_envelope, &self.objects)?;
        }
        if self
            .objects
            .iter()
            .any(|object| object.source_generation.is_some())
            && !self.payload_encryption.as_ref().is_some_and(|encryption| {
                encryption.algorithm == PayloadEncryptionAlgorithm::Aes256GcmChunkV2
            })
        {
            return Err(Problem::new(
                "manifest_object_reuse_requires_v2",
                "Object reuse requires versioned payload encryption",
                400,
                "sync",
            ));
        }
        for dependency in &self.dependencies {
            match dependency {
                ManifestDependency::Proxy {
                    proxy_id,
                    revision,
                    credential_version,
                    metadata_object,
                    credential_envelope_object,
                } => {
                    if *revision == 0 || !dependency_ids.insert(proxy_id) {
                        return Err(Problem::new(
                            "invalid_manifest_dependency",
                            "Invalid or duplicate manifest dependency",
                            400,
                            "sync",
                        ));
                    }
                    if let Some(metadata_object) = metadata_object {
                        validate_dependency_object(metadata_object, &self.objects)?;
                    }
                    if let Some(envelope) = credential_envelope_object {
                        validate_dependency_object(envelope, &self.objects)?;
                        if metadata_object
                            .as_ref()
                            .is_some_and(|metadata_object| envelope.key == metadata_object.key)
                        {
                            return Err(Problem::new(
                                "invalid_manifest_dependency",
                                "Manifest dependency objects must be distinct",
                                400,
                                "sync",
                            ));
                        }
                    }
                    if credential_version.is_some() && credential_envelope_object.is_none() {
                        return Err(Problem::new(
                            "invalid_manifest_dependency",
                            "Proxy credential version requires an encrypted credential envelope",
                            400,
                            "sync",
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn validate_new_profile_paths(&self) -> Result<(), Problem> {
        if self
            .objects
            .iter()
            .any(|object| is_ephemeral_profile_path(&object.key))
            || self
                .file_chunks
                .iter()
                .any(|file| is_ephemeral_profile_path(&file.path))
        {
            return Err(Problem::new(
                "sync_ephemeral_profile_path",
                "Browser cache and crash-dump paths cannot be committed as profile data",
                400,
                "sync",
            ));
        }
        Ok(())
    }

    pub fn validate_successor(
        &self,
        previous: &Self,
        observed_at_unix_ms: u64,
    ) -> Result<(), Problem> {
        previous.validate()?;
        self.validate()?;
        self.validate_new_profile_paths()?;

        let objects = self.object_map();
        let live_file_paths = self
            .file_chunks
            .iter()
            .map(|file| file.path.as_str())
            .collect::<BTreeSet<_>>();
        let tombstones = self.tombstone_map();
        for previous_tombstone in &previous.tombstones {
            if previous_tombstone.retention_deadline_unix_ms <= observed_at_unix_ms {
                continue;
            }
            if objects.contains_key(previous_tombstone.key.as_str())
                || (!previous_tombstone.object_retired
                    && live_file_paths.contains(previous_tombstone.key.as_str()))
            {
                return Err(Problem::new(
                    "stale_manifest_resurrection",
                    "Live object conflicts with retained tombstone",
                    409,
                    "sync",
                ));
            }
            if tombstones.get(previous_tombstone.key.as_str()) != Some(&previous_tombstone) {
                return Err(Problem::new(
                    "missing_retained_tombstone",
                    "Retained tombstone is missing or changed",
                    409,
                    "sync",
                ));
            }
        }
        Ok(())
    }

    pub fn verify_root(&self, bytes: &[u8]) -> Result<(), Problem> {
        if checksum(bytes) != self.root_checksum {
            return Err(checksum_problem("manifest_checksum_mismatch"));
        }
        Ok(())
    }

    pub fn object_map(&self) -> BTreeMap<&str, &ManifestObject> {
        self.objects
            .iter()
            .map(|object| (object.key.as_str(), object))
            .collect()
    }

    pub fn tombstone_map(&self) -> BTreeMap<&str, &ManifestTombstone> {
        self.tombstones
            .iter()
            .map(|tombstone| (tombstone.key.as_str(), tombstone))
            .collect()
    }
}

fn validate_dependency_object(
    reference: &ManifestObjectReference,
    objects: &[ManifestObject],
) -> Result<(), Problem> {
    validate_key(&reference.key)?;
    if !is_checksum(&reference.checksum)
        || !objects.iter().any(|object| {
            object.key == reference.key
                && object.size == reference.size
                && object.checksum == reference.checksum
        })
    {
        return Err(Problem::new(
            "manifest_dependency_object_missing",
            "Manifest dependency object is missing or mismatched",
            409,
            "sync",
        ));
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), Problem> {
    if key.is_empty()
        || key.len() > 1024
        || key.starts_with('/')
        || key.contains('\\')
        || key.contains(':')
        || key.chars().any(char::is_control)
        || key
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(Problem::new(
            "invalid_manifest_object_key",
            "Invalid manifest object key",
            400,
            "sync",
        ));
    }
    Ok(())
}

fn validate_file_path(path: &str) -> Result<(), Problem> {
    if path.is_empty()
        || path.len() > 512
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(Problem::new(
            "invalid_manifest_file_chunks",
            "Manifest file path is invalid",
            400,
            "sync",
        ));
    }
    Ok(())
}

fn is_checksum(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_false(value: &bool) -> bool {
    !value
}

fn checksum_problem(code: &str) -> Problem {
    Problem::new(code, "Manifest checksum mismatch", 409, "sync")
}
