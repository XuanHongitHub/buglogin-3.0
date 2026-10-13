use std::collections::{BTreeMap, BTreeSet};

use buglogin_domain::{
    errors::Problem,
    ids::{ResourceId, WorkspaceId},
};
use buglogin_storage::blob::checksum;
use serde::{Deserialize, Serialize};

use crate::manifest::SyncManifest;

pub const SYNC_EXPORT_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncExportBlob {
    pub key: String,
    pub size: u64,
    pub checksum: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncRevision {
    pub generation: u64,
    pub parent_generation: Option<u64>,
    pub manifest_sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncExport {
    pub schema_version: u16,
    pub export_id: String,
    pub source_instance_id: String,
    pub workspace_id: WorkspaceId,
    pub resource_id: ResourceId,
    pub base_generation: Option<u64>,
    pub current_generation: u64,
    pub manifest_sha256: String,
    pub blob_set_sha256: String,
    pub manifest: SyncManifest,
    pub revisions: Vec<SyncRevision>,
    pub blobs: Vec<SyncExportBlob>,
    pub created_at_unix_ms: u64,
    pub tool_version: String,
}

impl SyncExport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        export_id: impl Into<String>,
        source_instance_id: impl Into<String>,
        mut manifest: SyncManifest,
        base_generation: Option<u64>,
        revisions: Vec<SyncRevision>,
        mut blobs: Vec<SyncExportBlob>,
        created_at_unix_ms: u64,
        tool_version: impl Into<String>,
    ) -> Result<Self, Problem> {
        manifest
            .objects
            .sort_by(|left, right| left.key.cmp(&right.key));
        manifest
            .tombstones
            .sort_by(|left, right| left.key.cmp(&right.key));
        blobs.sort_by(|left, right| left.key.cmp(&right.key));
        let export = Self {
            schema_version: SYNC_EXPORT_SCHEMA_VERSION,
            export_id: export_id.into(),
            source_instance_id: source_instance_id.into(),
            workspace_id: manifest.workspace_id.clone(),
            resource_id: manifest.resource_id.clone(),
            base_generation,
            current_generation: manifest.generation,
            manifest_sha256: canonical_manifest_checksum(&manifest)?,
            blob_set_sha256: blob_set_checksum(&blobs)?,
            manifest,
            revisions,
            blobs,
            created_at_unix_ms,
            tool_version: tool_version.into(),
        };
        export.validate()?;
        Ok(export)
    }

    pub fn validate(&self) -> Result<(), Problem> {
        if self.schema_version != SYNC_EXPORT_SCHEMA_VERSION {
            return Err(invalid(
                "sync_export_schema_unsupported",
                "Sync export schema unsupported",
            ));
        }
        if self.export_id.trim().is_empty() || self.source_instance_id.trim().is_empty() {
            return Err(invalid(
                "sync_export_identity_invalid",
                "Sync export identity invalid",
            ));
        }
        if self.created_at_unix_ms == 0 || self.tool_version.trim().is_empty() {
            return Err(invalid(
                "sync_export_metadata_invalid",
                "Sync export metadata invalid",
            ));
        }
        if self.workspace_id != self.manifest.workspace_id
            || self.resource_id != self.manifest.resource_id
            || self.current_generation != self.manifest.generation
        {
            return Err(invalid(
                "sync_export_scope_mismatch",
                "Sync export scope mismatch",
            ));
        }
        self.manifest.validate()?;
        if canonical_manifest_checksum(&self.manifest)? != self.manifest_sha256 {
            return Err(invalid(
                "sync_export_manifest_checksum_mismatch",
                "Sync export manifest checksum mismatch",
            ));
        }
        validate_revisions(&self.revisions, &self.manifest, self.base_generation)?;
        validate_blobs(&self.blobs, &self.manifest)?;
        if blob_set_checksum(&self.blobs)? != self.blob_set_sha256 {
            return Err(invalid(
                "sync_export_blob_set_checksum_mismatch",
                "Sync export blob set checksum mismatch",
            ));
        }
        Ok(())
    }

    pub fn import_manifest(&self, fencing_token: u64) -> Result<SyncManifest, Problem> {
        if fencing_token == 0 {
            return Err(invalid(
                "sync_import_fencing_token_invalid",
                "Import fencing token invalid",
            ));
        }
        self.validate()?;
        let mut manifest = self.manifest.clone();
        manifest.fencing_token = fencing_token;
        manifest.validate()?;
        Ok(manifest)
    }
}

pub fn canonical_manifest_checksum(manifest: &SyncManifest) -> Result<String, Problem> {
    let mut canonical = manifest.clone();
    canonical.fencing_token = 0;
    canonical
        .objects
        .sort_by(|left, right| left.key.cmp(&right.key));
    canonical
        .tombstones
        .sort_by(|left, right| left.key.cmp(&right.key));
    let bytes = serde_json::to_vec(&canonical).map_err(|_| {
        invalid(
            "sync_export_serialization_failed",
            "Sync manifest serialization failed",
        )
    })?;
    Ok(checksum(&bytes))
}

pub fn blob_set_checksum(blobs: &[SyncExportBlob]) -> Result<String, Problem> {
    let mut ordered = blobs.to_vec();
    ordered.sort_by(|left, right| left.key.cmp(&right.key));
    let mut bytes = Vec::new();
    for blob in ordered {
        bytes.extend_from_slice(blob.key.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&blob.size.to_be_bytes());
        bytes.extend_from_slice(blob.checksum.as_bytes());
        bytes.push(0);
    }
    Ok(checksum(&bytes))
}

fn validate_blobs(blobs: &[SyncExportBlob], manifest: &SyncManifest) -> Result<(), Problem> {
    let expected = manifest
        .objects
        .iter()
        .map(|object| (object.key.as_str(), (object.size, object.checksum.as_str())))
        .collect::<BTreeMap<_, _>>();
    let mut seen = BTreeSet::new();
    for blob in blobs {
        if !seen.insert(blob.key.as_str()) {
            return Err(invalid(
                "sync_export_duplicate_blob",
                "Sync export contains duplicate blob",
            ));
        }
        let Some((size, object_checksum)) = expected.get(blob.key.as_str()) else {
            return Err(invalid(
                "sync_export_blob_not_in_manifest",
                "Sync export blob is not in manifest",
            ));
        };
        if *size != blob.size
            || *object_checksum != blob.checksum.as_str()
            || blob.data.len() as u64 != blob.size
            || checksum(&blob.data) != blob.checksum
        {
            return Err(invalid(
                "sync_export_blob_checksum_mismatch",
                "Sync export blob checksum mismatch",
            ));
        }
    }
    if seen.len() != expected.len() {
        return Err(invalid(
            "sync_export_blob_missing",
            "Sync export blob is missing",
        ));
    }
    Ok(())
}

fn validate_revisions(
    revisions: &[SyncRevision],
    manifest: &SyncManifest,
    base_generation: Option<u64>,
) -> Result<(), Problem> {
    let mut previous = base_generation;
    for revision in revisions {
        if revision.generation == 0
            || revision.parent_generation != previous
            || !valid_checksum(&revision.manifest_sha256)
        {
            return Err(invalid(
                "sync_export_revision_invalid",
                "Sync export revision invalid",
            ));
        }
        previous = Some(revision.generation);
    }
    if revisions.last().map(|revision| revision.generation) != Some(manifest.generation)
        || revisions
            .last()
            .map(|revision| revision.manifest_sha256.as_str())
            != Some(canonical_manifest_checksum(manifest)?.as_str())
    {
        return Err(invalid(
            "sync_export_revision_incomplete",
            "Sync export revision history incomplete",
        ));
    }
    Ok(())
}

fn valid_checksum(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn invalid(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 409, "sync-export")
}
