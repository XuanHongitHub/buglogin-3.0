use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

use buglogin_domain::errors::Problem;
use serde::{Deserialize, Serialize};

use crate::{
    profile_data_activation::DecryptedFiles,
    transform::{NeutralProfileRecord, ensure_directory, io_problem, reject_symlink},
};

const ROOT: &str = "migration-preserved";
const MARKER_PREFIX: &str = ".buglogin-migration-";

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PreservedProfilePlan {
    pub create_ids: Vec<String>,
    pub reuse_ids: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PreservedRollbackReceipt {
    pub quarantined_ids: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreservedRecord {
    schema: String,
    id: String,
    source_ref: crate::transform::ProfileSourceRef,
    name: String,
    runtime: String,
    migration_class: String,
    source_data_object: crate::transform::ProfileDataObject,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreservedJournalEntry {
    profile_id: String,
    record_sha256: String,
    root_sha256: String,
    created: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreservedJournal {
    schema: String,
    run_id: String,
    package_id: String,
    entries: Vec<PreservedJournalEntry>,
}

pub(crate) struct PreparedPreservedProfiles {
    root: PathBuf,
    journal: PreservedJournal,
    markers: Vec<PathBuf>,
    created_roots: Vec<PathBuf>,
    created_count: usize,
    reused_count: usize,
}

pub(crate) fn plan_preserved_profiles(
    destination_root: &Path,
    profiles: &[NeutralProfileRecord],
    files: &DecryptedFiles,
) -> Result<PreservedProfilePlan, Problem> {
    let mut plan = PreservedProfilePlan::default();
    for profile in sorted_profiles(profiles) {
        let root = profile_root(destination_root, &profile.id);
        if root.exists() {
            verify_root(&root, profile, profile_files(files, &profile.id), None)?;
            plan.reuse_ids.push(profile.id.clone());
        } else {
            plan.create_ids.push(profile.id.clone());
        }
    }
    Ok(plan)
}

pub(crate) fn prepare_preserved_profiles(
    destination_root: &Path,
    run_id: &str,
    package_id: &str,
    profiles: &[NeutralProfileRecord],
    files: &DecryptedFiles,
) -> Result<PreparedPreservedProfiles, Problem> {
    let authority_root = destination_root.join(ROOT);
    let existing_journal = read_journal_if_exists(&authority_root, run_id)?;
    let is_rerun = existing_journal.is_some();
    let mut entries = Vec::new();
    let mut markers = Vec::new();
    let mut created_roots = Vec::new();
    for profile in sorted_profiles(profiles) {
        let root = profile_root(destination_root, &profile.id);
        let marker = root.join(format!("{MARKER_PREFIX}{run_id}"));
        let record_bytes = record_bytes(profile)?;
        let created = if root.exists() {
            reject_symlink(&root)?;
            if marker.exists() {
                verify_root(
                    &root,
                    profile,
                    profile_files(files, &profile.id),
                    Some(&marker),
                )?;
                true
            } else {
                verify_root(&root, profile, profile_files(files, &profile.id), None)?;
                false
            }
        } else {
            if existing_journal.is_some() {
                return Err(invalid("migration_preserved_profile_missing"));
            }
            ensure_directory(&root.join("profile"))?;
            write_new(&marker, run_id.as_bytes())?;
            write_new(&root.join("record.json"), &record_bytes)?;
            write_files(&root, profile_files(files, &profile.id))?;
            created_roots.push(root.clone());
            true
        };
        if created {
            markers.push(marker);
        }
        entries.push(PreservedJournalEntry {
            profile_id: profile.id.clone(),
            record_sha256: buglogin_storage::blob::checksum(&record_bytes),
            root_sha256: root_checksum(&root)?,
            created,
        });
    }
    let created_count = entries.iter().filter(|entry| entry.created).count();
    let reused_count = entries.len() - created_count;
    let mut journal = PreservedJournal {
        schema: "buglogin.preserved-profile-run/1".into(),
        run_id: run_id.into(),
        package_id: package_id.into(),
        entries,
    };
    if let Some(existing) = existing_journal {
        if existing.schema != journal.schema
            || existing.run_id != journal.run_id
            || existing.package_id != journal.package_id
            || existing.entries.len() != journal.entries.len()
            || existing
                .entries
                .iter()
                .zip(&journal.entries)
                .any(|(left, right)| {
                    left.profile_id != right.profile_id
                        || left.record_sha256 != right.record_sha256
                        || left.root_sha256 != right.root_sha256
                })
        {
            return Err(invalid("migration_preserved_journal_conflict"));
        }
        journal = existing;
    }
    Ok(PreparedPreservedProfiles {
        root: authority_root,
        journal,
        markers,
        created_roots,
        created_count: if created_count > 0 && !is_rerun {
            created_count
        } else {
            0
        },
        reused_count: if is_rerun {
            created_count + reused_count
        } else {
            reused_count
        },
    })
}

impl PreparedPreservedProfiles {
    pub(crate) fn created_count(&self) -> usize {
        self.created_count
    }

    pub(crate) fn reused_count(&self) -> usize {
        self.reused_count
    }

    pub(crate) fn commit(&self) -> Result<(), Problem> {
        if self.journal.entries.is_empty() {
            return Ok(());
        }
        let path = journal_path(&self.root, &self.journal.run_id);
        let bytes = serde_json::to_vec(&self.journal)
            .map_err(|_| invalid("migration_preserved_journal_invalid"))?;
        if path.exists() {
            let existing =
                fs::read(&path).map_err(|error| io_problem("read preserved journal", &error))?;
            if existing == bytes {
                return Ok(());
            }
            return Err(invalid("migration_preserved_journal_conflict"));
        }
        ensure_directory(
            path.parent()
                .ok_or_else(|| invalid("migration_preserved_path_invalid"))?,
        )?;
        write_new(&path, &bytes)
    }

    pub(crate) fn finalize(self) -> Result<(), Problem> {
        for marker in self.markers {
            if marker.exists() {
                fs::remove_file(marker)
                    .map_err(|error| io_problem("remove preserved profile marker", &error))?;
            }
        }
        Ok(())
    }

    pub(crate) fn cleanup(self) -> Result<(), Problem> {
        for root in self.created_roots.into_iter().rev() {
            if root.exists() {
                fs::remove_dir_all(root)
                    .map_err(|error| io_problem("cleanup preserved profile", &error))?;
            }
        }
        Ok(())
    }
}

pub(crate) fn rollback_preserved_profiles(
    destination_root: &Path,
    run_id: &str,
) -> Result<PreservedRollbackReceipt, Problem> {
    let authority_root = destination_root.join(ROOT);
    let path = journal_path(&authority_root, run_id);
    if !path.exists() {
        return Ok(PreservedRollbackReceipt::default());
    }
    let journal: PreservedJournal = serde_json::from_slice(
        &fs::read(&path).map_err(|error| io_problem("read preserved journal", &error))?,
    )
    .map_err(|_| invalid("migration_preserved_journal_invalid"))?;
    if journal.schema != "buglogin.preserved-profile-run/1" || journal.run_id != run_id {
        return Err(invalid("migration_preserved_journal_invalid"));
    }
    let completed = authority_root
        .join("runs")
        .join(format!("{run_id}.rolled-back"));
    if completed.exists() {
        return Ok(PreservedRollbackReceipt::default());
    }
    let mut quarantined_ids = Vec::new();
    for entry in journal.entries.iter().filter(|entry| entry.created) {
        let source = profile_root(destination_root, &entry.profile_id);
        let target = authority_root
            .join("quarantine")
            .join(run_id)
            .join(&entry.profile_id);
        if source.exists() {
            if target.exists() {
                return Err(invalid("migration_preserved_rollback_collision"));
            }
            ensure_directory(
                target
                    .parent()
                    .ok_or_else(|| invalid("migration_preserved_path_invalid"))?,
            )?;
            fs::rename(&source, &target)
                .map_err(|error| io_problem("quarantine preserved profile", &error))?;
        } else if !target.exists() {
            return Err(invalid("migration_preserved_profile_missing"));
        }
        quarantined_ids.push(entry.profile_id.clone());
    }
    write_new(&completed, b"rolled-back")?;
    Ok(PreservedRollbackReceipt { quarantined_ids })
}

pub(crate) fn reconcile_preserved_profiles(
    destination_root: &Path,
    run_id: &str,
) -> Result<(u64, Vec<String>), Problem> {
    let path = journal_path(&destination_root.join(ROOT), run_id);
    if !path.exists() {
        return Ok((0, Vec::new()));
    }
    let journal: PreservedJournal = serde_json::from_slice(
        &fs::read(path).map_err(|error| io_problem("read preserved journal", &error))?,
    )
    .map_err(|_| invalid("migration_preserved_journal_invalid"))?;
    if journal.schema != "buglogin.preserved-profile-run/1" || journal.run_id != run_id {
        return Err(invalid("migration_preserved_journal_invalid"));
    }
    let mut mismatches = Vec::new();
    for entry in &journal.entries {
        let root = profile_root(destination_root, &entry.profile_id);
        let record = root.join("record.json");
        let matches = fs::read(&record).ok().is_some_and(|bytes| {
            buglogin_storage::blob::checksum(&bytes) == entry.record_sha256
                && root_checksum(&root).is_ok_and(|checksum| checksum == entry.root_sha256)
        });
        if !matches {
            mismatches.push(entry.profile_id.clone());
        }
    }
    Ok((journal.entries.len() as u64, mismatches))
}

fn root_checksum(root: &Path) -> Result<String, Problem> {
    let mut paths = collect_relative_files(root)?;
    paths.retain(|path| path == "record.json" || path.starts_with("profile/"));
    let mut material = Vec::new();
    for relative in paths {
        let bytes = fs::read(root.join(&relative))
            .map_err(|error| io_problem("read preserved profile checksum input", &error))?;
        material.extend_from_slice(&(relative.len() as u64).to_be_bytes());
        material.extend_from_slice(relative.as_bytes());
        material.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        material.extend_from_slice(&bytes);
    }
    Ok(buglogin_storage::blob::checksum(&material))
}

fn profile_root(destination_root: &Path, profile_id: &str) -> PathBuf {
    destination_root
        .join(ROOT)
        .join("profiles")
        .join(profile_id)
}

fn journal_path(authority_root: &Path, run_id: &str) -> PathBuf {
    authority_root.join("runs").join(format!("{run_id}.json"))
}

fn read_journal_if_exists(
    authority_root: &Path,
    run_id: &str,
) -> Result<Option<PreservedJournal>, Problem> {
    let path = journal_path(authority_root, run_id);
    if !path.exists() {
        return Ok(None);
    }
    serde_json::from_slice(
        &fs::read(path).map_err(|error| io_problem("read preserved journal", &error))?,
    )
    .map(Some)
    .map_err(|_| invalid("migration_preserved_journal_invalid"))
}

fn sorted_profiles(profiles: &[NeutralProfileRecord]) -> Vec<&NeutralProfileRecord> {
    let mut profiles = profiles.iter().collect::<Vec<_>>();
    profiles.sort_by(|left, right| left.id.cmp(&right.id));
    profiles
}

fn profile_files<'a>(files: &'a DecryptedFiles, profile_id: &str) -> &'a [(PathBuf, Vec<u8>)] {
    files.get(profile_id).map(Vec::as_slice).unwrap_or_default()
}

fn record_bytes(profile: &NeutralProfileRecord) -> Result<Vec<u8>, Problem> {
    serde_json::to_vec(&PreservedRecord {
        schema: "buglogin.preserved-profile/1".into(),
        id: profile.id.clone(),
        source_ref: profile.source_ref.clone(),
        name: profile.data.name.clone(),
        runtime: profile.data.runtime.clone(),
        migration_class: profile.data.migration_class.clone(),
        source_data_object: profile.data.source_data_object.clone(),
    })
    .map_err(|_| invalid("migration_preserved_record_invalid"))
}

fn verify_root(
    root: &Path,
    profile: &NeutralProfileRecord,
    files: &[(PathBuf, Vec<u8>)],
    allowed_marker: Option<&Path>,
) -> Result<(), Problem> {
    reject_symlink(root)?;
    let expected_record = record_bytes(profile)?;
    let actual_record = fs::read(root.join("record.json"))
        .map_err(|_| invalid("migration_preserved_profile_collision"))?;
    if actual_record != expected_record {
        return Err(invalid("migration_preserved_profile_collision"));
    }
    let mut expected_paths = BTreeSet::new();
    for (relative, expected) in files {
        let target = root.join("profile").join(relative);
        if fs::read(&target).map_err(|_| invalid("migration_preserved_profile_collision"))?
            != *expected
        {
            return Err(invalid("migration_preserved_profile_collision"));
        }
        expected_paths.insert(relative.to_string_lossy().replace('\\', "/"));
    }
    let actual_paths = collect_relative_files(&root.join("profile"))?;
    if actual_paths != expected_paths {
        return Err(invalid("migration_preserved_profile_collision"));
    }
    for entry in fs::read_dir(root).map_err(|error| io_problem("read preserved profile", &error))? {
        let entry = entry.map_err(|error| io_problem("read preserved profile", &error))?;
        let path = entry.path();
        let allowed = path == root.join("record.json")
            || path == root.join("profile")
            || allowed_marker.is_some_and(|marker| path == marker);
        if !allowed {
            return Err(invalid("migration_preserved_profile_collision"));
        }
    }
    Ok(())
}

fn collect_relative_files(root: &Path) -> Result<BTreeSet<String>, Problem> {
    if !root.exists() {
        return Ok(BTreeSet::new());
    }
    let mut files = BTreeSet::new();
    collect_files(root, root, &mut files)?;
    Ok(files)
}

fn collect_files(root: &Path, current: &Path, files: &mut BTreeSet<String>) -> Result<(), Problem> {
    for entry in fs::read_dir(current).map_err(|error| io_problem("read preserved data", &error))? {
        let entry = entry.map_err(|error| io_problem("read preserved data", &error))?;
        let file_type = entry
            .file_type()
            .map_err(|error| io_problem("read preserved data type", &error))?;
        if file_type.is_symlink() {
            return Err(invalid("migration_preserved_symlink_rejected"));
        }
        if file_type.is_dir() {
            collect_files(root, &entry.path(), files)?;
        } else if file_type.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)
                .map_err(|_| invalid("migration_preserved_path_invalid"))?
                .to_path_buf();
            if relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            {
                return Err(invalid("migration_preserved_path_invalid"));
            }
            files.insert(relative.to_string_lossy().replace('\\', "/"));
        } else {
            return Err(invalid("migration_preserved_entry_invalid"));
        }
    }
    Ok(())
}

fn write_files(root: &Path, files: &[(PathBuf, Vec<u8>)]) -> Result<(), Problem> {
    for (relative, bytes) in files {
        let target = root.join("profile").join(relative);
        ensure_directory(
            target
                .parent()
                .ok_or_else(|| invalid("migration_preserved_path_invalid"))?,
        )?;
        write_new(&target, bytes)?;
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), Problem> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_problem("create preserved migration file", &error))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| io_problem("write preserved migration file", &error))
}

fn invalid(code: &'static str) -> Problem {
    Problem::new(
        code,
        "Preserved migration profile is invalid",
        409,
        "migration",
    )
}
