use std::collections::{BTreeMap, BTreeSet};

use buglogin_domain::{
    errors::Problem,
    ids::{ProxyId, ResourceId, WorkspaceId},
};
use buglogin_storage::blob::checksum;
use serde::{Deserialize, Serialize};

pub const MANIFEST_SCHEMA_VERSION: u16 = 2;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestObject {
    pub key: String,
    pub size: u64,
    pub checksum: String,
}

impl ManifestObject {
    pub fn new(key: impl Into<String>, bytes: &[u8]) -> Result<Self, Problem> {
        let key = key.into();
        validate_key(&key)?;
        Ok(Self {
            key,
            size: bytes.len() as u64,
            checksum: checksum(bytes),
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
#[serde(rename_all = "camelCase")]
pub struct ManifestTombstone {
    pub key: String,
    pub deleted_generation: u64,
    pub deleted_at_unix_ms: u64,
    pub retention_deadline_unix_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestObjectReference {
    pub key: String,
    pub size: u64,
    pub checksum: String,
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
        metadata_object: ManifestObjectReference,
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
        };
        tombstone.validate(u64::MAX)?;
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
#[serde(rename_all = "camelCase")]
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
        };
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), Problem> {
        if self.schema_version != MANIFEST_SCHEMA_VERSION {
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
        let mut keys = BTreeSet::new();
        for object in &self.objects {
            validate_key(&object.key)?;
            if !keys.insert(&object.key) {
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
            if keys.contains(&tombstone.key) {
                return Err(Problem::new(
                    "stale_manifest_resurrection",
                    "Live object conflicts with tombstone",
                    409,
                    "sync",
                ));
            }
        }
        let mut dependency_ids = BTreeSet::new();
        for dependency in &self.dependencies {
            match dependency {
                ManifestDependency::Proxy {
                    proxy_id,
                    revision,
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
                    validate_dependency_object(metadata_object, &self.objects)?;
                    if let Some(envelope) = credential_envelope_object {
                        validate_dependency_object(envelope, &self.objects)?;
                        if envelope.key == metadata_object.key {
                            return Err(Problem::new(
                                "invalid_manifest_dependency",
                                "Manifest dependency objects must be distinct",
                                400,
                                "sync",
                            ));
                        }
                    }
                }
            }
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

        let objects = self.object_map();
        let tombstones = self.tombstone_map();
        for previous_tombstone in &previous.tombstones {
            if previous_tombstone.retention_deadline_unix_ms <= observed_at_unix_ms {
                continue;
            }
            if objects.contains_key(previous_tombstone.key.as_str()) {
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
    if key.is_empty() || key.starts_with('/') || key.contains("..") {
        return Err(Problem::new(
            "invalid_manifest_object_key",
            "Invalid manifest object key",
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

fn checksum_problem(code: &str) -> Problem {
    Problem::new(code, "Manifest checksum mismatch", 409, "sync")
}
