use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use buglogin_domain::{entities::Profile, errors::Problem};
use buglogin_storage::local::LocalRepository;
use buglogin_sync::crypto::ClientKey;

use crate::{
    encrypted_data::EncryptedProfileData,
    transform::{NeutralProfileRecord, ensure_directory, io_problem, reject_symlink},
};

const MARKER_PREFIX: &str = ".buglogin-migration-";

pub(crate) struct PreparedProfileData {
    roots: Vec<PathBuf>,
    markers: Vec<PathBuf>,
}

pub(crate) fn plan_profile_data(
    destination_root: &Path,
    grouped: &DecryptedFiles,
) -> Result<(), Problem> {
    if !destination_root.exists() {
        return Ok(());
    }
    reject_symlink(destination_root)?;
    let repository = LocalRepository::open(destination_root)?;
    for (profile_id, files) in grouped {
        let root = destination_root.join("profiles").join(&profile_id);
        match repository.get::<Profile>(&profile_id)? {
            Some(_) => verify_existing(&root, &files)?,
            None if root.exists() => return Err(collision(&profile_id)),
            None => {}
        }
    }
    Ok(())
}

pub(crate) fn prepare_profile_data(
    destination_root: &Path,
    run_id: &str,
    grouped: &DecryptedFiles,
) -> Result<PreparedProfileData, Problem> {
    let repository = LocalRepository::open(destination_root)?;
    let mut roots = Vec::new();
    let mut markers = Vec::new();
    for (profile_id, files) in grouped {
        let root = destination_root.join("profiles").join(&profile_id);
        if repository.get::<Profile>(&profile_id)?.is_some() {
            verify_existing(&root, &files)?;
            remove_marker(&root, run_id)?;
            continue;
        }
        let marker = root.join(format!("{MARKER_PREFIX}{run_id}"));
        if root.exists() {
            reject_symlink(&root)?;
            if !marker.is_file() {
                return Err(collision(&profile_id));
            }
            verify_or_complete(&root, &files)?;
        } else {
            ensure_directory(&root)?;
            write_new(&marker, run_id.as_bytes())?;
            if let Err(error) = verify_or_complete(&root, &files) {
                let _ = fs::remove_dir_all(&root);
                return Err(error);
            }
            roots.push(root.clone());
        }
        markers.push(marker);
    }
    Ok(PreparedProfileData { roots, markers })
}

impl PreparedProfileData {
    pub(crate) fn finalize(self) -> Result<(), Problem> {
        for marker in self.markers {
            if marker.exists() {
                fs::remove_file(&marker)
                    .map_err(|error| io_problem("remove profile data marker", &error))?;
            }
        }
        Ok(())
    }

    pub(crate) fn cleanup(self, destination_root: &Path) -> Result<(), Problem> {
        let repository = LocalRepository::open(destination_root)?;
        for root in self.roots.into_iter().rev() {
            let profile_id = root
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| invalid("migration_profile_data_path_invalid"))?;
            if repository.get::<Profile>(profile_id)?.is_none() && root.exists() {
                fs::remove_dir_all(&root)
                    .map_err(|error| io_problem("cleanup profile data", &error))?;
            }
        }
        Ok(())
    }
}

pub(crate) type DecryptedFiles = BTreeMap<String, Vec<(PathBuf, Vec<u8>)>>;

pub(crate) fn decrypt_profile_files(
    profiles: &[NeutralProfileRecord],
    data: &EncryptedProfileData,
    key: &ClientKey,
) -> Result<DecryptedFiles, Problem> {
    let mut source_to_profile = BTreeMap::new();
    for profile in profiles {
        if source_to_profile
            .insert(profile.source_ref.id.as_str(), profile.id.as_str())
            .is_some()
        {
            return Err(invalid("migration_profile_data_source_duplicate"));
        }
    }
    let mut paths = BTreeSet::new();
    let mut grouped: DecryptedFiles = BTreeMap::new();
    for file in &data.index.files {
        let (source_id, relative) = parse_source_path(&file.source_path)?;
        let profile_id = source_to_profile
            .get(source_id.as_str())
            .ok_or_else(|| invalid("migration_profile_data_profile_missing"))?;
        let binding = format!(
            "{profile_id}/{}",
            relative.to_string_lossy().replace('\\', "/")
        );
        if !paths.insert(binding) {
            return Err(invalid("migration_profile_data_path_duplicate"));
        }
        grouped
            .entry((*profile_id).to_owned())
            .or_default()
            .push((relative, data.decrypt_file(&file.source_path, key)?));
    }
    Ok(grouped)
}

fn parse_source_path(value: &str) -> Result<(String, PathBuf), Problem> {
    if value.contains('\\') || value.starts_with('/') || value.ends_with('/') {
        return Err(invalid("migration_profile_data_path_invalid"));
    }
    let parts = value.split('/').collect::<Vec<_>>();
    if parts.len() < 4 || parts[0] != "profiles" || parts[1].is_empty() || parts[2] != "profile" {
        return Err(invalid("migration_profile_data_path_invalid"));
    }
    let mut relative = PathBuf::new();
    for part in &parts[3..] {
        if part.is_empty() || *part == "." || *part == ".." {
            return Err(invalid("migration_profile_data_path_invalid"));
        }
        relative.push(part);
    }
    Ok((parts[1].to_owned(), relative))
}

fn verify_existing(root: &Path, files: &[(PathBuf, Vec<u8>)]) -> Result<(), Problem> {
    reject_symlink(root)?;
    for (relative, expected) in files {
        let target = root.join("profile").join(relative);
        let actual = fs::read(&target).map_err(|_| collision_path(&target))?;
        if actual != *expected {
            return Err(collision_path(&target));
        }
    }
    Ok(())
}

fn verify_or_complete(root: &Path, files: &[(PathBuf, Vec<u8>)]) -> Result<(), Problem> {
    for (relative, expected) in files {
        let target = root.join("profile").join(relative);
        if target.exists() {
            let actual =
                fs::read(&target).map_err(|error| io_problem("read profile data", &error))?;
            if actual != *expected {
                return Err(collision_path(&target));
            }
            continue;
        }
        let parent = target
            .parent()
            .ok_or_else(|| invalid("migration_profile_data_path_invalid"))?;
        ensure_directory(parent)?;
        write_new(&target, expected)?;
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Problem> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_problem("create profile data", &error))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| io_problem("write profile data", &error))
}

fn remove_marker(root: &Path, run_id: &str) -> Result<(), Problem> {
    let marker = root.join(format!("{MARKER_PREFIX}{run_id}"));
    if marker.exists() {
        fs::remove_file(marker)
            .map_err(|error| io_problem("remove profile data marker", &error))?;
    }
    Ok(())
}

fn collision(profile_id: &str) -> Problem {
    Problem::new(
        "migration_profile_data_collision",
        format!("Profile data collision for {profile_id}"),
        409,
        "migration",
    )
}

fn collision_path(path: &Path) -> Problem {
    let _ = path;
    invalid("migration_profile_data_collision")
}

fn invalid(code: &'static str) -> Problem {
    Problem::new(code, "Migration profile data is invalid", 409, "migration")
}
