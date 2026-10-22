use crate::{
    export::{SyncExport, SyncExportBlob},
    manifest::SyncManifest,
};
use buglogin_domain::errors::Problem;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncImportPlan {
    pub export_id: String,
    pub replayed: bool,
    pub manifest: SyncManifest,
    pub blobs: Vec<SyncExportBlob>,
}

pub fn plan_sync_import(
    export: &SyncExport,
    current: Option<&SyncManifest>,
    destination_fencing_token: u64,
) -> Result<SyncImportPlan, Problem> {
    export.validate()?;
    let mut plan = plan_sync_import_manifest_unchecked(export, current, destination_fencing_token)?;
    plan.blobs = export.blobs.clone();
    Ok(plan)
}

/// Builds a plan without copying export payloads. Call only after full export validation.
pub fn plan_sync_import_manifest(
    export: &SyncExport,
    current: Option<&SyncManifest>,
    destination_fencing_token: u64,
) -> Result<SyncImportPlan, Problem> {
    export.validate_metadata()?;
    plan_sync_import_manifest_unchecked(export, current, destination_fencing_token)
}

fn plan_sync_import_manifest_unchecked(
    export: &SyncExport,
    current: Option<&SyncManifest>,
    destination_fencing_token: u64,
) -> Result<SyncImportPlan, Problem> {
    if destination_fencing_token == 0 {
        return Err(problem(
            "sync_import_fencing_token_invalid",
            "Import fencing token invalid",
        ));
    }

    if let Some(current) = current {
        if same_state(current, &export.manifest) {
            return Ok(SyncImportPlan {
                export_id: export.export_id.clone(),
                replayed: true,
                manifest: current.clone(),
                blobs: Vec::new(),
            });
        }
        if export.base_generation != Some(current.generation)
            || export.manifest.parent_generation != Some(current.generation)
        {
            return Err(problem(
                "sync_import_base_conflict",
                "Sync import base generation conflicts with destination",
            ));
        }
        if export.manifest.generation <= current.generation {
            return Err(problem(
                "sync_import_generation_conflict",
                "Sync import generation conflicts with destination",
            ));
        }
    } else if export.base_generation.is_some()
        || export.manifest.parent_generation.is_some()
        || export.manifest.generation != 1
    {
        return Err(problem(
            "sync_import_base_missing",
            "Sync import requires a destination base manifest",
        ));
    }

    Ok(SyncImportPlan {
        export_id: export.export_id.clone(),
        replayed: false,
        manifest: import_manifest_metadata(export, destination_fencing_token)?,
        blobs: Vec::new(),
    })
}

fn import_manifest_metadata(
    export: &SyncExport,
    fencing_token: u64,
) -> Result<SyncManifest, Problem> {
    let mut manifest = export.manifest.clone();
    manifest.fencing_token = fencing_token;
    // Every exported object has its bytes in the package and is staged under
    // the destination manifest generation. Source generations are source-store
    // locations and must not be carried into the destination manifest.
    for object in &mut manifest.objects {
        object.source_generation = None;
    }
    manifest.validate()?;
    Ok(manifest)
}

fn same_state(left: &SyncManifest, right: &SyncManifest) -> bool {
    left.schema_version == right.schema_version
        && left.workspace_id == right.workspace_id
        && left.resource_id == right.resource_id
        && left.generation == right.generation
        && left.parent_generation == right.parent_generation
        && left.root_checksum == right.root_checksum
        && left.objects.len() == right.objects.len()
        && left
            .objects
            .iter()
            .zip(&right.objects)
            .all(|(left, right)| {
                left.key == right.key && left.size == right.size && left.checksum == right.checksum
            })
        && left.tombstones == right.tombstones
        && left.dependencies == right.dependencies
        && left.payload_encryption == right.payload_encryption
        && left.file_chunks == right.file_chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        ManifestDependency, ManifestObject, ManifestObjectReference, ManifestPayloadEncryption,
        PayloadEncryptionAlgorithm,
    };
    use buglogin_domain::ids::{ProxyId, ResourceId, WorkspaceId};

    fn manifest_with_envelope() -> SyncManifest {
        SyncManifest::new(
            WorkspaceId::new("ws-import-replay").unwrap(),
            ResourceId::new("res-import-replay").unwrap(),
            4,
            2,
            Some(1),
            b"same-root",
            vec![
                ManifestObject::new(".buglogin/resource-key-envelope.json", b"envelope").unwrap(),
                ManifestObject::new("profile/state", b"payload").unwrap(),
            ],
        )
        .unwrap()
    }

    #[test]
    fn replay_identity_includes_dependencies_and_payload_encryption() {
        let base = manifest_with_envelope();

        let mut changed_dependency = base.clone();
        changed_dependency
            .dependencies
            .push(ManifestDependency::Proxy {
                proxy_id: ProxyId::new("proxy-import-replay").unwrap(),
                revision: 1,
                credential_version: None,
                metadata_object: None,
                credential_envelope_object: None,
            });
        assert!(!same_state(&base, &changed_dependency));

        let envelope = ManifestObjectReference::from(&base.objects[0]);
        let mut changed_encryption = base.clone();
        changed_encryption.payload_encryption = Some(ManifestPayloadEncryption {
            schema_version: 1,
            algorithm: PayloadEncryptionAlgorithm::Aes256GcmChunkV1,
            key_version: None,
            resource_key_envelope: envelope,
        });
        assert!(!same_state(&base, &changed_encryption));
    }

    #[test]
    fn replay_identity_includes_manifest_schema_version() {
        let base = manifest_with_envelope();
        let mut changed_schema = base.clone();
        changed_schema.schema_version += 1;

        assert!(!same_state(&base, &changed_schema));
    }

    #[test]
    fn replay_identity_ignores_source_object_generation() {
        let base = manifest_with_envelope();
        let mut source_rebased = base.clone();
        source_rebased.objects[0].source_generation = Some(1);

        assert!(same_state(&base, &source_rebased));
    }
}

fn problem(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 409, "sync-import")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncStagedObject {
    pub storage_key: String,
    pub checksum: String,
    pub created: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncStageReceipt {
    pub objects: Vec<SyncStagedObject>,
}

pub fn stage_sync_import(
    remote: &mut (impl buglogin_storage::remote::RemoteChunkStore + ?Sized),
    plan: &SyncImportPlan,
) -> Result<SyncStageReceipt, Problem> {
    let mut objects = Vec::with_capacity(plan.blobs.len());
    for blob in &plan.blobs {
        let storage_key = sync_object_storage_key(&plan.manifest, blob);
        match remote.checksum(&storage_key)? {
            Some(existing) if existing == blob.checksum => objects.push(SyncStagedObject {
                storage_key,
                checksum: blob.checksum.clone(),
                created: false,
            }),
            Some(_) => {
                let error = problem(
                    "sync_import_blob_collision",
                    "Sync import blob collides with destination",
                );
                rollback_staged_sync_import(remote, &objects)?;
                return Err(error);
            }
            None => {
                if let Err(error) = remote.put(&storage_key, &blob.data, &blob.checksum) {
                    rollback_staged_sync_import(remote, &objects)?;
                    return Err(error);
                }
                objects.push(SyncStagedObject {
                    storage_key,
                    checksum: blob.checksum.clone(),
                    created: true,
                });
            }
        }
    }
    Ok(SyncStageReceipt { objects })
}

pub fn rollback_staged_sync_import(
    remote: &mut (impl buglogin_storage::remote::RemoteChunkStore + ?Sized),
    receipt: &[SyncStagedObject],
) -> Result<(), Problem> {
    for object in receipt.iter().rev().filter(|object| object.created) {
        remote.delete_if_checksum(&object.storage_key, &object.checksum)?;
    }
    Ok(())
}

pub fn sync_object_storage_key(manifest: &SyncManifest, blob: &SyncExportBlob) -> String {
    format!(
        "profiles/{}/{}/{}/{}",
        manifest.resource_id.as_str(),
        manifest.generation,
        blob.checksum,
        blob.key
    )
}
