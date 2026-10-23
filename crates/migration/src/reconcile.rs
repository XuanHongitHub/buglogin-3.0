use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use buglogin_domain::{
    entities::{Extension, ExtensionGroup, Group, Profile, Proxy, Vpn},
    errors::Problem,
};
use buglogin_storage::local::LocalRepository;
use buglogin_storage::{
    secrets::{OsVault, SecretKey},
    vault::NativeVault,
};
use serde::{Deserialize, Serialize};

use crate::activate::{
    ActivationJournal, ActivationStatus, EntityActivationJournalEntry,
    VpnConfigActivationJournalEntry, journal_path,
};
use crate::encrypted_vpn_configs::VpnConfigSecret;
use crate::inspect::{InspectReport, MigrationClass};
use crate::preserved_profile_activation::reconcile_preserved_profiles;
use crate::transform::{io_problem, problem};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    Passed,
    Blocked,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReconciliationReport {
    pub schema: String,
    pub run_id: String,
    pub status: ReconciliationStatus,
    pub activated_credentials: u64,
    pub mismatched_credential_ids: Vec<String>,
    pub expected_extension_artifacts: u64,
    pub activated_extension_artifacts: u64,
    pub mismatched_extension_artifact_ids: Vec<String>,
    pub expected_extensions: u64,
    pub activated_extensions: u64,
    pub mismatched_extension_ids: Vec<String>,
    pub expected_extension_groups: u64,
    pub activated_extension_groups: u64,
    pub mismatched_extension_group_ids: Vec<String>,
    pub expected_vpn_configs: u64,
    pub activated_vpn_configs: u64,
    pub mismatched_vpn_config_ids: Vec<String>,
    pub expected_vpns: u64,
    pub activated_vpns: u64,
    pub mismatched_vpn_ids: Vec<String>,
    pub profile_vpn_reference_mismatches: Vec<String>,
    pub expected_groups: u64,
    pub activated_groups: u64,
    pub mismatched_group_ids: Vec<String>,
    pub expected_proxies: u64,
    pub activated_proxies: u64,
    pub mismatched_proxy_ids: Vec<String>,
    pub expected_profiles: u64,
    pub activated_profiles: u64,
    pub mismatched_profile_ids: Vec<String>,
    pub expected_preserved_profiles: u64,
    pub preserved_profiles: u64,
    pub mismatched_preserved_profile_ids: Vec<String>,
    pub unsupported_entity_counts: BTreeMap<String, u64>,
    pub invalid_source_records: u64,
    pub dangling_source_references: u64,
}

pub fn reconcile_activation(
    source_report: &InspectReport,
    destination_root: &Path,
    run_id: &str,
) -> Result<ReconciliationReport, Problem> {
    reconcile_activation_with_vault(source_report, destination_root, run_id, &NativeVault::new())
}

pub fn reconcile_activation_with_vault<V: OsVault>(
    source_report: &InspectReport,
    destination_root: &Path,
    run_id: &str,
    vault: &V,
) -> Result<ReconciliationReport, Problem> {
    let journal = read_activation_journal_v3(destination_root, run_id)?;
    let repository = LocalRepository::open(destination_root)?;
    let total_profiles = source_report
        .entity_counts
        .get("profiles")
        .copied()
        .unwrap_or(0);
    let expected_preserved_profiles = source_report
        .runtime_findings
        .iter()
        .filter(|finding| {
            matches!(
                finding.class,
                MigrationClass::Partial | MigrationClass::Unsupported | MigrationClass::Unknown
            )
        })
        .map(|finding| finding.records)
        .sum::<u64>();
    let expected_profiles = total_profiles.saturating_sub(expected_preserved_profiles);
    let (preserved_profiles, mismatched_preserved_profile_ids) =
        reconcile_preserved_profiles(destination_root, run_id)?;
    let expected_extensions = source_report
        .entity_counts
        .get("extensions")
        .copied()
        .unwrap_or(0);
    let expected_extension_groups = source_report
        .entity_counts
        .get("extensionGroups")
        .copied()
        .unwrap_or(0);
    let expected_extension_artifacts = expected_extensions;
    let expected_vpns = source_report
        .entity_counts
        .get("vpns")
        .copied()
        .unwrap_or(0);
    let expected_vpn_configs = expected_vpns;
    let expected_groups = source_report
        .entity_counts
        .get("groups")
        .copied()
        .unwrap_or(0);
    let expected_proxies = source_report
        .entity_counts
        .get("proxies")
        .copied()
        .unwrap_or(0);
    let unsupported_entity_counts = source_report
        .entity_counts
        .iter()
        .filter(|(entity, count)| {
            !matches!(
                entity.as_str(),
                "profiles"
                    | "profilesDeleted"
                    | "groups"
                    | "proxies"
                    | "extensions"
                    | "extensionGroups"
                    | "vpns"
            ) && **count > 0
        })
        .map(|(entity, count)| (entity.clone(), *count))
        .collect::<BTreeMap<_, _>>();
    let mut mismatched_credential_ids = Vec::new();
    for entry in &journal.journal.credentials {
        let matches = SecretKey::new("proxy-credentials", &entry.credential_id)
            .ok()
            .and_then(|key| vault.read(&key).ok())
            .is_some_and(|(value, version)| {
                version.generation() == entry.version
                    && buglogin_storage::blob::checksum(value.as_bytes()) == entry.sha256
            });
        if !matches {
            mismatched_credential_ids.push(entry.credential_id.clone());
        }
    }
    let mut mismatched_extension_artifact_ids = Vec::new();
    for entry in &journal.journal.extension_artifacts {
        if buglogin_storage::blob::read_verified(
            destination_root,
            Path::new(&entry.path),
            &entry.sha256,
        )
        .is_err()
        {
            mismatched_extension_artifact_ids.push(entry.artifact_ref.clone());
        }
    }
    let mut mismatched_extension_ids = Vec::new();
    for entry in &journal.journal.extensions {
        if !matches_entity::<Extension>(
            &repository,
            &entry.entity_id,
            entry.revision,
            &entry.sha256,
        )? {
            mismatched_extension_ids.push(entry.entity_id.clone());
        }
    }
    let mut mismatched_extension_group_ids = Vec::new();
    for entry in &journal.journal.extension_groups {
        if !matches_entity::<ExtensionGroup>(
            &repository,
            &entry.entity_id,
            entry.revision,
            &entry.sha256,
        )? {
            mismatched_extension_group_ids.push(entry.entity_id.clone());
        }
    }
    let mut mismatched_vpn_config_ids = Vec::new();
    let mut vpn_config_bindings = BTreeMap::new();
    for entry in &journal.vpn_configs {
        let secret = SecretKey::new("vpn-configs", &entry.config_ref)
            .ok()
            .and_then(|key| vault.read(&key).ok())
            .and_then(|(value, version)| {
                if version.generation() != entry.version
                    || buglogin_storage::blob::checksum(value.as_bytes()) != entry.sha256
                {
                    return None;
                }
                serde_json::from_slice::<VpnConfigSecret>(value.as_bytes()).ok()
            });
        let matches = secret.as_ref().is_some_and(|secret| {
            secret.config_ref == entry.config_ref
                && buglogin_storage::blob::checksum(secret.config_data.as_bytes())
                    == secret.config_sha256
                && repository
                    .get::<Vpn>(&secret.vpn_id)
                    .ok()
                    .flatten()
                    .is_some_and(|vpn| {
                        vpn.value.config_ref.as_str() == secret.config_ref
                            && vpn.value.kind == secret.kind
                    })
                && journal
                    .vpns
                    .iter()
                    .any(|vpn| vpn.entity_id == secret.vpn_id)
        });
        if matches {
            let secret = secret.expect("checked above");
            vpn_config_bindings.insert(secret.config_ref, secret.vpn_id);
        } else {
            mismatched_vpn_config_ids.push(entry.config_ref.clone());
        }
    }
    let mut mismatched_vpn_ids = Vec::new();
    for entry in &journal.vpns {
        let matches =
            matches_entity::<Vpn>(&repository, &entry.entity_id, entry.revision, &entry.sha256)?
                && repository.get::<Vpn>(&entry.entity_id)?.is_some_and(|vpn| {
                    vpn_config_bindings
                        .get(vpn.value.config_ref.as_str())
                        .is_some_and(|vpn_id| vpn_id == &entry.entity_id)
                });
        if !matches {
            mismatched_vpn_ids.push(entry.entity_id.clone());
        }
    }
    let mut mismatched_group_ids = Vec::new();
    for entry in &journal.journal.groups {
        if !matches_entity::<Group>(&repository, &entry.entity_id, entry.revision, &entry.sha256)? {
            mismatched_group_ids.push(entry.entity_id.clone());
        }
    }
    let mut mismatched_proxy_ids = Vec::new();
    for entry in &journal.journal.proxies {
        if !matches_entity::<Proxy>(&repository, &entry.entity_id, entry.revision, &entry.sha256)? {
            mismatched_proxy_ids.push(entry.entity_id.clone());
        }
    }
    let mut mismatched_profile_ids = Vec::new();
    let mut profile_vpn_reference_mismatches = Vec::new();
    for entry in &journal.journal.profiles {
        if !matches_entity::<Profile>(
            &repository,
            &entry.profile_id,
            entry.revision,
            &entry.sha256,
        )? {
            mismatched_profile_ids.push(entry.profile_id.clone());
        }
        let reference_matches =
            repository
                .get::<Profile>(&entry.profile_id)?
                .is_some_and(|profile| {
                    if profile.value.proxy_id.is_some() && profile.value.vpn_id.is_some() {
                        return false;
                    }
                    profile.value.vpn_id.as_ref().is_none_or(|vpn_id| {
                        repository
                            .get::<Vpn>(vpn_id.as_str())
                            .ok()
                            .flatten()
                            .is_some()
                    })
                });
        if !reference_matches {
            profile_vpn_reference_mismatches.push(entry.profile_id.clone());
        }
    }
    let invalid_source_records = source_report.invalid_records.len() as u64;
    let dangling_source_references = source_report.dangling_references.len() as u64;
    let activated_groups = journal.journal.groups.len() as u64;
    let activated_proxies = journal.journal.proxies.len() as u64;
    let activated_profiles = journal.journal.profiles.len() as u64;
    let activated_credentials = journal.journal.credentials.len() as u64;
    let activated_extension_artifacts = journal.journal.extension_artifacts.len() as u64;
    let activated_extensions = journal.journal.extensions.len() as u64;
    let activated_extension_groups = journal.journal.extension_groups.len() as u64;
    let activated_vpn_configs = journal.vpn_configs.len() as u64;
    let activated_vpns = journal.vpns.len() as u64;
    let status = if journal.journal.status == ActivationStatus::Active
        && expected_extension_artifacts == activated_extension_artifacts
        && expected_extensions == activated_extensions
        && expected_extension_groups == activated_extension_groups
        && expected_vpn_configs == activated_vpn_configs
        && expected_vpns == activated_vpns
        && expected_groups == activated_groups
        && expected_proxies == activated_proxies
        && expected_profiles == activated_profiles
        && expected_preserved_profiles == preserved_profiles
        && mismatched_credential_ids.is_empty()
        && mismatched_extension_artifact_ids.is_empty()
        && mismatched_extension_ids.is_empty()
        && mismatched_extension_group_ids.is_empty()
        && mismatched_vpn_config_ids.is_empty()
        && mismatched_vpn_ids.is_empty()
        && profile_vpn_reference_mismatches.is_empty()
        && mismatched_group_ids.is_empty()
        && mismatched_proxy_ids.is_empty()
        && mismatched_profile_ids.is_empty()
        && mismatched_preserved_profile_ids.is_empty()
        && unsupported_entity_counts.is_empty()
        && invalid_source_records == 0
        && dangling_source_references == 0
    {
        ReconciliationStatus::Passed
    } else {
        ReconciliationStatus::Blocked
    };
    Ok(ReconciliationReport {
        schema: "buglogin.reconciliation/1".into(),
        run_id: run_id.into(),
        status,
        activated_credentials,
        mismatched_credential_ids,
        expected_extension_artifacts,
        activated_extension_artifacts,
        mismatched_extension_artifact_ids,
        expected_extensions,
        activated_extensions,
        mismatched_extension_ids,
        expected_extension_groups,
        activated_extension_groups,
        mismatched_extension_group_ids,
        expected_vpn_configs,
        activated_vpn_configs,
        mismatched_vpn_config_ids,
        expected_vpns,
        activated_vpns,
        mismatched_vpn_ids,
        profile_vpn_reference_mismatches,
        expected_groups,
        activated_groups,
        mismatched_group_ids,
        expected_proxies,
        activated_proxies,
        mismatched_proxy_ids,
        expected_profiles,
        activated_profiles,
        mismatched_profile_ids,
        expected_preserved_profiles,
        preserved_profiles,
        mismatched_preserved_profile_ids,
        unsupported_entity_counts,
        invalid_source_records,
        dangling_source_references,
    })
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ActivationJournalV3 {
    #[serde(flatten)]
    journal: ActivationJournal,
    #[serde(default)]
    vpn_configs: Vec<VpnConfigActivationJournalEntry>,
    #[serde(default)]
    vpns: Vec<EntityActivationJournalEntry>,
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

fn matches_entity<T: buglogin_storage::local::LocalEntity>(
    repository: &LocalRepository,
    id: &str,
    revision: u64,
    sha256: &str,
) -> Result<bool, Problem> {
    Ok(repository
        .get::<T>(id)?
        .and_then(|current| {
            serde_json::to_vec(&current.value).ok().map(|bytes| {
                current.revision == revision && buglogin_storage::blob::checksum(&bytes) == sha256
            })
        })
        .unwrap_or(false))
}
