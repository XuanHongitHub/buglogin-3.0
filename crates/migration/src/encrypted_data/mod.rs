use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use buglogin_domain::errors::Problem;
use buglogin_storage::blob::checksum;
use buglogin_sync::{
    chunks::DEFAULT_CHUNK_BYTES,
    crypto::{AuthenticatedChunk, ClientKey},
};
use serde::{Deserialize, Serialize};

use crate::package::NeutralPackage;

const INDEX_PATH: &str = "encrypted/profile-data/index.json";
const OBJECT_PREFIX: &str = "encrypted/profile-data";
const INDEX_SCHEMA: &str = "buglogin.encrypted-profile-data/2";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedProfileDataIndex {
    pub schema: String,
    pub files: Vec<EncryptedProfileFile>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedProfileFile {
    pub source_path: String,
    pub bytes: u64,
    pub chunks: Vec<EncryptedProfileChunk>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedProfileChunk {
    pub index: u64,
    pub object_key: String,
    pub bytes: u64,
    pub ciphertext_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncryptedProfileData {
    pub index: EncryptedProfileDataIndex,
    pub objects: BTreeMap<String, Vec<u8>>,
}

impl EncryptedProfileData {
    pub fn decrypt_file(&self, source_path: &str, key: &ClientKey) -> Result<Vec<u8>, Problem> {
        let file = self
            .index
            .files
            .iter()
            .find(|file| file.source_path == source_path)
            .ok_or_else(|| data_problem("migration_encrypted_profile_missing"))?;
        decrypt_file(file, &self.objects, key)
    }

    pub fn file_mut(&mut self, object_key: &str) -> Option<&mut Vec<u8>> {
        self.objects.get_mut(object_key)
    }
}

pub fn encrypt_legacy_profile_data(
    source_root: &Path,
    key: &ClientKey,
) -> Result<EncryptedProfileData, Problem> {
    let mut files = Vec::new();
    let mut objects = BTreeMap::new();
    let profiles_root = source_root.join("profiles");
    if profiles_root.exists() {
        let mut profile_dirs = read_profile_dirs(&profiles_root)?;
        profile_dirs.sort();
        for profile_id in profile_dirs {
            let profile_root = profiles_root.join(&profile_id).join("profile");
            if profile_root.exists() {
                collect_files(source_root, &profile_root, &mut files, &mut objects, key)?;
            }
        }
    }
    files.sort_by(|left: &EncryptedProfileFile, right| left.source_path.cmp(&right.source_path));
    Ok(EncryptedProfileData {
        index: EncryptedProfileDataIndex {
            schema: INDEX_SCHEMA.to_owned(),
            files,
        },
        objects,
    })
}

pub fn decrypt_packaged_profile_data(
    package: &NeutralPackage,
    key: &ClientKey,
) -> Result<EncryptedProfileData, Problem> {
    let index: EncryptedProfileDataIndex = serde_json::from_slice(
        package
            .file(INDEX_PATH)
            .ok_or_else(|| data_problem("migration_encrypted_profile_index_missing"))?,
    )
    .map_err(|_| data_problem("migration_encrypted_profile_index_invalid"))?;
    if index.schema != INDEX_SCHEMA {
        return Err(data_problem("migration_encrypted_profile_index_invalid"));
    }
    let objects = index
        .files
        .iter()
        .flat_map(|file| file.chunks.iter())
        .map(|chunk| {
            let bytes = package
                .file(&chunk.object_key)
                .ok_or_else(|| data_problem("migration_encrypted_profile_chunk_missing"))?;
            if checksum(bytes) != chunk.ciphertext_sha256 {
                return Err(data_problem("migration_encrypted_profile_chunk_tampered"));
            }
            Ok((chunk.object_key.clone(), bytes.to_vec()))
        })
        .collect::<Result<BTreeMap<_, _>, Problem>>()?;
    let expected = objects
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    let actual = package
        .files()
        .keys()
        .filter(|path| {
            path.starts_with(&format!("{OBJECT_PREFIX}/")) && path.as_str() != INDEX_PATH
        })
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    if actual != expected {
        return Err(data_problem(
            "migration_encrypted_profile_object_set_mismatch",
        ));
    }
    let data = EncryptedProfileData { index, objects };
    for file in &data.index.files {
        decrypt_file(file, &data.objects, key)?;
    }
    Ok(data)
}

pub(crate) const fn index_path() -> &'static str {
    INDEX_PATH
}

fn collect_files(
    source_root: &Path,
    directory: &Path,
    files: &mut Vec<EncryptedProfileFile>,
    objects: &mut BTreeMap<String, Vec<u8>>,
    key: &ClientKey,
) -> Result<(), Problem> {
    let mut entries = fs::read_dir(directory)
        .map_err(|_| data_problem("migration_encrypted_profile_read_failed"))?
        .map(|entry| entry.map_err(|_| data_problem("migration_encrypted_profile_read_failed")))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|_| data_problem("migration_encrypted_profile_read_failed"))?;
        if file_type.is_symlink() {
            return Err(data_problem(
                "migration_encrypted_profile_symlink_unsupported",
            ));
        }
        if file_type.is_dir() {
            collect_files(source_root, &path, files, objects, key)?;
            continue;
        }
        if !file_type.is_file() {
            return Err(data_problem(
                "migration_encrypted_profile_entry_unsupported",
            ));
        }
        let source_path = path
            .strip_prefix(source_root)
            .map_err(|_| data_problem("migration_encrypted_profile_path_invalid"))?
            .to_string_lossy()
            .replace('\\', "/");
        let plaintext =
            fs::read(&path).map_err(|_| data_problem("migration_encrypted_profile_read_failed"))?;
        let prefix = format!("{OBJECT_PREFIX}/{}", checksum(source_path.as_bytes()));
        let chunks = plaintext
            .chunks(DEFAULT_CHUNK_BYTES)
            .enumerate()
            .map(|(index, chunk)| {
                let object_key = format!("{prefix}/{index:016x}");
                let bytes =
                    AuthenticatedChunk::encrypt(key, chunk, object_key.as_bytes())?.encoded();
                let encrypted = EncryptedProfileChunk {
                    index: index as u64,
                    object_key: object_key.clone(),
                    bytes: chunk.len() as u64,
                    ciphertext_sha256: checksum(&bytes),
                };
                objects.insert(object_key, bytes);
                Ok(encrypted)
            })
            .collect::<Result<Vec<_>, Problem>>()?;
        files.push(EncryptedProfileFile {
            source_path,
            bytes: plaintext.len() as u64,
            chunks,
        });
    }
    Ok(())
}

fn decrypt_file(
    file: &EncryptedProfileFile,
    objects: &BTreeMap<String, Vec<u8>>,
    key: &ClientKey,
) -> Result<Vec<u8>, Problem> {
    let expected_prefix = format!("{OBJECT_PREFIX}/{}", checksum(file.source_path.as_bytes()));
    let chunks = file
        .chunks
        .iter()
        .enumerate()
        .map(|(expected_index, chunk)| {
            let expected_object_key = format!("{expected_prefix}/{expected_index:016x}");
            if chunk.index != expected_index as u64 || chunk.object_key != expected_object_key {
                return Err(data_problem("migration_encrypted_profile_path_mismatch"));
            }
            let bytes = objects
                .get(&chunk.object_key)
                .ok_or_else(|| data_problem("migration_encrypted_profile_chunk_missing"))?;
            if checksum(bytes) != chunk.ciphertext_sha256 {
                return Err(data_problem("migration_encrypted_profile_chunk_tampered"));
            }
            let plaintext =
                AuthenticatedChunk::decode(bytes)?.decrypt(key, chunk.object_key.as_bytes())?;
            if plaintext.len() as u64 != chunk.bytes {
                return Err(data_problem("migration_encrypted_profile_size_mismatch"));
            }
            Ok(plaintext)
        })
        .collect::<Result<Vec<_>, Problem>>()?;
    let mut plaintext = Vec::with_capacity(file.bytes as usize);
    for chunk in chunks {
        plaintext.extend_from_slice(&chunk);
    }
    if plaintext.len() as u64 != file.bytes {
        return Err(data_problem(
            "migration_encrypted_profile_checksum_mismatch",
        ));
    }
    Ok(plaintext)
}

fn read_profile_dirs(root: &Path) -> Result<Vec<String>, Problem> {
    fs::read_dir(root)
        .map_err(|_| data_problem("migration_encrypted_profile_read_failed"))?
        .map(|entry| {
            let entry =
                entry.map_err(|_| data_problem("migration_encrypted_profile_read_failed"))?;
            let file_type = entry
                .file_type()
                .map_err(|_| data_problem("migration_encrypted_profile_read_failed"))?;
            if file_type.is_symlink() || !file_type.is_dir() {
                return Err(data_problem(
                    "migration_encrypted_profile_entry_unsupported",
                ));
            }
            Ok(entry.file_name().to_string_lossy().into_owned())
        })
        .collect()
}

fn data_problem(code: &'static str) -> Problem {
    Problem::new(
        code,
        "Encrypted migration profile data operation failed",
        409,
        "migration",
    )
}
