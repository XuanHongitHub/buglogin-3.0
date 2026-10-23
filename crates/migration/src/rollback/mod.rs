use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_domain::entities::{Extension, ExtensionGroup, Group, Profile, Proxy, Vpn};
use buglogin_domain::errors::Problem;
use buglogin_storage::local::LocalRepository;
use buglogin_storage::{
    secrets::{OsVault, SecretError, SecretKey, SecretVersion},
    vault::NativeVault,
};

use crate::activate::{
    ActivationJournal, ActivationStatus, EntityActivationJournalEntry,
    VpnConfigActivationJournalEntry, journal_path,
};

use crate::cutover::{CutoverJournal, IMPORTS_DIRECTORY, JOURNAL_CHECKSUM_FILE, JOURNAL_FILE};
use crate::preserved_profile_activation::rollback_preserved_profiles;
use crate::transform::{io_problem, problem, reject_symlink, validate_run_id};
use crate::verify::validate_relative_path;
use serde::{Deserialize, Serialize};

const ROLLBACK_CONFIRMATION: &str = "ROLLBACK_MIGRATION_RUN";
static JOURNAL_REPLACEMENT_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
pub struct RollbackConfirmation(());

pub fn confirm_rollback(value: &str) -> Result<RollbackConfirmation, Problem> {
    if value == ROLLBACK_CONFIRMATION {
        Ok(RollbackConfirmation(()))
    } else {
        Err(problem(
            "migration_rollback_not_confirmed",
            "Migration rollback is not confirmed",
            409,
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RollbackReceipt {
    pub run_id: String,
    pub removed_artifacts: usize,
    pub retained_unowned_artifacts: Vec<PathBuf>,
}

pub fn rollback_cutover(
    destination_root: &Path,
    run_id: &str,
    confirmation: RollbackConfirmation,
) -> Result<RollbackReceipt, Problem> {
    let _ = confirmation;
    validate_run_id(run_id)?;
    reject_symlink(destination_root)?;
    let imports_root = destination_root.join(IMPORTS_DIRECTORY);
    reject_symlink(&imports_root)?;
    let run_root = imports_root.join(run_id);
    reject_symlink(&run_root)?;

    let journal_bytes = fs::read(run_root.join(JOURNAL_FILE))
        .map_err(|error| io_problem("read cutover journal", &error))?;
    let journal_checksum = fs::read_to_string(run_root.join(JOURNAL_CHECKSUM_FILE))
        .map_err(|error| io_problem("read cutover journal checksum", &error))?;
    if buglogin_storage::blob::checksum(&journal_bytes) != journal_checksum {
        return Err(problem(
            "migration_cutover_journal_checksum_mismatch",
            "Migration cutover journal checksum mismatch",
            409,
        ));
    }
    let journal: CutoverJournal = serde_json::from_slice(&journal_bytes).map_err(|_| {
        problem(
            "migration_cutover_journal_invalid",
            "Migration cutover journal is invalid",
            409,
        )
    })?;
    if journal.schema != "buglogin.cutover/1" || journal.run_id != run_id {
        return Err(problem(
            "migration_cutover_journal_invalid",
            "Migration cutover journal is invalid",
            409,
        ));
    }

    validate_rollback_artifacts(&run_root, &journal)?;

    for artifact in journal.artifacts.iter().rev() {
        fs::remove_file(run_root.join(&artifact.path))
            .map_err(|error| io_problem("remove rollback artifact", &error))?;
        remove_empty_parents(run_root.join(&artifact.path).parent(), &run_root)?;
    }
    fs::remove_file(run_root.join(JOURNAL_CHECKSUM_FILE))
        .map_err(|error| io_problem("remove rollback journal checksum", &error))?;
    fs::remove_file(run_root.join(JOURNAL_FILE))
        .map_err(|error| io_problem("remove rollback journal", &error))?;

    let retained_unowned_artifacts = collect_remaining_files(&run_root)?;
    if retained_unowned_artifacts.is_empty() {
        fs::remove_dir(&run_root)
            .map_err(|error| io_problem("remove rollback run directory", &error))?;
        if journal.imports_directory_created {
            let _ = fs::remove_dir(&imports_root);
        }
        if journal.destination_created {
            let _ = fs::remove_dir(destination_root);
        }
    }

    Ok(RollbackReceipt {
        run_id: run_id.to_owned(),
        removed_artifacts: journal.artifacts.len() + 2,
        retained_unowned_artifacts,
    })
}

fn validate_rollback_artifacts(run_root: &Path, journal: &CutoverJournal) -> Result<(), Problem> {
    let mut paths = BTreeSet::new();
    for artifact in &journal.artifacts {
        validate_relative_path(&artifact.path)?;
        if matches!(artifact.path.as_str(), JOURNAL_FILE | JOURNAL_CHECKSUM_FILE)
            || !paths.insert(artifact.path.as_str())
        {
            return Err(problem(
                "migration_cutover_journal_invalid",
                "Migration cutover journal contains duplicate or reserved artifacts",
                409,
            ));
        }
        let path = run_root.join(&artifact.path);
        reject_symlink(&path)?;
        let metadata =
            fs::metadata(&path).map_err(|error| io_problem("inspect rollback artifact", &error))?;
        if !metadata.is_file() {
            return Err(problem(
                "migration_rollback_artifact_invalid",
                "Migration rollback artifact is not a regular file",
                409,
            ));
        }
        let bytes =
            fs::read(&path).map_err(|error| io_problem("read rollback artifact", &error))?;
        if bytes.len() as u64 != artifact.bytes
            || buglogin_storage::blob::checksum(&bytes) != artifact.sha256
        {
            return Err(problem(
                "migration_rollback_artifact_changed",
                "Migration rollback artifact changed after cutover",
                409,
            ));
        }
    }
    Ok(())
}

fn collect_remaining_files(root: &Path) -> Result<Vec<PathBuf>, Problem> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_files(root: &Path, current: &Path, files: &mut Vec<PathBuf>) -> Result<(), Problem> {
    for entry in
        fs::read_dir(current).map_err(|error| io_problem("inspect rollback directory", &error))?
    {
        let entry = entry.map_err(|error| io_problem("inspect rollback entry", &error))?;
        let file_type = entry
            .file_type()
            .map_err(|error| io_problem("inspect rollback entry type", &error))?;
        if file_type.is_symlink() {
            files.push(entry.path().strip_prefix(root).unwrap().to_path_buf());
        } else if file_type.is_dir() {
            collect_files(root, &entry.path(), files)?;
        } else {
            files.push(entry.path().strip_prefix(root).unwrap().to_path_buf());
        }
    }
    Ok(())
}

fn remove_empty_parents(mut parent: Option<&Path>, boundary: &Path) -> Result<(), Problem> {
    while let Some(path) = parent {
        if path == boundary {
            break;
        }
        match fs::remove_dir(path) {
            Ok(()) => parent = path.parent(),
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => break,
            Err(error) => return Err(io_problem("remove empty rollback directory", &error)),
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationRollbackReceipt {
    pub run_id: String,
    pub removed_credentials: Vec<String>,
    pub removed_vpn_config_ids: Vec<String>,
    pub removed_extension_artifacts: Vec<PathBuf>,
    pub quarantined_extensions: Vec<PathBuf>,
    pub quarantined_extension_groups: Vec<PathBuf>,
    pub quarantined_vpns: Vec<PathBuf>,
    pub quarantined_groups: Vec<PathBuf>,
    pub quarantined_proxies: Vec<PathBuf>,
    pub quarantined_profiles: Vec<PathBuf>,
    pub quarantined_preserved_profile_ids: Vec<String>,
}

pub fn rollback_activation(
    destination_root: &Path,
    run_id: &str,
    confirmation: RollbackConfirmation,
) -> Result<ActivationRollbackReceipt, Problem> {
    rollback_activation_with_vault(
        destination_root,
        run_id,
        confirmation,
        &mut NativeVault::new(),
    )
}

pub fn rollback_activation_with_vault<V: OsVault>(
    destination_root: &Path,
    run_id: &str,
    confirmation: RollbackConfirmation,
    vault: &mut V,
) -> Result<ActivationRollbackReceipt, Problem> {
    let _ = confirmation;
    validate_run_id(run_id)?;
    reject_symlink(destination_root)?;
    let journal = read_activation_journal_v3(destination_root, run_id)?;
    if journal.journal.status == ActivationStatus::RolledBack {
        let preserved = rollback_preserved_profiles(destination_root, run_id)?;
        return Ok(ActivationRollbackReceipt {
            run_id: run_id.to_owned(),
            removed_credentials: Vec::new(),
            removed_vpn_config_ids: Vec::new(),
            removed_extension_artifacts: Vec::new(),
            quarantined_extensions: Vec::new(),
            quarantined_extension_groups: Vec::new(),
            quarantined_vpns: Vec::new(),
            quarantined_groups: Vec::new(),
            quarantined_proxies: Vec::new(),
            quarantined_profiles: Vec::new(),
            quarantined_preserved_profile_ids: preserved.quarantined_ids,
        });
    }
    let repository = LocalRepository::open(destination_root)?;
    let mut journal = journal.clone();
    for entry in journal.journal.credentials.iter().filter(|entry| {
        entry.created && !rollback_done(&journal, &rollback_key("credential", &entry.credential_id))
    }) {
        let key = proxy_credential_key(&entry.credential_id)?;
        let (value, version) = match vault.read(&key) {
            Ok(value) => value,
            Err(SecretError::NotFound)
                if rollback_started(
                    &journal,
                    &rollback_key("credential", &entry.credential_id),
                ) =>
            {
                continue;
            }
            Err(_) => return Err(activation_changed("credentialId", &entry.credential_id)),
        };
        if version.generation() != entry.version
            || buglogin_storage::blob::checksum(value.as_bytes()) != entry.sha256
        {
            return Err(activation_changed("credentialId", &entry.credential_id));
        }
    }
    for entry in &journal.journal.extension_artifacts {
        if rollback_done(&journal, &rollback_key("artifact", &entry.path)) {
            continue;
        }
        verify_activation_artifact(
            destination_root,
            &entry.path,
            &entry.sha256,
            &entry.artifact_ref,
            rollback_started(&journal, &rollback_key("artifact", &entry.path)),
        )?;
    }
    for entry in journal.journal.extensions.iter().filter(|entry| {
        entry.created && !rollback_done(&journal, &rollback_key("extension", &entry.entity_id))
    }) {
        verify_activation_entity::<Extension>(
            &repository,
            &entry.entity_id,
            entry.revision,
            &entry.sha256,
            "extensionId",
            rollback_started(&journal, &rollback_key("extension", &entry.entity_id)),
        )?;
    }
    for entry in journal.journal.extension_groups.iter().filter(|entry| {
        entry.created && !rollback_done(&journal, &rollback_key("extensionGroup", &entry.entity_id))
    }) {
        verify_activation_entity::<ExtensionGroup>(
            &repository,
            &entry.entity_id,
            entry.revision,
            &entry.sha256,
            "extensionGroupId",
            rollback_started(&journal, &rollback_key("extensionGroup", &entry.entity_id)),
        )?;
    }
    for entry in journal.vpns.iter().filter(|entry| {
        entry.created && !rollback_done(&journal, &rollback_key("vpn", &entry.entity_id))
    }) {
        verify_activation_entity::<Vpn>(
            &repository,
            &entry.entity_id,
            entry.revision,
            &entry.sha256,
            "vpnId",
            rollback_started(&journal, &rollback_key("vpn", &entry.entity_id)),
        )?;
    }
    for entry in journal.vpn_configs.iter().filter(|entry| {
        entry.created && !rollback_done(&journal, &rollback_key("vpnConfig", &entry.config_ref))
    }) {
        let key = vpn_config_key(&entry.config_ref)?;
        let (value, version) = match vault.read(&key) {
            Ok(value) => value,
            Err(SecretError::NotFound)
                if rollback_started(&journal, &rollback_key("vpnConfig", &entry.config_ref)) =>
            {
                continue;
            }
            Err(_) => return Err(activation_changed("configRef", &entry.config_ref)),
        };
        if version.generation() != entry.version
            || buglogin_storage::blob::checksum(value.as_bytes()) != entry.sha256
        {
            return Err(activation_changed("configRef", &entry.config_ref));
        }
    }
    for entry in journal.journal.groups.iter().filter(|entry| {
        entry.created && !rollback_done(&journal, &rollback_key("group", &entry.entity_id))
    }) {
        verify_activation_entity::<Group>(
            &repository,
            &entry.entity_id,
            entry.revision,
            &entry.sha256,
            "groupId",
            rollback_started(&journal, &rollback_key("group", &entry.entity_id)),
        )?;
    }
    for entry in journal.journal.proxies.iter().filter(|entry| {
        entry.created && !rollback_done(&journal, &rollback_key("proxy", &entry.entity_id))
    }) {
        verify_activation_entity::<Proxy>(
            &repository,
            &entry.entity_id,
            entry.revision,
            &entry.sha256,
            "proxyId",
            rollback_started(&journal, &rollback_key("proxy", &entry.entity_id)),
        )?;
    }
    for entry in journal.journal.profiles.iter().filter(|entry| {
        entry.created && !rollback_done(&journal, &rollback_key("profile", &entry.profile_id))
    }) {
        verify_activation_entity::<Profile>(
            &repository,
            &entry.profile_id,
            entry.revision,
            &entry.sha256,
            "profileId",
            rollback_started(&journal, &rollback_key("profile", &entry.profile_id)),
        )?;
    }
    let profiles = journal
        .journal
        .profiles
        .iter()
        .rev()
        .filter(|entry| {
            entry.created && !rollback_done(&journal, &rollback_key("profile", &entry.profile_id))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut quarantined_profiles = Vec::new();
    for entry in profiles {
        let key = rollback_key("profile", &entry.profile_id);
        if !rollback_started(&journal, &key) {
            start_rollback(&mut journal, key.clone(), destination_root)?;
        }
        if repository.get::<Profile>(&entry.profile_id)?.is_some() {
            quarantined_profiles
                .push(repository.quarantine::<Profile>(entry.revision, &entry.profile_id)?);
        }
        checkpoint_rollback(
            &mut journal,
            rollback_key("profile", &entry.profile_id),
            destination_root,
        )?;
    }
    let mut quarantined_vpns = Vec::new();
    for entry in journal
        .vpns
        .iter()
        .rev()
        .filter(|entry| {
            entry.created && !rollback_done(&journal, &rollback_key("vpn", &entry.entity_id))
        })
        .cloned()
        .collect::<Vec<_>>()
    {
        if repository.get::<Vpn>(&entry.entity_id)?.is_some() {
            quarantined_vpns.push(repository.quarantine::<Vpn>(entry.revision, &entry.entity_id)?);
        }
        checkpoint_rollback(
            &mut journal,
            rollback_key("vpn", &entry.entity_id),
            destination_root,
        )?;
    }
    let mut removed_vpn_config_ids = Vec::new();
    for entry in journal
        .vpn_configs
        .iter()
        .rev()
        .filter(|entry| {
            entry.created && !rollback_done(&journal, &rollback_key("vpnConfig", &entry.config_ref))
        })
        .cloned()
        .collect::<Vec<_>>()
    {
        vault
            .remove(
                &vpn_config_key(&entry.config_ref)?,
                SecretVersion::new(entry.version)
                    .ok_or_else(|| activation_changed("configRef", &entry.config_ref))?,
            )
            .or_else(|error| {
                if error == SecretError::NotFound {
                    Ok(())
                } else {
                    Err(error)
                }
            })
            .map_err(|error| vault_problem(error, "remove VPN config"))?;
        removed_vpn_config_ids.push(entry.config_ref.clone());
        checkpoint_rollback(
            &mut journal,
            rollback_key("vpnConfig", &entry.config_ref),
            destination_root,
        )?;
    }
    let mut quarantined_extension_groups = Vec::new();
    for entry in journal
        .journal
        .extension_groups
        .iter()
        .rev()
        .filter(|entry| {
            entry.created
                && !rollback_done(&journal, &rollback_key("extensionGroup", &entry.entity_id))
        })
        .cloned()
        .collect::<Vec<_>>()
    {
        if repository
            .get::<ExtensionGroup>(&entry.entity_id)?
            .is_some()
        {
            quarantined_extension_groups
                .push(repository.quarantine::<ExtensionGroup>(entry.revision, &entry.entity_id)?);
        }
        checkpoint_rollback(
            &mut journal,
            rollback_key("extensionGroup", &entry.entity_id),
            destination_root,
        )?;
    }
    let mut quarantined_extensions = Vec::new();
    for entry in journal
        .journal
        .extensions
        .iter()
        .rev()
        .filter(|entry| {
            entry.created && !rollback_done(&journal, &rollback_key("extension", &entry.entity_id))
        })
        .cloned()
        .collect::<Vec<_>>()
    {
        if repository.get::<Extension>(&entry.entity_id)?.is_some() {
            quarantined_extensions
                .push(repository.quarantine::<Extension>(entry.revision, &entry.entity_id)?);
        }
        checkpoint_rollback(
            &mut journal,
            rollback_key("extension", &entry.entity_id),
            destination_root,
        )?;
    }
    let mut removed_extension_artifacts = Vec::new();
    for entry in journal
        .journal
        .extension_artifacts
        .iter()
        .rev()
        .filter(|entry| {
            entry.created && !rollback_done(&journal, &rollback_key("artifact", &entry.path))
        })
        .cloned()
        .collect::<Vec<_>>()
    {
        let relative = PathBuf::from(&entry.path);
        match fs::remove_file(destination_root.join(&relative)) {
            Ok(()) => {
                remove_empty_parents(destination_root.join(&relative).parent(), destination_root)?;
                removed_extension_artifacts.push(relative);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_problem("remove extension artifact", &error)),
        }
        checkpoint_rollback(
            &mut journal,
            rollback_key("artifact", &entry.path),
            destination_root,
        )?;
    }
    let mut quarantined_proxies = Vec::new();
    for entry in journal
        .journal
        .proxies
        .iter()
        .rev()
        .filter(|entry| {
            entry.created && !rollback_done(&journal, &rollback_key("proxy", &entry.entity_id))
        })
        .cloned()
        .collect::<Vec<_>>()
    {
        if repository.get::<Proxy>(&entry.entity_id)?.is_some() {
            quarantined_proxies
                .push(repository.quarantine::<Proxy>(entry.revision, &entry.entity_id)?);
        }
        checkpoint_rollback(
            &mut journal,
            rollback_key("proxy", &entry.entity_id),
            destination_root,
        )?;
    }
    let mut quarantined_groups = Vec::new();
    for entry in journal
        .journal
        .groups
        .iter()
        .rev()
        .filter(|entry| {
            entry.created && !rollback_done(&journal, &rollback_key("group", &entry.entity_id))
        })
        .cloned()
        .collect::<Vec<_>>()
    {
        if repository.get::<Group>(&entry.entity_id)?.is_some() {
            quarantined_groups
                .push(repository.quarantine::<Group>(entry.revision, &entry.entity_id)?);
        }
        checkpoint_rollback(
            &mut journal,
            rollback_key("group", &entry.entity_id),
            destination_root,
        )?;
    }
    let mut removed_credentials = Vec::new();
    for entry in journal
        .journal
        .credentials
        .iter()
        .rev()
        .filter(|entry| {
            entry.created
                && !rollback_done(&journal, &rollback_key("credential", &entry.credential_id))
        })
        .cloned()
        .collect::<Vec<_>>()
    {
        vault
            .remove(
                &proxy_credential_key(&entry.credential_id)?,
                SecretVersion::new(entry.version)
                    .ok_or_else(|| activation_changed("credentialId", &entry.credential_id))?,
            )
            .or_else(|error| {
                if error == SecretError::NotFound {
                    Ok(())
                } else {
                    Err(error)
                }
            })
            .map_err(|error| vault_problem(error, "remove proxy credential"))?;
        removed_credentials.push(entry.credential_id.clone());
        checkpoint_rollback(
            &mut journal,
            rollback_key("credential", &entry.credential_id),
            destination_root,
        )?;
    }
    let preserved = rollback_preserved_profiles(destination_root, run_id)?;
    journal.journal.status = ActivationStatus::RolledBack;
    replace_activation_journal_v3(destination_root, &journal)?;
    Ok(ActivationRollbackReceipt {
        run_id: run_id.to_owned(),
        removed_credentials,
        removed_vpn_config_ids,
        removed_extension_artifacts,
        quarantined_extensions,
        quarantined_extension_groups,
        quarantined_vpns,
        quarantined_groups,
        quarantined_proxies,
        quarantined_profiles,
        quarantined_preserved_profile_ids: preserved.quarantined_ids,
    })
}

fn verify_activation_artifact(
    destination_root: &Path,
    relative: &str,
    sha256: &str,
    artifact_ref: &str,
    allow_missing: bool,
) -> Result<(), Problem> {
    validate_relative_path(relative)?;
    match buglogin_storage::blob::read_verified(destination_root, Path::new(relative), sha256) {
        Ok(_) => Ok(()),
        Err(error) if allow_missing && error.status == 404 => Ok(()),
        Err(_) => Err(activation_changed("artifactRef", artifact_ref)),
    }
}

fn proxy_credential_key(id: &str) -> Result<SecretKey, Problem> {
    SecretKey::new("proxy-credentials", id)
        .map_err(|error| vault_problem(error, "build proxy credential key"))
}

fn vpn_config_key(id: &str) -> Result<SecretKey, Problem> {
    SecretKey::new("vpn-configs", id).map_err(|error| vault_problem(error, "build VPN config key"))
}

fn vault_problem(error: SecretError, operation: &'static str) -> Problem {
    Problem::new(
        error.code(),
        "Migration secret vault operation failed",
        if matches!(error, SecretError::NotFound | SecretError::VersionConflict) {
            409
        } else {
            503
        },
        "migration",
    )
    .with_field("operation", operation)
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivationJournalV3 {
    #[serde(flatten)]
    journal: ActivationJournal,
    #[serde(default)]
    vpn_configs: Vec<VpnConfigActivationJournalEntry>,
    #[serde(default)]
    vpns: Vec<EntityActivationJournalEntry>,
    #[serde(default, rename = "rollbackProgress")]
    rollback_progress: RollbackProgress,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct RollbackProgress {
    #[serde(default)]
    started: BTreeSet<String>,
    #[serde(default)]
    completed: BTreeSet<String>,
}

fn rollback_key(kind: &str, id: &str) -> String {
    format!("{kind}:{id}")
}

fn rollback_started(journal: &ActivationJournalV3, key: &str) -> bool {
    journal.rollback_progress.started.contains(key)
}

fn rollback_done(journal: &ActivationJournalV3, key: &str) -> bool {
    journal.rollback_progress.completed.contains(key)
}

fn start_rollback(
    journal: &mut ActivationJournalV3,
    key: impl Into<String>,
    root: &Path,
) -> Result<(), Problem> {
    journal.rollback_progress.started.insert(key.into());
    replace_activation_journal_v3(root, journal)
}

fn checkpoint_rollback(
    journal: &mut ActivationJournalV3,
    key: impl Into<String>,
    root: &Path,
) -> Result<(), Problem> {
    journal.rollback_progress.completed.insert(key.into());
    replace_activation_journal_v3(root, journal)
}

fn read_activation_journal_v3(root: &Path, run_id: &str) -> Result<ActivationJournalV3, Problem> {
    let bytes = fs::read(journal_path(root, run_id))
        .map_err(|error| io_problem("read activation journal", &error))?;
    let journal: ActivationJournalV3 = serde_json::from_slice(&bytes).map_err(|_| {
        problem(
            "migration_activation_journal_invalid",
            "Migration activation journal is invalid",
            409,
        )
    })?;
    if journal.journal.schema != "buglogin.activation/1" || journal.journal.run_id != run_id {
        return Err(problem(
            "migration_activation_journal_invalid",
            "Migration activation journal is invalid",
            409,
        ));
    }
    Ok(journal)
}

fn replace_activation_journal_v3(
    root: &Path,
    journal: &ActivationJournalV3,
) -> Result<(), Problem> {
    let path = journal_path(root, &journal.journal.run_id);
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        JOURNAL_REPLACEMENT_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let bytes = serde_json::to_vec_pretty(journal).map_err(|_| {
        problem(
            "migration_activation_journal_invalid",
            "Migration activation journal is invalid",
            409,
        )
    })?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(|error| io_problem("create activation journal", &error))?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| io_problem("write activation journal", &error))?;
    match fs::rename(&temporary, &path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => fs::remove_file(&path)
            .and_then(|_| fs::rename(&temporary, &path))
            .map_err(|error| io_problem("replace activation journal", &error)),
        Err(error) => Err(io_problem("replace activation journal", &error)),
    }
}

fn verify_activation_entity<T: buglogin_storage::local::LocalEntity>(
    repository: &LocalRepository,
    id: &str,
    revision: u64,
    sha256: &str,
    field: &str,
    allow_missing: bool,
) -> Result<(), Problem> {
    let Some(current) = repository.get::<T>(id)? else {
        return if allow_missing {
            Ok(())
        } else {
            Err(activation_changed(field, id))
        };
    };
    let checksum = serde_json::to_vec(&current.value)
        .map(|bytes| buglogin_storage::blob::checksum(&bytes))
        .map_err(|_| activation_changed(field, id))?;
    if current.revision != revision || checksum != sha256 {
        return Err(activation_changed(field, id));
    }
    Ok(())
}

fn activation_changed(field: &str, id: &str) -> Problem {
    problem(
        "migration_activation_entity_changed",
        "Migration activation entity changed",
        409,
    )
    .with_field(field, id)
}
