use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use buglogin_domain::{errors::Problem, ids::ProfileId};
use buglogin_storage::{
    blob::{checksum, read_verified, write_verified},
    paths::confined_path,
};
use buglogin_sync::{
    coordination::{CoordinationAuth, CoordinationScope},
    manifest::{
        ManifestDependency, ManifestObject, ManifestObjectReference, ManifestTombstone,
        SyncManifest,
    },
    payload::{PAYLOAD_SCHEMA_VERSION, PayloadDescriptor, PayloadObject},
};
use serde::{Deserialize, Serialize};

use crate::remote_api::{RemoteApiClient, RemoteProxySyncSnapshot};

const MARKER: &str = ".buglogin-projection.json";

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

    pub async fn hydrate(
        &self,
        client: &RemoteApiClient,
        auth: &CoordinationAuth,
        profile_id: &ProfileId,
        manifest: &SyncManifest,
        correlation_id: &str,
    ) -> Result<ProjectionReceipt, Problem> {
        let descriptor = descriptor_from_manifest(manifest)?;
        let mut objects = Vec::with_capacity(descriptor.objects.len());
        for object in &descriptor.objects {
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
            objects.push((object.path.clone(), bytes));
        }
        self.publish(profile_id, manifest, &descriptor, objects)
    }

    pub fn publish(
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

    pub fn snapshot(
        &self,
        profile_path: &Path,
        previous: &SyncManifest,
        fencing_token: u64,
    ) -> Result<(PayloadDescriptor, Vec<(String, Vec<u8>)>, SyncManifest), Problem> {
        self.snapshot_with_proxy(profile_path, previous, fencing_token, None)
    }

    pub fn snapshot_with_proxy(
        &self,
        profile_path: &Path,
        previous: &SyncManifest,
        fencing_token: u64,
        proxy: Option<RemoteProxySyncSnapshot>,
    ) -> Result<(PayloadDescriptor, Vec<(String, Vec<u8>)>, SyncManifest), Problem> {
        let mut files = Vec::new();
        collect_files(profile_path, profile_path, &mut files)?;
        let proxy_dependency = proxy.map(|proxy| {
            let key = format!(
                ".buglogin/proxy-dependencies/{}/{}/metadata.json",
                proxy.proxy_id.as_str(),
                proxy.revision
            );
            files.push((key.clone(), proxy.metadata));
            (proxy.proxy_id, proxy.revision, key)
        });
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
                })
                .collect(),
            tombstones,
        )?;
        if let Some((proxy_id, revision, key)) = proxy_dependency {
            let object = manifest
                .objects
                .iter()
                .find(|object| object.key == key)
                .ok_or_else(|| unavailable())?;
            manifest.dependencies.push(ManifestDependency::Proxy {
                proxy_id,
                revision,
                metadata_object: ManifestObjectReference::from(object),
                credential_envelope_object: None,
            });
            manifest.validate()?;
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
        output.push((relative, fs::read(&path).map_err(|_| unavailable())?));
    }
    Ok(())
}

fn unavailable() -> Problem {
    problem(
        "profile_projection_unavailable",
        "Profile projection is unavailable",
    )
}

fn problem(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 503, "desktop-projection")
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_domain::ids::{ProxyId, ResourceId, WorkspaceId};
    use buglogin_sync::manifest::ManifestObject;

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

        let (next_descriptor, files, next_manifest) =
            projection.snapshot(&receipt.path, &manifest, 2).unwrap();

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
    fn snapshot_adds_selected_proxy_metadata_as_a_manifest_dependency() {
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
        let metadata =
            br#"{"workspaceId":"ws_projection","proxy":{"id":"proxy_projection"},"revision":4}"#
                .to_vec();

        let (descriptor, files, manifest) = projection
            .snapshot_with_proxy(
                &profile_path,
                &previous,
                2,
                Some(RemoteProxySyncSnapshot {
                    proxy_id: proxy_id.clone(),
                    revision: 4,
                    metadata: metadata.clone(),
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
        assert_eq!(metadata_object.size, metadata.len() as u64);
        assert!(manifest.objects.iter().any(|object| {
            object.key == metadata_object.key && object.checksum == metadata_object.checksum
        }));
        assert_eq!(
            files
                .iter()
                .find(|(path, _)| path == &metadata_object.key)
                .unwrap()
                .1,
            metadata
        );
        assert_eq!(descriptor.generation, manifest.generation);
        let _ = fs::remove_dir_all(root);
    }
}
