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
                blobs: export.blobs.clone(),
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
        manifest: export.import_manifest(destination_fencing_token)?,
        blobs: export.blobs.clone(),
    })
}

fn same_state(left: &SyncManifest, right: &SyncManifest) -> bool {
    left.workspace_id == right.workspace_id
        && left.resource_id == right.resource_id
        && left.generation == right.generation
        && left.parent_generation == right.parent_generation
        && left.root_checksum == right.root_checksum
        && left.objects == right.objects
        && left.tombstones == right.tombstones
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
