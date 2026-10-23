use std::collections::BTreeSet;

use buglogin_domain::{
    errors::Problem,
    ids::{ResourceId, WorkspaceId},
};
use buglogin_storage::blob::checksum;
use serde::{Deserialize, Serialize};

use crate::manifest::{ManifestFileChunks, SyncManifest};

pub const PAYLOAD_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadObject {
    pub path: String,
    pub size: u64,
    pub checksum: String,
}

impl PayloadObject {
    pub fn new(path: impl Into<String>, bytes: &[u8]) -> Result<Self, Problem> {
        let object = Self {
            path: path.into(),
            size: bytes.len() as u64,
            checksum: checksum(bytes),
        };
        object.validate()?;
        Ok(object)
    }

    pub fn verify(&self, bytes: &[u8]) -> Result<(), Problem> {
        self.validate()?;
        if self.size != bytes.len() as u64 || self.checksum != checksum(bytes) {
            return Err(problem(
                "payload_object_checksum_mismatch",
                "Payload object checksum mismatch",
            ));
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), Problem> {
        validate_path(&self.path)?;
        if !valid_checksum(&self.checksum) {
            return Err(problem(
                "payload_object_checksum_invalid",
                "Payload object checksum is invalid",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PayloadDescriptor {
    pub schema_version: u16,
    pub workspace_id: WorkspaceId,
    pub resource_id: ResourceId,
    pub generation: u64,
    pub root_checksum: String,
    pub objects: Vec<PayloadObject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_chunks: Vec<ManifestFileChunks>,
}

impl PayloadDescriptor {
    pub fn new(
        workspace_id: WorkspaceId,
        resource_id: ResourceId,
        generation: u64,
        objects: Vec<PayloadObject>,
    ) -> Result<Self, Problem> {
        Self::new_with_file_chunks(workspace_id, resource_id, generation, objects, Vec::new())
    }

    pub fn new_with_file_chunks(
        workspace_id: WorkspaceId,
        resource_id: ResourceId,
        generation: u64,
        mut objects: Vec<PayloadObject>,
        mut file_chunks: Vec<ManifestFileChunks>,
    ) -> Result<Self, Problem> {
        objects.sort_by(|left, right| left.path.cmp(&right.path));
        file_chunks.sort_by(|left, right| left.path.cmp(&right.path));
        let mut descriptor = Self {
            schema_version: PAYLOAD_SCHEMA_VERSION,
            workspace_id,
            resource_id,
            generation,
            root_checksum: String::new(),
            objects,
            file_chunks,
        };
        descriptor.validate_without_root()?;
        descriptor.root_checksum = checksum(&descriptor.canonical_root_bytes());
        Ok(descriptor)
    }

    pub fn validate(&self) -> Result<(), Problem> {
        self.validate_without_root()?;
        let canonical_root_matches = checksum(&self.canonical_root_bytes()) == self.root_checksum;
        // Profiles created before payload descriptors were introduced use SHA-256(empty)
        // for their generation-1, zero-object manifest. Keep that immutable genesis
        // readable so those profiles can still be launched and then synced forward.
        let legacy_empty_genesis_matches = self.generation == 1
            && self.objects.is_empty()
            && self.file_chunks.is_empty()
            && checksum(&[]) == self.root_checksum;
        if !valid_checksum(&self.root_checksum)
            || (!canonical_root_matches && !legacy_empty_genesis_matches)
        {
            return Err(problem(
                "payload_root_checksum_mismatch",
                "Payload root checksum mismatch",
            ));
        }
        Ok(())
    }

    pub fn canonical_root_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"buglogin-payload-v1\0");
        bytes.extend_from_slice(self.workspace_id.as_str().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(self.resource_id.as_str().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&self.generation.to_be_bytes());
        for object in &self.objects {
            bytes.extend_from_slice(&(object.path.len() as u64).to_be_bytes());
            bytes.extend_from_slice(object.path.as_bytes());
            bytes.extend_from_slice(&object.size.to_be_bytes());
            bytes.extend_from_slice(object.checksum.as_bytes());
        }
        if !self.file_chunks.is_empty() {
            bytes.extend_from_slice(b"buglogin-payload-file-chunks-v1\0");
            let mut files = self.file_chunks.clone();
            files.sort_by(|left, right| left.path.cmp(&right.path));
            for file in files {
                append_text(&mut bytes, &file.path);
                bytes.extend_from_slice(&file.plaintext_size.to_be_bytes());
                append_text(&mut bytes, &file.plaintext_checksum);
                bytes.extend_from_slice(&(file.chunks.len() as u64).to_be_bytes());
                for chunk in file.chunks {
                    bytes.extend_from_slice(&chunk.index.to_be_bytes());
                    bytes.extend_from_slice(&chunk.plaintext_size.to_be_bytes());
                    append_text(&mut bytes, &chunk.plaintext_checksum);
                    append_text(&mut bytes, &chunk.object.key);
                    bytes.extend_from_slice(&chunk.object.size.to_be_bytes());
                    append_text(&mut bytes, &chunk.object.checksum);
                }
            }
        }
        bytes
    }

    pub fn verify_manifest(&self, manifest: &SyncManifest) -> Result<(), Problem> {
        self.validate()?;
        if self.workspace_id != manifest.workspace_id
            || self.resource_id != manifest.resource_id
            || self.generation != manifest.generation
            || self.root_checksum != manifest.root_checksum
            || self.objects.len() != manifest.objects.len()
            || self.file_chunks != manifest.file_chunks
        {
            return Err(problem(
                "payload_manifest_mismatch",
                "Payload descriptor does not match manifest",
            ));
        }
        for (payload, object) in self.objects.iter().zip(&manifest.objects) {
            if payload.path != object.key
                || payload.size != object.size
                || payload.checksum != object.checksum
            {
                return Err(problem(
                    "payload_manifest_mismatch",
                    "Payload descriptor does not match manifest",
                ));
            }
        }
        Ok(())
    }

    fn validate_without_root(&self) -> Result<(), Problem> {
        if self.schema_version != PAYLOAD_SCHEMA_VERSION {
            return Err(problem(
                "payload_schema_unsupported",
                "Payload schema is unsupported",
            ));
        }
        if self.generation == 0 {
            return Err(problem(
                "payload_generation_invalid",
                "Payload generation is invalid",
            ));
        }
        let mut paths = BTreeSet::new();
        let mut previous: Option<&str> = None;
        for object in &self.objects {
            object.validate()?;
            if previous.is_some_and(|value| value >= object.path.as_str())
                || !paths.insert(object.path.as_str())
            {
                return Err(problem(
                    "payload_object_order_invalid",
                    "Payload objects must be unique and ordered",
                ));
            }
            previous = Some(&object.path);
        }
        let mut previous_file_path: Option<&str> = None;
        for file in &self.file_chunks {
            validate_path(&file.path)?;
            if previous_file_path.is_some_and(|previous| previous >= file.path.as_str()) {
                return Err(problem(
                    "payload_file_chunks_order_invalid",
                    "Payload file chunk mappings must be unique and ordered",
                ));
            }
            previous_file_path = Some(&file.path);
        }
        Ok(())
    }
}

fn append_text(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u64).to_be_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

fn validate_path(path: &str) -> Result<(), Problem> {
    if path.is_empty()
        || path.len() > 512
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || path.contains(':')
        || path.starts_with(".buglogin-")
    {
        return Err(problem("payload_path_invalid", "Payload path is invalid"));
    }
    Ok(())
}

fn valid_checksum(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn problem(code: &'static str, title: &'static str) -> Problem {
    Problem::new(code, title, 409, "sync-payload")
}
