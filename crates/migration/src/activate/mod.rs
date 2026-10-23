use crate::{
    encrypted_secrets::ProxyCredentialSecret,
    encrypted_vpn_configs::VpnConfigSecret,
    legacy::{
        ExtensionArtifact, NeutralExtensionGroupRecord, NeutralExtensionRecord, NeutralGroupRecord,
        NeutralProxyRecord, NeutralVpnRecord, legacy_runtime_fingerprint,
    },
    transform::{
        NeutralProfileRecord, ensure_directory, io_problem, problem, reject_symlink,
        validate_run_id,
    },
};
use buglogin_domain::{
    entities::{
        BrowserCompatibility, Extension, ExtensionGroup, ExtensionPackageType, Group, Profile,
        ProfileState, Proxy, ProxyProtocol, Runtime, SyncMode, Vpn, VpnKind,
        validate_wayfern_runtime_config,
    },
    errors::Problem,
    ids::{
        ArtifactId, CredentialId, ExtensionGroupId, ExtensionId, GroupId, ProfileId, ProxyId,
        VpnId, migration_uuid,
    },
};
use buglogin_storage::{
    local::{LocalEntity, LocalRepository},
    secrets::{OsVault, SecretError, SecretKey, SecretValue, SecretVersion, UnavailableVault},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub(crate) const ACTIVATION_DIRECTORY: &str = "migration-activations";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationStatus {
    Active,
    RolledBack,
}

fn write_v3(path: &Path, journal: &ActivationJournalV3) -> Result<(), Problem> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_problem("create activation journal", &error))?;
    let bytes =
        serde_json::to_vec_pretty(journal).map_err(|_| invalid("journal", "serialization"))?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| io_problem("write activation journal", &error))
}

fn convert_vpn(record: &NeutralVpnRecord) -> Result<Vpn, Problem> {
    validate_record(
        &record.schema,
        "buglogin.vpn/1",
        record.revision,
        &record.id,
        &record.source_ref.system,
        &record.source_ref.instance_id,
        "vpn",
        &record.source_ref.id,
    )?;
    let expected_config_ref = migration_uuid(
        &record.source_ref.system,
        &record.source_ref.instance_id,
        "vpn-config",
        &record.source_ref.id,
    )
    .to_string();
    if record.data.name.trim().is_empty() || record.data.config_ref != expected_config_ref {
        return Err(invalid("vpnId", &record.id));
    }
    if let Some(last_used_at) = &record.data.last_used_at
        && (!last_used_at.contains('T') || !last_used_at.ends_with('Z'))
    {
        return Err(invalid("vpnId", &record.id));
    }
    let (created_at, updated_at) =
        timestamps(&record.created_at, &record.updated_at, "vpnId", &record.id)?;
    Ok(Vpn {
        id: VpnId::new(record.id.clone()).map_err(|_| invalid("vpnId", &record.id))?,
        name: record.data.name.clone(),
        kind: match record.data.kind.as_str() {
            "wireguard" => VpnKind::Wireguard,
            "openvpn" => VpnKind::Openvpn,
            _ => return Err(invalid("vpnId", &record.id)),
        },
        config_ref: CredentialId::new(record.data.config_ref.clone())
            .map_err(|_| invalid("vpnId", &record.id))?,
        last_used_at: record.data.last_used_at.clone(),
        sync_mode: Some(parse_sync_mode(
            &record.data.sync_mode,
            "vpnId",
            &record.id,
        )?),
        created_at,
        updated_at,
    })
}

fn rerun_v3(
    root: &Path,
    journal: ActivationJournalV3,
    path: PathBuf,
    vault: &impl OsVault,
) -> Result<ActivationReceipt, Problem> {
    if journal.journal.status != ActivationStatus::Active {
        return Err(problem(
            "migration_activation_already_rolled_back",
            "Migration activation was already rolled back",
            409,
        ));
    }
    let repository = LocalRepository::open(root)?;
    for entry in &journal.vpn_configs {
        verify_vpn_config(vault, entry)?;
    }
    for entry in &journal.vpns {
        verify_entity::<Vpn>(&repository, entry, "vpnId")?;
    }
    let vpn_config_count = journal.vpn_configs.len();
    let vpn_count = journal.vpns.len();
    let mut receipt = rerun(root, journal.journal, path, vault)?;
    receipt.reused_vpn_configs = vpn_config_count;
    receipt.reused_vpns = vpn_count;
    Ok(receipt)
}

fn vpn_kind_name(kind: &VpnKind) -> &'static str {
    match kind {
        VpnKind::Wireguard => "wireguard",
        VpnKind::Openvpn => "openvpn",
    }
}

fn receipt_v3(journal: &ActivationJournalV3, journal_path: PathBuf) -> ActivationReceipt {
    let mut receipt = receipt(&journal.journal, journal_path);
    receipt.created_vpn_configs = journal
        .vpn_configs
        .iter()
        .filter(|entry| entry.created)
        .count();
    receipt.reused_vpn_configs = journal
        .vpn_configs
        .iter()
        .filter(|entry| !entry.created)
        .count();
    receipt.created_vpns = journal.vpns.iter().filter(|entry| entry.created).count();
    receipt.reused_vpns = journal.vpns.iter().filter(|entry| !entry.created).count();
    receipt
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationJournalEntry {
    pub profile_id: String,
    pub revision: u64,
    pub sha256: String,
    pub created: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntityActivationJournalEntry {
    pub entity_id: String,
    pub revision: u64,
    pub sha256: String,
    pub created: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialActivationJournalEntry {
    pub credential_id: String,
    pub version: u64,
    pub sha256: String,
    pub created: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VpnConfigActivationJournalEntry {
    pub config_ref: String,
    pub version: u64,
    pub sha256: String,
    pub created: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactActivationJournalEntry {
    pub artifact_ref: String,
    pub path: String,
    pub sha256: String,
    pub created: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationJournal {
    pub schema: String,
    pub run_id: String,
    pub status: ActivationStatus,
    pub input_sha256: String,
    #[serde(default)]
    pub credentials: Vec<CredentialActivationJournalEntry>,
    #[serde(default)]
    pub extension_artifacts: Vec<ArtifactActivationJournalEntry>,
    #[serde(default)]
    pub extensions: Vec<EntityActivationJournalEntry>,
    #[serde(default)]
    pub extension_groups: Vec<EntityActivationJournalEntry>,
    #[serde(default)]
    pub groups: Vec<EntityActivationJournalEntry>,
    #[serde(default)]
    pub proxies: Vec<EntityActivationJournalEntry>,
    #[serde(default)]
    pub profiles: Vec<ActivationJournalEntry>,
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActivationReceipt {
    pub run_id: String,
    pub created_credentials: usize,
    pub reused_credentials: usize,
    pub created_extension_artifacts: usize,
    pub reused_extension_artifacts: usize,
    pub created_extensions: usize,
    pub reused_extensions: usize,
    pub created_extension_groups: usize,
    pub reused_extension_groups: usize,
    pub created_vpn_configs: usize,
    pub reused_vpn_configs: usize,
    pub created_vpns: usize,
    pub reused_vpns: usize,
    pub created_groups: usize,
    pub reused_groups: usize,
    pub created_proxies: usize,
    pub reused_proxies: usize,
    pub created_profiles: usize,
    pub reused_profiles: usize,
    pub created_preserved_profiles: usize,
    pub reused_preserved_profiles: usize,
    pub journal_path: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivationPlan {
    #[serde(default)]
    pub create_credential_ids: Vec<String>,
    #[serde(default)]
    pub reuse_credential_ids: Vec<String>,
    #[serde(default)]
    pub create_extension_artifact_ids: Vec<String>,
    #[serde(default)]
    pub reuse_extension_artifact_ids: Vec<String>,
    #[serde(default)]
    pub create_extension_ids: Vec<String>,
    #[serde(default)]
    pub reuse_extension_ids: Vec<String>,
    #[serde(default)]
    pub create_extension_group_ids: Vec<String>,
    #[serde(default)]
    pub reuse_extension_group_ids: Vec<String>,
    #[serde(default)]
    pub create_vpn_config_ids: Vec<String>,
    #[serde(default)]
    pub reuse_vpn_config_ids: Vec<String>,
    #[serde(default)]
    pub create_vpn_ids: Vec<String>,
    #[serde(default)]
    pub reuse_vpn_ids: Vec<String>,
    #[serde(default)]
    pub create_group_ids: Vec<String>,
    #[serde(default)]
    pub reuse_group_ids: Vec<String>,
    #[serde(default)]
    pub create_proxy_ids: Vec<String>,
    #[serde(default)]
    pub reuse_proxy_ids: Vec<String>,
    #[serde(default)]
    pub create_profile_ids: Vec<String>,
    #[serde(default)]
    pub reuse_profile_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub create_preserved_profile_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reuse_preserved_profile_ids: Vec<String>,
}

pub fn plan_profiles(
    root: &Path,
    records: &[NeutralProfileRecord],
) -> Result<ActivationPlan, Problem> {
    plan_records(root, records, &[], &[])
}

pub fn plan_records(
    root: &Path,
    profile_records: &[NeutralProfileRecord],
    group_records: &[NeutralGroupRecord],
    proxy_records: &[NeutralProxyRecord],
) -> Result<ActivationPlan, Problem> {
    plan_records_with_vault(
        root,
        profile_records,
        group_records,
        proxy_records,
        &[],
        &UnavailableVault,
    )
}

pub fn plan_records_with_vault<V: OsVault>(
    root: &Path,
    profile_records: &[NeutralProfileRecord],
    group_records: &[NeutralGroupRecord],
    proxy_records: &[NeutralProxyRecord],
    credentials: &[ProxyCredentialSecret],
    vault: &V,
) -> Result<ActivationPlan, Problem> {
    plan_records_v2_with_vault(
        root,
        profile_records,
        group_records,
        proxy_records,
        credentials,
        &[],
        &[],
        &[],
        vault,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn plan_records_v2_with_vault<V: OsVault>(
    root: &Path,
    profile_records: &[NeutralProfileRecord],
    group_records: &[NeutralGroupRecord],
    proxy_records: &[NeutralProxyRecord],
    credentials: &[ProxyCredentialSecret],
    extension_records: &[NeutralExtensionRecord],
    extension_group_records: &[NeutralExtensionGroupRecord],
    extension_artifacts: &[ExtensionArtifact],
    vault: &V,
) -> Result<ActivationPlan, Problem> {
    plan_records_v3_with_vault(
        root,
        profile_records,
        group_records,
        proxy_records,
        credentials,
        extension_records,
        extension_group_records,
        extension_artifacts,
        &[],
        &[],
        vault,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn plan_records_v3_with_vault<V: OsVault>(
    root: &Path,
    profile_records: &[NeutralProfileRecord],
    group_records: &[NeutralGroupRecord],
    proxy_records: &[NeutralProxyRecord],
    credentials: &[ProxyCredentialSecret],
    extension_records: &[NeutralExtensionRecord],
    extension_group_records: &[NeutralExtensionGroupRecord],
    extension_artifacts: &[ExtensionArtifact],
    vpn_records: &[NeutralVpnRecord],
    vpn_configs: &[VpnConfigSecret],
    vault: &V,
) -> Result<ActivationPlan, Problem> {
    let groups = prepare(
        group_records
            .iter()
            .map(convert_group)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let proxies = prepare(
        proxy_records
            .iter()
            .map(convert_proxy)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let profiles = prepare(
        profile_records
            .iter()
            .map(convert_profile)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let extensions = prepare(
        extension_records
            .iter()
            .map(convert_extension)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let extension_groups = prepare(
        extension_group_records
            .iter()
            .map(convert_extension_group)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let vpns = prepare(
        vpn_records
            .iter()
            .map(convert_vpn)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    validate_credential_references(&proxies, credentials)?;
    validate_vpn_config_references(&vpns, vpn_configs)?;
    validate_extension_artifacts(&extensions, extension_artifacts)?;
    validate_extension_references(root, &profiles, &extensions, &extension_groups)?;
    validate_references(root, &profiles, &groups, &proxies, &vpns)?;
    let (create_credential_ids, reuse_credential_ids) = plan_credentials(vault, credentials)?;
    let (create_vpn_config_ids, reuse_vpn_config_ids) = plan_vpn_configs(vault, vpn_configs)?;
    let (create_extension_artifact_ids, reuse_extension_artifact_ids) =
        plan_extension_artifacts(root, extension_artifacts)?;
    if !root.exists() {
        return Ok(ActivationPlan {
            create_credential_ids,
            reuse_credential_ids,
            create_extension_artifact_ids,
            reuse_extension_artifact_ids,
            create_extension_ids: ids(&extensions),
            create_extension_group_ids: ids(&extension_groups),
            create_vpn_config_ids,
            reuse_vpn_config_ids,
            create_vpn_ids: ids(&vpns),
            create_group_ids: ids(&groups),
            create_proxy_ids: ids(&proxies),
            create_profile_ids: ids(&profiles),
            ..ActivationPlan::default()
        });
    }
    reject_symlink(root)?;
    let repository = LocalRepository::open(root)?;
    let (create_group_ids, reuse_group_ids) =
        plan_entities::<Group>(&repository, &groups, "groupId")?;
    let (create_proxy_ids, reuse_proxy_ids) =
        plan_entities::<Proxy>(&repository, &proxies, "proxyId")?;
    let (create_profile_ids, reuse_profile_ids) =
        plan_entities::<Profile>(&repository, &profiles, "profileId")?;
    let (create_extension_ids, reuse_extension_ids) =
        plan_entities::<Extension>(&repository, &extensions, "extensionId")?;
    let (create_extension_group_ids, reuse_extension_group_ids) =
        plan_entities::<ExtensionGroup>(&repository, &extension_groups, "extensionGroupId")?;
    let (create_vpn_ids, reuse_vpn_ids) = plan_entities::<Vpn>(&repository, &vpns, "vpnId")?;
    Ok(ActivationPlan {
        create_credential_ids,
        reuse_credential_ids,
        create_extension_artifact_ids,
        reuse_extension_artifact_ids,
        create_extension_ids,
        reuse_extension_ids,
        create_extension_group_ids,
        reuse_extension_group_ids,
        create_vpn_config_ids,
        reuse_vpn_config_ids,
        create_vpn_ids,
        reuse_vpn_ids,
        create_group_ids,
        reuse_group_ids,
        create_proxy_ids,
        reuse_proxy_ids,
        create_profile_ids,
        reuse_profile_ids,
        create_preserved_profile_ids: Vec::new(),
        reuse_preserved_profile_ids: Vec::new(),
    })
}

pub fn activate_profiles(
    root: &Path,
    run_id: &str,
    records: &[NeutralProfileRecord],
) -> Result<ActivationReceipt, Problem> {
    activate_records(root, run_id, records, &[], &[])
}

pub fn activate_records(
    root: &Path,
    run_id: &str,
    profile_records: &[NeutralProfileRecord],
    group_records: &[NeutralGroupRecord],
    proxy_records: &[NeutralProxyRecord],
) -> Result<ActivationReceipt, Problem> {
    activate_records_with_vault(
        root,
        run_id,
        profile_records,
        group_records,
        proxy_records,
        &[],
        &mut UnavailableVault,
    )
}

pub fn activate_records_with_vault<V: OsVault>(
    root: &Path,
    run_id: &str,
    profile_records: &[NeutralProfileRecord],
    group_records: &[NeutralGroupRecord],
    proxy_records: &[NeutralProxyRecord],
    credentials: &[ProxyCredentialSecret],
    vault: &mut V,
) -> Result<ActivationReceipt, Problem> {
    activate_records_v2_with_vault(
        root,
        run_id,
        profile_records,
        group_records,
        proxy_records,
        credentials,
        &[],
        &[],
        &[],
        vault,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn activate_records_v2_with_vault<V: OsVault>(
    root: &Path,
    run_id: &str,
    profile_records: &[NeutralProfileRecord],
    group_records: &[NeutralGroupRecord],
    proxy_records: &[NeutralProxyRecord],
    credentials: &[ProxyCredentialSecret],
    extension_records: &[NeutralExtensionRecord],
    extension_group_records: &[NeutralExtensionGroupRecord],
    extension_artifacts: &[ExtensionArtifact],
    vault: &mut V,
) -> Result<ActivationReceipt, Problem> {
    activate_records_v3_with_vault(
        root,
        run_id,
        profile_records,
        group_records,
        proxy_records,
        credentials,
        extension_records,
        extension_group_records,
        extension_artifacts,
        &[],
        &[],
        vault,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn activate_records_v3_with_vault<V: OsVault>(
    root: &Path,
    run_id: &str,
    profile_records: &[NeutralProfileRecord],
    group_records: &[NeutralGroupRecord],
    proxy_records: &[NeutralProxyRecord],
    credentials: &[ProxyCredentialSecret],
    extension_records: &[NeutralExtensionRecord],
    extension_group_records: &[NeutralExtensionGroupRecord],
    extension_artifacts: &[ExtensionArtifact],
    vpn_records: &[NeutralVpnRecord],
    vpn_configs: &[VpnConfigSecret],
    vault: &mut V,
) -> Result<ActivationReceipt, Problem> {
    validate_run_id(run_id)?;
    let groups = prepare(
        group_records
            .iter()
            .map(convert_group)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let proxies = prepare(
        proxy_records
            .iter()
            .map(convert_proxy)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let profiles = prepare(
        profile_records
            .iter()
            .map(convert_profile)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let extensions = prepare(
        extension_records
            .iter()
            .map(convert_extension)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let extension_groups = prepare(
        extension_group_records
            .iter()
            .map(convert_extension_group)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    let vpns = prepare(
        vpn_records
            .iter()
            .map(convert_vpn)
            .collect::<Result<Vec<_>, _>>()?,
    )?;
    validate_credential_references(&proxies, credentials)?;
    validate_vpn_config_references(&vpns, vpn_configs)?;
    validate_extension_artifacts(&extensions, extension_artifacts)?;
    validate_extension_references(root, &profiles, &extensions, &extension_groups)?;
    validate_references(root, &profiles, &groups, &proxies, &vpns)?;
    let input_sha256 = hash(&(
        credentials,
        &groups,
        &proxies,
        &extensions,
        &extension_groups,
        &profiles,
        extension_artifacts,
        &vpns,
        vpn_configs,
    ))?;
    ensure_directory(root)?;
    reject_symlink(root)?;
    let directory = root.join(ACTIVATION_DIRECTORY);
    ensure_directory(&directory)?;
    reject_symlink(&directory)?;
    let path = journal_path(root, run_id);
    let pending_path = activation_pending_path(root, run_id);
    if path.exists() {
        let journal = read_activation_journal_v3(root, run_id)?;
        if journal.journal.input_sha256 != input_sha256 {
            return Err(problem(
                "migration_activation_rerun_mismatch",
                "Migration activation rerun input differs",
                409,
            ));
        }
        let _ = fs::remove_file(&pending_path);
        return rerun_v3(root, journal, path, vault);
    }

    if pending_path.exists() {
        let pending = read_activation_pending_journal_v3(&pending_path, run_id)?;
        if pending.journal.input_sha256 != input_sha256 {
            return Err(problem(
                "migration_activation_pending_mismatch",
                "Migration activation pending input differs",
                409,
            ));
        }
    }

    let repository = LocalRepository::open(root)?;
    if !pending_path.exists() {
        write_v3(&pending_path, &pending_journal(run_id, &input_sha256))?;
    }
    let mut created_credentials = Vec::new();
    let mut created_groups = Vec::new();
    let mut created_proxies = Vec::new();
    let mut created_artifacts = Vec::new();
    let mut created_extensions = Vec::new();
    let mut created_extension_groups = Vec::new();
    let mut created_vpn_configs = Vec::new();
    let mut created_vpns = Vec::new();
    let mut created_profiles = Vec::new();

    let result = (|| {
        let credential_entries =
            activate_credentials(vault, credentials, &mut created_credentials)?;
        let vpn_config_entries =
            activate_vpn_configs(vault, vpn_configs, &mut created_vpn_configs)?;
        let group_entries = activate_entities(&repository, groups, "groupId", &mut created_groups)?;
        let proxy_entries =
            activate_entities(&repository, proxies, "proxyId", &mut created_proxies)?;
        let artifact_entries =
            activate_extension_artifacts(root, extension_artifacts, &mut created_artifacts)?;
        let extension_entries = activate_entities(
            &repository,
            extensions,
            "extensionId",
            &mut created_extensions,
        )?;
        let extension_group_entries = activate_entities(
            &repository,
            extension_groups,
            "extensionGroupId",
            &mut created_extension_groups,
        )?;
        let vpn_entries = activate_entities(&repository, vpns, "vpnId", &mut created_vpns)?;
        let profile_entries =
            activate_entities(&repository, profiles, "profileId", &mut created_profiles)?
                .into_iter()
                .map(|entry| ActivationJournalEntry {
                    profile_id: entry.entity_id,
                    revision: entry.revision,
                    sha256: entry.sha256,
                    created: entry.created,
                })
                .collect();
        Ok::<_, Problem>(ActivationJournalV3 {
            journal: ActivationJournal {
                schema: "buglogin.activation/1".into(),
                run_id: run_id.into(),
                status: ActivationStatus::Active,
                input_sha256,
                credentials: credential_entries,
                extension_artifacts: artifact_entries,
                extensions: extension_entries,
                extension_groups: extension_group_entries,
                groups: group_entries,
                proxies: proxy_entries,
                profiles: profile_entries,
            },
            vpn_configs: vpn_config_entries,
            vpns: vpn_entries,
        })
    })();

    let journal = match result {
        Ok(journal) => journal,
        Err(error) => {
            return fail_with_cleanup_v3(
                &repository,
                root,
                &pending_path,
                vault,
                error,
                &created_credentials,
                &created_groups,
                &created_proxies,
                &created_artifacts,
                &created_extensions,
                &created_extension_groups,
                &created_vpn_configs,
                &created_vpns,
                &created_profiles,
            );
        }
    };
    if let Err(error) = write_v3(&path, &journal) {
        return fail_with_cleanup_v3(
            &repository,
            root,
            &pending_path,
            vault,
            error,
            &created_credentials,
            &created_groups,
            &created_proxies,
            &created_artifacts,
            &created_extensions,
            &created_extension_groups,
            &created_vpn_configs,
            &created_vpns,
            &created_profiles,
        );
    }
    let receipt = receipt_v3(&journal, path);
    let _ = fs::remove_file(pending_path);
    Ok(receipt)
}

fn prepare<T: LocalEntity>(mut entities: Vec<T>) -> Result<Vec<T>, Problem> {
    entities.sort_by(|left, right| left.entity_id().cmp(right.entity_id()));
    let mut ids = BTreeSet::new();
    if entities
        .iter()
        .any(|entity| !ids.insert(entity.entity_id()))
    {
        return Err(problem(
            "migration_activation_duplicate_id",
            "Migration activation contains duplicate IDs",
            422,
        ));
    }
    Ok(entities)
}

fn validate_credential_references(
    proxies: &[Proxy],
    credentials: &[ProxyCredentialSecret],
) -> Result<(), Problem> {
    let mut credential_ids = BTreeSet::new();
    for credential in credentials {
        if CredentialId::new(credential.credential_id.clone()).is_err()
            || !credential_ids.insert(credential.credential_id.as_str())
            || (credential.username.is_none() && credential.password.is_none())
        {
            return Err(invalid("credentialId", &credential.credential_id));
        }
    }
    let referenced = proxies
        .iter()
        .filter_map(|proxy| proxy.credential_ref.as_ref())
        .map(|id| id.as_str())
        .collect::<BTreeSet<_>>();
    if referenced != credential_ids {
        return Err(problem(
            "migration_proxy_credential_reference_mismatch",
            "Migration proxy credential references do not match encrypted credentials",
            422,
        ));
    }
    Ok(())
}

fn verify_vpn_config(
    vault: &impl OsVault,
    entry: &VpnConfigActivationJournalEntry,
) -> Result<(), Problem> {
    let key = vpn_config_key(&entry.config_ref)?;
    let (value, version) = vault
        .read(&key)
        .map_err(|_| changed("configRef", &entry.config_ref))?;
    if version.generation() != entry.version
        || buglogin_storage::blob::checksum(value.as_bytes()) != entry.sha256
    {
        return Err(changed("configRef", &entry.config_ref));
    }
    Ok(())
}

fn plan_credentials<V: OsVault>(
    vault: &V,
    credentials: &[ProxyCredentialSecret],
) -> Result<(Vec<String>, Vec<String>), Problem> {
    let mut create = Vec::new();
    let mut reuse = Vec::new();
    for credential in sorted_credentials(credentials) {
        let key = credential_key(&credential.credential_id)?;
        let value = credential_value(credential)?;
        match vault.read(&key) {
            Ok((existing, _)) if existing.as_bytes() == value.as_bytes() => {
                reuse.push(credential.credential_id.clone());
            }
            Ok(_) => return Err(collision("credentialId", credential.credential_id.clone())),
            Err(SecretError::NotFound) => create.push(credential.credential_id.clone()),
            Err(error) => return Err(vault_problem(error, "read proxy credential")),
        }
    }
    Ok((create, reuse))
}

fn activate_credentials<V: OsVault>(
    vault: &mut V,
    credentials: &[ProxyCredentialSecret],
    created: &mut Vec<(String, SecretVersion)>,
) -> Result<Vec<CredentialActivationJournalEntry>, Problem> {
    let mut entries = Vec::new();
    for credential in sorted_credentials(credentials) {
        let key = credential_key(&credential.credential_id)?;
        let value = credential_value(credential)?;
        let sha256 = buglogin_storage::blob::checksum(value.as_bytes());
        match vault.read(&key) {
            Ok((existing, version)) if existing.as_bytes() == value.as_bytes() => {
                entries.push(CredentialActivationJournalEntry {
                    credential_id: credential.credential_id.clone(),
                    version: version.generation(),
                    sha256,
                    created: false,
                });
            }
            Ok(_) => return Err(collision("credentialId", credential.credential_id.clone())),
            Err(SecretError::NotFound) => {
                let version = vault
                    .create(&key, value)
                    .map_err(|error| vault_problem(error, "create proxy credential"))?;
                created.push((credential.credential_id.clone(), version));
                entries.push(CredentialActivationJournalEntry {
                    credential_id: credential.credential_id.clone(),
                    version: version.generation(),
                    sha256,
                    created: true,
                });
            }
            Err(error) => return Err(vault_problem(error, "read proxy credential")),
        }
    }
    Ok(entries)
}

fn sorted_credentials(credentials: &[ProxyCredentialSecret]) -> Vec<&ProxyCredentialSecret> {
    let mut sorted = credentials.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.credential_id.cmp(&right.credential_id));
    sorted
}

fn credential_key(id: &str) -> Result<SecretKey, Problem> {
    SecretKey::new("proxy-credentials", id)
        .map_err(|error| vault_problem(error, "build proxy credential key"))
}

fn credential_value(credential: &ProxyCredentialSecret) -> Result<SecretValue, Problem> {
    serde_json::to_vec(credential)
        .map_err(|_| invalid("credentialId", &credential.credential_id))
        .and_then(|bytes| {
            SecretValue::new(bytes).map_err(|error| vault_problem(error, "encode proxy credential"))
        })
}

fn validate_vpn_config_references(
    vpns: &[Vpn],
    configs: &[VpnConfigSecret],
) -> Result<(), Problem> {
    let expected = vpns
        .iter()
        .map(|vpn| {
            (
                vpn.config_ref.as_str(),
                vpn.id.as_str(),
                vpn_kind_name(&vpn.kind),
            )
        })
        .collect::<BTreeSet<_>>();
    let mut actual = BTreeSet::new();
    for config in configs {
        if config.schema != "buglogin.vpn-config-secret/1"
            || CredentialId::new(config.config_ref.clone()).is_err()
            || VpnId::new(config.vpn_id.clone()).is_err()
            || config.config_data.is_empty()
            || !valid_sha256(&config.config_sha256)
            || buglogin_storage::blob::checksum(config.config_data.as_bytes())
                != config.config_sha256
            || !actual.insert((
                config.config_ref.as_str(),
                config.vpn_id.as_str(),
                vpn_kind_name(&config.kind),
            ))
        {
            return Err(invalid("configRef", &config.config_ref));
        }
    }
    if expected != actual {
        return Err(problem(
            "migration_vpn_config_reference_mismatch",
            "Migration VPN references do not match encrypted configs",
            422,
        ));
    }
    Ok(())
}

fn plan_vpn_configs<V: OsVault>(
    vault: &V,
    configs: &[VpnConfigSecret],
) -> Result<(Vec<String>, Vec<String>), Problem> {
    let mut create = Vec::new();
    let mut reuse = Vec::new();
    for config in sorted_vpn_configs(configs) {
        let key = vpn_config_key(&config.config_ref)?;
        let value = vpn_config_value(config)?;
        match vault.read(&key) {
            Ok((existing, _)) if existing.as_bytes() == value.as_bytes() => {
                reuse.push(config.config_ref.clone());
            }
            Ok(_) => return Err(collision("configRef", config.config_ref.clone())),
            Err(SecretError::NotFound) => create.push(config.config_ref.clone()),
            Err(error) => return Err(vault_problem(error, "read VPN config")),
        }
    }
    Ok((create, reuse))
}

fn activate_vpn_configs<V: OsVault>(
    vault: &mut V,
    configs: &[VpnConfigSecret],
    created: &mut Vec<(String, SecretVersion)>,
) -> Result<Vec<VpnConfigActivationJournalEntry>, Problem> {
    let mut entries = Vec::new();
    for config in sorted_vpn_configs(configs) {
        let key = vpn_config_key(&config.config_ref)?;
        let value = vpn_config_value(config)?;
        let sha256 = buglogin_storage::blob::checksum(value.as_bytes());
        match vault.read(&key) {
            Ok((existing, version)) if existing.as_bytes() == value.as_bytes() => {
                entries.push(VpnConfigActivationJournalEntry {
                    config_ref: config.config_ref.clone(),
                    version: version.generation(),
                    sha256,
                    created: false,
                });
            }
            Ok(_) => return Err(collision("configRef", config.config_ref.clone())),
            Err(SecretError::NotFound) => {
                let version = vault
                    .create(&key, value)
                    .map_err(|error| vault_problem(error, "create VPN config"))?;
                created.push((config.config_ref.clone(), version));
                entries.push(VpnConfigActivationJournalEntry {
                    config_ref: config.config_ref.clone(),
                    version: version.generation(),
                    sha256,
                    created: true,
                });
            }
            Err(error) => return Err(vault_problem(error, "read VPN config")),
        }
    }
    Ok(entries)
}

fn sorted_vpn_configs(configs: &[VpnConfigSecret]) -> Vec<&VpnConfigSecret> {
    let mut sorted = configs.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.config_ref.cmp(&right.config_ref));
    sorted
}

fn vpn_config_key(id: &str) -> Result<SecretKey, Problem> {
    SecretKey::new("vpn-configs", id).map_err(|error| vault_problem(error, "build VPN config key"))
}

fn vpn_config_value(config: &VpnConfigSecret) -> Result<SecretValue, Problem> {
    serde_json::to_vec(config)
        .map_err(|_| invalid("configRef", &config.config_ref))
        .and_then(|bytes| {
            SecretValue::new(bytes).map_err(|error| vault_problem(error, "encode VPN config"))
        })
}

fn validate_references(
    root: &Path,
    profiles: &[Profile],
    groups: &[Group],
    proxies: &[Proxy],
    vpns: &[Vpn],
) -> Result<(), Problem> {
    let imported_groups = groups
        .iter()
        .map(|group| group.id.as_str())
        .collect::<BTreeSet<_>>();
    let imported_proxies = proxies
        .iter()
        .map(|proxy| proxy.id.as_str())
        .collect::<BTreeSet<_>>();
    let imported_vpns = vpns
        .iter()
        .map(|vpn| vpn.id.as_str())
        .collect::<BTreeSet<_>>();
    let repository = if root.exists() {
        reject_symlink(root)?;
        Some(LocalRepository::open(root)?)
    } else {
        None
    };
    for profile in profiles {
        if profile.proxy_id.is_some() && profile.vpn_id.is_some() {
            return Err(problem(
                "migration_profile_network_conflict",
                "Migration profile cannot reference both proxy and VPN",
                422,
            )
            .with_field("profileId", profile.id.as_str()));
        }
        for group_id in &profile.group_ids {
            if !imported_groups.contains(group_id.as_str())
                && !exists::<Group>(repository.as_ref(), group_id.as_str())?
            {
                return Err(missing_reference(
                    "groupId",
                    group_id.as_str(),
                    profile.id.as_str(),
                ));
            }
        }
        if let Some(proxy_id) = &profile.proxy_id
            && !imported_proxies.contains(proxy_id.as_str())
            && !exists::<Proxy>(repository.as_ref(), proxy_id.as_str())?
        {
            return Err(missing_reference(
                "proxyId",
                proxy_id.as_str(),
                profile.id.as_str(),
            ));
        }
        if let Some(vpn_id) = &profile.vpn_id
            && !imported_vpns.contains(vpn_id.as_str())
            && !exists::<Vpn>(repository.as_ref(), vpn_id.as_str())?
        {
            return Err(missing_reference(
                "vpnId",
                vpn_id.as_str(),
                profile.id.as_str(),
            ));
        }
    }
    Ok(())
}

fn exists<T: LocalEntity>(repository: Option<&LocalRepository>, id: &str) -> Result<bool, Problem> {
    repository.map_or(Ok(false), |repository| {
        Ok(repository.get::<T>(id)?.is_some())
    })
}

fn plan_entities<T>(
    repository: &LocalRepository,
    entities: &[T],
    field: &str,
) -> Result<(Vec<String>, Vec<String>), Problem>
where
    T: LocalEntity + PartialEq,
{
    let mut create = Vec::new();
    let mut reuse = Vec::new();
    for entity in entities {
        let id = entity.entity_id().to_owned();
        match repository.get::<T>(&id)? {
            Some(existing) if existing.value == *entity => reuse.push(id),
            Some(_) => return Err(collision(field, id)),
            None => create.push(id),
        }
    }
    Ok((create, reuse))
}

fn activate_entities<T>(
    repository: &LocalRepository,
    entities: Vec<T>,
    field: &str,
    created: &mut Vec<(String, u64)>,
) -> Result<Vec<EntityActivationJournalEntry>, Problem>
where
    T: LocalEntity + PartialEq,
{
    let mut entries = Vec::new();
    for entity in entities {
        let id = entity.entity_id().to_owned();
        let sha256 = hash(&entity)?;
        match repository.get::<T>(&id)? {
            Some(existing) if existing.value == entity => {
                entries.push(EntityActivationJournalEntry {
                    entity_id: id,
                    revision: existing.revision,
                    sha256,
                    created: false,
                })
            }
            Some(_) => return Err(collision(field, id)),
            None => {
                let versioned = repository.create(entity)?;
                created.push((id.clone(), versioned.revision));
                entries.push(EntityActivationJournalEntry {
                    entity_id: id,
                    revision: versioned.revision,
                    sha256,
                    created: true,
                });
            }
        }
    }
    Ok(entries)
}

fn convert_profile(record: &NeutralProfileRecord) -> Result<Profile, Problem> {
    validate_record(
        &record.schema,
        "buglogin.profile/1",
        record.revision,
        &record.id,
        &record.source_ref.system,
        &record.source_ref.instance_id,
        "profile",
        &record.source_ref.id,
    )?;
    if record.data.name.trim().is_empty()
        || record.data.ephemeral
        || record.data.release_type != "stable"
        || !matches!(record.data.migration_class.as_str(), "direct" | "adapter")
    {
        return Err(invalid("profileId", &record.id));
    }
    let runtime = match record.data.runtime.as_str() {
        "wayfern" => Runtime::Wayfern,
        "bugox" => Runtime::Bugox,
        "bugium" => Runtime::Bugium,
        "bugmini" => Runtime::Bugmini,
        "bugthor" => Runtime::Bugthor,
        "bughelium" => Runtime::Bughelium,
        _ => return Err(invalid("profileId", &record.id)),
    };
    if record
        .data
        .legacy_runtime_config
        .as_ref()
        .is_some_and(buglogin_domain::security::contains_secret)
    {
        return Err(invalid("legacyRuntimeConfig", &record.id));
    }
    if let Some(runtime_config) = record.data.legacy_runtime_config.as_ref() {
        validate_wayfern_runtime_config(&runtime, runtime_config)
            .map_err(|_| invalid("legacyRuntimeConfig", &record.id))?;
    }
    let (created_at, updated_at) = timestamps(
        &record.created_at,
        &record.updated_at,
        "profileId",
        &record.id,
    )?;
    Ok(Profile {
        id: ProfileId::new(record.id.clone()).map_err(|_| invalid("profileId", &record.id))?,
        name: record.data.name.clone(),
        runtime,
        runtime_version: record.data.runtime_version.clone(),
        runtime_config: record.data.legacy_runtime_config.clone(),
        state: ProfileState::Ready,
        archived: false,
        fingerprint: legacy_runtime_fingerprint(record.data.legacy_runtime_config.as_ref())?,
        note: record.data.note.clone(),
        proxy_id: record
            .data
            .proxy_id
            .as_ref()
            .map(|id| ProxyId::new(id.clone()).map_err(|_| invalid("profileId", &record.id)))
            .transpose()?,
        vpn_id: record
            .data
            .vpn_id
            .as_ref()
            .map(|id| VpnId::new(id.clone()).map_err(|_| invalid("profileId", &record.id)))
            .transpose()?,
        group_ids: record
            .data
            .group_id
            .as_ref()
            .map(|id| GroupId::new(id.clone()).map_err(|_| invalid("profileId", &record.id)))
            .transpose()?
            .into_iter()
            .collect(),
        tags: record.data.tags.clone(),
        extension_group_id: record
            .data
            .extension_group_id
            .as_ref()
            .map(|id| {
                ExtensionGroupId::new(id.clone()).map_err(|_| invalid("profileId", &record.id))
            })
            .transpose()?,
        sync_mode: Some(SyncMode::Local),
        created_at,
        updated_at,
    })
}

fn convert_group(record: &NeutralGroupRecord) -> Result<Group, Problem> {
    validate_record(
        &record.schema,
        "buglogin.group/1",
        record.revision,
        &record.id,
        &record.source_ref.system,
        &record.source_ref.instance_id,
        "group",
        &record.source_ref.id,
    )?;
    if record.data.name.trim().is_empty() {
        return Err(invalid("groupId", &record.id));
    }
    let (created_at, updated_at) = timestamps(
        &record.created_at,
        &record.updated_at,
        "groupId",
        &record.id,
    )?;
    Ok(Group {
        id: GroupId::new(record.id.clone()).map_err(|_| invalid("groupId", &record.id))?,
        name: record.data.name.clone(),
        appearance: record.data.appearance.clone(),
        created_at,
        updated_at,
    })
}

fn convert_proxy(record: &NeutralProxyRecord) -> Result<Proxy, Problem> {
    validate_record(
        &record.schema,
        "buglogin.proxy/1",
        record.revision,
        &record.id,
        &record.source_ref.system,
        &record.source_ref.instance_id,
        "proxy",
        &record.source_ref.id,
    )?;
    if record.data.host.trim().is_empty() || record.data.port == 0 {
        return Err(invalid("proxyId", &record.id));
    }
    let (created_at, updated_at) = timestamps(
        &record.created_at,
        &record.updated_at,
        "proxyId",
        &record.id,
    )?;
    Ok(Proxy {
        id: ProxyId::new(record.id.clone()).map_err(|_| invalid("proxyId", &record.id))?,
        name: None,
        protocol: match record.data.protocol.as_str() {
            "http" => ProxyProtocol::Http,
            "https" => ProxyProtocol::Https,
            "socks4" => ProxyProtocol::Socks4,
            "socks5" => ProxyProtocol::Socks5,
            _ => return Err(invalid("proxyId", &record.id)),
        },
        host: record.data.host.clone(),
        port: record.data.port,
        credential_ref: record
            .data
            .credential_ref
            .as_ref()
            .map(|id| CredentialId::new(id.clone()).map_err(|_| invalid("proxyId", &record.id)))
            .transpose()?,
        rotation_url: record.data.rotation_url.clone(),
        bypass_rules: record.data.bypass_rules.clone(),
        created_at,
        updated_at,
    })
}

fn convert_extension(record: &NeutralExtensionRecord) -> Result<Extension, Problem> {
    validate_record(
        &record.schema,
        "buglogin.extension/1",
        record.revision,
        &record.id,
        &record.source_ref.system,
        &record.source_ref.instance_id,
        "extension",
        &record.source_ref.id,
    )?;
    let expected_artifact = migration_uuid(
        &record.source_ref.system,
        &record.source_ref.instance_id,
        "extension-artifact",
        &record.source_ref.id,
    )
    .to_string();
    if record.data.name.trim().is_empty()
        || record.data.file_name.trim().is_empty()
        || record.data.artifact_ref != expected_artifact
        || !valid_sha256(&record.data.artifact_sha256)
        || record.data.compatibility.is_empty()
    {
        return Err(invalid("extensionId", &record.id));
    }
    let mut compatibility = record
        .data
        .compatibility
        .iter()
        .map(|value| match value.as_str() {
            "firefox" => Ok(BrowserCompatibility::Firefox),
            "chromium" => Ok(BrowserCompatibility::Chromium),
            _ => Err(invalid("extensionId", &record.id)),
        })
        .collect::<Result<Vec<_>, _>>()?;
    compatibility.sort();
    compatibility.dedup();
    let (created_at, updated_at) = timestamps(
        &record.created_at,
        &record.updated_at,
        "extensionId",
        &record.id,
    )?;
    Ok(Extension {
        id: ExtensionId::new(record.id.clone()).map_err(|_| invalid("extensionId", &record.id))?,
        name: record.data.name.clone(),
        file_name: Some(record.data.file_name.clone()),
        package_type: match record.data.package_type.as_str() {
            "xpi" => ExtensionPackageType::Xpi,
            "crx" => ExtensionPackageType::Crx,
            "zip" => ExtensionPackageType::Zip,
            _ => return Err(invalid("extensionId", &record.id)),
        },
        artifact_ref: ArtifactId::new(record.data.artifact_ref.clone())
            .map_err(|_| invalid("extensionId", &record.id))?,
        artifact_sha256: record.data.artifact_sha256.clone(),
        compatibility,
        sync_mode: Some(parse_sync_mode(
            &record.data.sync_mode,
            "extensionId",
            &record.id,
        )?),
        created_at,
        updated_at,
    })
}

fn convert_extension_group(
    record: &NeutralExtensionGroupRecord,
) -> Result<ExtensionGroup, Problem> {
    validate_record(
        &record.schema,
        "buglogin.extension-group/1",
        record.revision,
        &record.id,
        &record.source_ref.system,
        &record.source_ref.instance_id,
        "extension-group",
        &record.source_ref.id,
    )?;
    if record.data.name.trim().is_empty() {
        return Err(invalid("extensionGroupId", &record.id));
    }
    let mut extension_ids = record
        .data
        .extension_ids
        .iter()
        .map(|id| ExtensionId::new(id.clone()).map_err(|_| invalid("extensionGroupId", &record.id)))
        .collect::<Result<Vec<_>, _>>()?;
    extension_ids.sort_by(|left, right| left.as_str().cmp(right.as_str()));
    extension_ids.dedup();
    let (created_at, updated_at) = timestamps(
        &record.created_at,
        &record.updated_at,
        "extensionGroupId",
        &record.id,
    )?;
    Ok(ExtensionGroup {
        id: ExtensionGroupId::new(record.id.clone())
            .map_err(|_| invalid("extensionGroupId", &record.id))?,
        name: record.data.name.clone(),
        extension_ids,
        sync_mode: Some(parse_sync_mode(
            &record.data.sync_mode,
            "extensionGroupId",
            &record.id,
        )?),
        created_at,
        updated_at,
    })
}

fn parse_sync_mode(value: &str, field: &str, id: &str) -> Result<SyncMode, Problem> {
    match value {
        "local" => Ok(SyncMode::Local),
        "managed" => Ok(SyncMode::Managed),
        "disabled" => Ok(SyncMode::Disabled),
        _ => Err(invalid(field, id)),
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn artifact_relative_path(artifact_ref: &str, sha256: &str) -> PathBuf {
    PathBuf::from("extension-artifacts")
        .join(artifact_ref)
        .join(sha256)
}

fn package_artifact_sha256(artifact: &ExtensionArtifact) -> Result<String, Problem> {
    ArtifactId::new(artifact.artifact_ref.clone())
        .map_err(|_| invalid("artifactRef", &artifact.artifact_ref))?;
    let sha256 = buglogin_storage::blob::checksum(&artifact.bytes);
    let expected = format!("artifacts/extensions/{}/{}", artifact.artifact_ref, sha256);
    if artifact.bytes.is_empty() || artifact.package_path.replace('\\', "/") != expected {
        return Err(invalid("artifactRef", &artifact.artifact_ref));
    }
    Ok(sha256)
}

fn validate_extension_artifacts(
    extensions: &[Extension],
    artifacts: &[ExtensionArtifact],
) -> Result<(), Problem> {
    let expected = extensions
        .iter()
        .map(|extension| {
            (
                extension.artifact_ref.as_str().to_owned(),
                extension.artifact_sha256.clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if expected.len() != extensions.len() || artifacts.len() != extensions.len() {
        return Err(problem(
            "migration_extension_artifact_mismatch",
            "Migration extension artifacts do not match extension records",
            422,
        ));
    }
    let mut actual = BTreeMap::new();
    for artifact in artifacts {
        let sha256 = package_artifact_sha256(artifact)?;
        if actual
            .insert(artifact.artifact_ref.clone(), sha256)
            .is_some()
        {
            return Err(invalid("artifactRef", &artifact.artifact_ref));
        }
    }
    if expected != actual {
        return Err(problem(
            "migration_extension_artifact_mismatch",
            "Migration extension artifacts do not match extension records",
            422,
        ));
    }
    Ok(())
}

fn validate_extension_references(
    root: &Path,
    profiles: &[Profile],
    extensions: &[Extension],
    groups: &[ExtensionGroup],
) -> Result<(), Problem> {
    let repository = if root.exists() {
        reject_symlink(root)?;
        Some(LocalRepository::open(root)?)
    } else {
        None
    };
    let imported_extensions = extensions
        .iter()
        .map(|extension| extension.id.as_str())
        .collect::<BTreeSet<_>>();
    let imported_groups = groups
        .iter()
        .map(|group| group.id.as_str())
        .collect::<BTreeSet<_>>();
    for group in groups {
        for id in &group.extension_ids {
            if !imported_extensions.contains(id.as_str())
                && !exists::<Extension>(repository.as_ref(), id.as_str())?
            {
                return Err(missing_reference(
                    "extensionId",
                    id.as_str(),
                    group.id.as_str(),
                ));
            }
        }
    }
    for profile in profiles {
        if let Some(id) = &profile.extension_group_id
            && !imported_groups.contains(id.as_str())
            && !exists::<ExtensionGroup>(repository.as_ref(), id.as_str())?
        {
            return Err(missing_reference(
                "extensionGroupId",
                id.as_str(),
                profile.id.as_str(),
            ));
        }
    }
    Ok(())
}

fn plan_extension_artifacts(
    root: &Path,
    artifacts: &[ExtensionArtifact],
) -> Result<(Vec<String>, Vec<String>), Problem> {
    let mut create = Vec::new();
    let mut reuse = Vec::new();
    for artifact in artifacts {
        let sha256 = package_artifact_sha256(artifact)?;
        let relative = artifact_relative_path(&artifact.artifact_ref, &sha256);
        let target = root.join(&relative);
        match fs::read(&target) {
            Ok(bytes) if buglogin_storage::blob::checksum(&bytes) == sha256 => {
                reuse.push(artifact.artifact_ref.clone())
            }
            Ok(_) => return Err(collision("artifactRef", artifact.artifact_ref.clone())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                create.push(artifact.artifact_ref.clone())
            }
            Err(error) => return Err(io_problem("read extension artifact", &error)),
        }
    }
    create.sort();
    reuse.sort();
    Ok((create, reuse))
}

fn activate_extension_artifacts(
    root: &Path,
    artifacts: &[ExtensionArtifact],
    created: &mut Vec<PathBuf>,
) -> Result<Vec<ArtifactActivationJournalEntry>, Problem> {
    let mut sorted = artifacts.iter().collect::<Vec<_>>();
    sorted.sort_by(|left, right| left.artifact_ref.cmp(&right.artifact_ref));
    let mut entries = Vec::new();
    for artifact in sorted {
        let sha256 = package_artifact_sha256(artifact)?;
        let relative = artifact_relative_path(&artifact.artifact_ref, &sha256);
        let target = root.join(&relative);
        let created_entry = match fs::read(&target) {
            Ok(bytes) if buglogin_storage::blob::checksum(&bytes) == sha256 => false,
            Ok(_) => return Err(collision("artifactRef", artifact.artifact_ref.clone())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                buglogin_storage::blob::write_verified(root, &relative, &artifact.bytes, &sha256)?;
                created.push(relative.clone());
                true
            }
            Err(error) => return Err(io_problem("read extension artifact", &error)),
        };
        entries.push(ArtifactActivationJournalEntry {
            artifact_ref: artifact.artifact_ref.clone(),
            path: relative.to_string_lossy().replace('\\', "/"),
            sha256,
            created: created_entry,
        });
    }
    Ok(entries)
}

fn validate_record(
    schema: &str,
    expected_schema: &str,
    revision: u32,
    id: &str,
    source_system: &str,
    source_instance: &str,
    entity: &str,
    source_id: &str,
) -> Result<(), Problem> {
    let expected = migration_uuid(source_system, source_instance, entity, source_id).to_string();
    if schema != expected_schema || revision != 1 || id != expected {
        Err(invalid(&format!("{entity}Id"), id))
    } else {
        Ok(())
    }
}

fn timestamps(
    created_at: &Option<String>,
    updated_at: &Option<String>,
    field: &str,
    id: &str,
) -> Result<(String, String), Problem> {
    let created = created_at
        .clone()
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    let updated = updated_at.clone().unwrap_or_else(|| created.clone());
    if [created.as_str(), updated.as_str()]
        .iter()
        .any(|value| !value.contains('T') || !value.ends_with('Z'))
    {
        return Err(invalid(field, id));
    }
    Ok((created, updated))
}

pub(crate) fn journal_path(root: &Path, run_id: &str) -> PathBuf {
    root.join(ACTIVATION_DIRECTORY)
        .join(format!("{run_id}.json"))
}

pub(crate) fn activation_pending_path(root: &Path, run_id: &str) -> PathBuf {
    root.join(ACTIVATION_DIRECTORY)
        .join(format!(".{run_id}.pending.json"))
}

fn pending_journal(run_id: &str, input_sha256: &str) -> ActivationJournalV3 {
    ActivationJournalV3 {
        journal: ActivationJournal {
            schema: "buglogin.activation/1".into(),
            run_id: run_id.into(),
            status: ActivationStatus::Active,
            input_sha256: input_sha256.into(),
            credentials: Vec::new(),
            extension_artifacts: Vec::new(),
            extensions: Vec::new(),
            extension_groups: Vec::new(),
            groups: Vec::new(),
            proxies: Vec::new(),
            profiles: Vec::new(),
        },
        vpn_configs: Vec::new(),
        vpns: Vec::new(),
    }
}

pub(crate) fn read_activation_journal(
    root: &Path,
    run_id: &str,
) -> Result<ActivationJournal, Problem> {
    let bytes = fs::read(journal_path(root, run_id))
        .map_err(|error| io_problem("read activation journal", &error))?;
    let journal: ActivationJournal = serde_json::from_slice(&bytes).map_err(|_| {
        problem(
            "migration_activation_journal_invalid",
            "Migration activation journal is invalid",
            409,
        )
    })?;
    if journal.schema != "buglogin.activation/1" || journal.run_id != run_id {
        return Err(problem(
            "migration_activation_journal_invalid",
            "Migration activation journal is invalid",
            409,
        ));
    }
    Ok(journal)
}

fn read_activation_journal_v3(root: &Path, run_id: &str) -> Result<ActivationJournalV3, Problem> {
    read_activation_journal_v3_at(&journal_path(root, run_id), run_id)
}

fn read_activation_pending_journal_v3(
    path: &Path,
    run_id: &str,
) -> Result<ActivationJournalV3, Problem> {
    read_activation_journal_v3_at(path, run_id)
}

fn read_activation_journal_v3_at(
    path: &Path,
    run_id: &str,
) -> Result<ActivationJournalV3, Problem> {
    let bytes = fs::read(path).map_err(|error| io_problem("read activation journal", &error))?;
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

pub(crate) fn replace_activation_journal(
    root: &Path,
    journal: &ActivationJournal,
) -> Result<(), Problem> {
    let path = journal_path(root, &journal.run_id);
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    write(&temporary, journal)?;
    fs::rename(temporary, path).map_err(|error| io_problem("replace activation journal", &error))
}

fn rerun(
    root: &Path,
    journal: ActivationJournal,
    path: PathBuf,
    vault: &impl OsVault,
) -> Result<ActivationReceipt, Problem> {
    if journal.status != ActivationStatus::Active {
        return Err(problem(
            "migration_activation_already_rolled_back",
            "Migration activation was already rolled back",
            409,
        ));
    }
    let repository = LocalRepository::open(root)?;
    for entry in &journal.credentials {
        verify_credential(vault, entry)?;
    }
    for entry in &journal.extension_artifacts {
        buglogin_storage::blob::read_verified(root, Path::new(&entry.path), &entry.sha256)
            .map_err(|_| changed("artifactRef", &entry.artifact_ref))?;
    }
    for entry in &journal.extensions {
        verify_entity::<Extension>(&repository, entry, "extensionId")?;
    }
    for entry in &journal.extension_groups {
        verify_entity::<ExtensionGroup>(&repository, entry, "extensionGroupId")?;
    }
    for entry in &journal.groups {
        verify_entity::<Group>(&repository, entry, "groupId")?;
    }
    for entry in &journal.proxies {
        verify_entity::<Proxy>(&repository, entry, "proxyId")?;
    }
    for entry in &journal.profiles {
        verify::<Profile>(
            &repository,
            &entry.profile_id,
            entry.revision,
            &entry.sha256,
            "profileId",
        )?;
    }
    Ok(ActivationReceipt {
        run_id: journal.run_id,
        created_credentials: 0,
        reused_credentials: journal.credentials.len(),
        created_extension_artifacts: 0,
        reused_extension_artifacts: journal.extension_artifacts.len(),
        created_extensions: 0,
        reused_extensions: journal.extensions.len(),
        created_extension_groups: 0,
        reused_extension_groups: journal.extension_groups.len(),
        created_vpn_configs: 0,
        reused_vpn_configs: 0,
        created_vpns: 0,
        reused_vpns: 0,
        created_groups: 0,
        reused_groups: journal.groups.len(),
        created_proxies: 0,
        reused_proxies: journal.proxies.len(),
        created_profiles: 0,
        reused_profiles: journal.profiles.len(),
        created_preserved_profiles: 0,
        reused_preserved_profiles: 0,
        journal_path: path,
    })
}

fn verify_credential(
    vault: &impl OsVault,
    entry: &CredentialActivationJournalEntry,
) -> Result<(), Problem> {
    let key = credential_key(&entry.credential_id)?;
    let (value, version) = vault
        .read(&key)
        .map_err(|_| changed("credentialId", &entry.credential_id))?;
    if version.generation() != entry.version
        || buglogin_storage::blob::checksum(value.as_bytes()) != entry.sha256
    {
        return Err(changed("credentialId", &entry.credential_id));
    }
    Ok(())
}

fn verify_entity<T: LocalEntity>(
    repository: &LocalRepository,
    entry: &EntityActivationJournalEntry,
    field: &str,
) -> Result<(), Problem> {
    verify::<T>(
        repository,
        &entry.entity_id,
        entry.revision,
        &entry.sha256,
        field,
    )
}

fn verify<T: LocalEntity>(
    repository: &LocalRepository,
    id: &str,
    revision: u64,
    sha256: &str,
    field: &str,
) -> Result<(), Problem> {
    let versioned = repository.get::<T>(id)?.ok_or_else(|| changed(field, id))?;
    if versioned.revision != revision || hash(&versioned.value)? != sha256 {
        return Err(changed(field, id));
    }
    Ok(())
}

fn write(path: &Path, journal: &ActivationJournal) -> Result<(), Problem> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io_problem("create activation journal", &error))?;
    let bytes =
        serde_json::to_vec_pretty(journal).map_err(|_| invalid("journal", "serialization"))?;
    file.write_all(&bytes)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|error| io_problem("write activation journal", &error))
}

#[allow(clippy::too_many_arguments)]
fn fail_with_cleanup_v2<T, V: OsVault>(
    repository: &LocalRepository,
    root: &Path,
    vault: &mut V,
    original: Problem,
    credentials: &[(String, SecretVersion)],
    groups: &[(String, u64)],
    proxies: &[(String, u64)],
    artifacts: &[PathBuf],
    extensions: &[(String, u64)],
    extension_groups: &[(String, u64)],
    profiles: &[(String, u64)],
) -> Result<T, Problem> {
    cleanup_v2(
        repository,
        root,
        vault,
        credentials,
        groups,
        proxies,
        artifacts,
        extensions,
        extension_groups,
        profiles,
    )
    .map_err(|cleanup_error| {
        problem(
            "migration_activation_cleanup_failed",
            "Migration activation cleanup failed",
            500,
        )
        .with_detail(format!(
            "{}; cleanup: {}",
            original.code, cleanup_error.code
        ))
    })?;
    Err(original)
}

#[allow(clippy::too_many_arguments)]
fn cleanup_v2<V: OsVault>(
    repository: &LocalRepository,
    root: &Path,
    vault: &mut V,
    credentials: &[(String, SecretVersion)],
    groups: &[(String, u64)],
    proxies: &[(String, u64)],
    artifacts: &[PathBuf],
    extensions: &[(String, u64)],
    extension_groups: &[(String, u64)],
    profiles: &[(String, u64)],
) -> Result<(), Problem> {
    for (id, revision) in profiles.iter().rev() {
        repository.quarantine::<Profile>(*revision, id)?;
    }
    for (id, revision) in extension_groups.iter().rev() {
        repository.quarantine::<ExtensionGroup>(*revision, id)?;
    }
    for (id, revision) in extensions.iter().rev() {
        repository.quarantine::<Extension>(*revision, id)?;
    }
    for relative in artifacts.iter().rev() {
        let target = root.join(relative);
        fs::remove_file(&target)
            .map_err(|error| io_problem("remove extension artifact", &error))?;
    }
    for (id, revision) in proxies.iter().rev() {
        repository.quarantine::<Proxy>(*revision, id)?;
    }
    for (id, revision) in groups.iter().rev() {
        repository.quarantine::<Group>(*revision, id)?;
    }
    cleanup_credentials(vault, credentials)
}

#[allow(clippy::too_many_arguments)]
fn fail_with_cleanup_v3<T, V: OsVault>(
    repository: &LocalRepository,
    root: &Path,
    pending_path: &Path,
    vault: &mut V,
    original: Problem,
    credentials: &[(String, SecretVersion)],
    groups: &[(String, u64)],
    proxies: &[(String, u64)],
    artifacts: &[PathBuf],
    extensions: &[(String, u64)],
    extension_groups: &[(String, u64)],
    vpn_configs: &[(String, SecretVersion)],
    vpns: &[(String, u64)],
    profiles: &[(String, u64)],
) -> Result<T, Problem> {
    cleanup_v3(
        repository,
        root,
        vault,
        credentials,
        groups,
        proxies,
        artifacts,
        extensions,
        extension_groups,
        vpn_configs,
        vpns,
        profiles,
    )
    .map_err(|cleanup_error| {
        problem(
            "migration_activation_cleanup_failed",
            "Migration activation cleanup failed",
            500,
        )
        .with_detail(format!(
            "{}; cleanup: {}",
            original.code, cleanup_error.code
        ))
    })?;
    if pending_path.exists() {
        fs::remove_file(pending_path)
            .map_err(|error| io_problem("remove pending activation journal", &error))?;
    }
    Err(original)
}

#[allow(clippy::too_many_arguments)]
fn cleanup_v3<V: OsVault>(
    repository: &LocalRepository,
    root: &Path,
    vault: &mut V,
    credentials: &[(String, SecretVersion)],
    groups: &[(String, u64)],
    proxies: &[(String, u64)],
    artifacts: &[PathBuf],
    extensions: &[(String, u64)],
    extension_groups: &[(String, u64)],
    vpn_configs: &[(String, SecretVersion)],
    vpns: &[(String, u64)],
    profiles: &[(String, u64)],
) -> Result<(), Problem> {
    for (id, revision) in profiles.iter().rev() {
        repository.quarantine::<Profile>(*revision, id)?;
    }
    for (id, revision) in vpns.iter().rev() {
        repository.quarantine::<Vpn>(*revision, id)?;
    }
    for (id, revision) in extension_groups.iter().rev() {
        repository.quarantine::<ExtensionGroup>(*revision, id)?;
    }
    for (id, revision) in extensions.iter().rev() {
        repository.quarantine::<Extension>(*revision, id)?;
    }
    for relative in artifacts.iter().rev() {
        fs::remove_file(root.join(relative))
            .map_err(|error| io_problem("remove extension artifact", &error))?;
    }
    for (id, revision) in proxies.iter().rev() {
        repository.quarantine::<Proxy>(*revision, id)?;
    }
    for (id, revision) in groups.iter().rev() {
        repository.quarantine::<Group>(*revision, id)?;
    }
    cleanup_vpn_configs(vault, vpn_configs)?;
    cleanup_credentials(vault, credentials)
}

fn cleanup_vpn_configs(
    vault: &mut impl OsVault,
    configs: &[(String, SecretVersion)],
) -> Result<(), Problem> {
    for (id, version) in configs.iter().rev() {
        vault
            .remove(&vpn_config_key(id)?, *version)
            .map_err(|error| vault_problem(error, "remove VPN config"))?;
    }
    Ok(())
}

fn cleanup_credentials(
    vault: &mut impl OsVault,
    credentials: &[(String, SecretVersion)],
) -> Result<(), Problem> {
    for (id, version) in credentials.iter().rev() {
        vault
            .remove(&credential_key(id)?, *version)
            .map_err(|error| vault_problem(error, "remove proxy credential"))?;
    }
    Ok(())
}

fn receipt(journal: &ActivationJournal, journal_path: PathBuf) -> ActivationReceipt {
    ActivationReceipt {
        run_id: journal.run_id.clone(),
        created_credentials: journal
            .credentials
            .iter()
            .filter(|entry| entry.created)
            .count(),
        created_vpn_configs: 0,
        reused_vpn_configs: 0,
        created_vpns: 0,
        reused_vpns: 0,
        reused_credentials: journal
            .credentials
            .iter()
            .filter(|entry| !entry.created)
            .count(),
        created_extension_artifacts: journal
            .extension_artifacts
            .iter()
            .filter(|entry| entry.created)
            .count(),
        reused_extension_artifacts: journal
            .extension_artifacts
            .iter()
            .filter(|entry| !entry.created)
            .count(),
        created_extensions: journal
            .extensions
            .iter()
            .filter(|entry| entry.created)
            .count(),
        reused_extensions: journal
            .extensions
            .iter()
            .filter(|entry| !entry.created)
            .count(),
        created_extension_groups: journal
            .extension_groups
            .iter()
            .filter(|entry| entry.created)
            .count(),
        reused_extension_groups: journal
            .extension_groups
            .iter()
            .filter(|entry| !entry.created)
            .count(),
        created_groups: journal.groups.iter().filter(|entry| entry.created).count(),
        reused_groups: journal.groups.iter().filter(|entry| !entry.created).count(),
        created_proxies: journal.proxies.iter().filter(|entry| entry.created).count(),
        reused_proxies: journal
            .proxies
            .iter()
            .filter(|entry| !entry.created)
            .count(),
        created_profiles: journal
            .profiles
            .iter()
            .filter(|entry| entry.created)
            .count(),
        reused_profiles: journal
            .profiles
            .iter()
            .filter(|entry| !entry.created)
            .count(),
        created_preserved_profiles: 0,
        reused_preserved_profiles: 0,
        journal_path,
    }
}

fn ids<T: LocalEntity>(entities: &[T]) -> Vec<String> {
    entities
        .iter()
        .map(|entity| entity.entity_id().to_owned())
        .collect()
}

fn collision(field: &str, id: String) -> Problem {
    problem(
        "migration_activation_collision",
        "Migration activation collides with destination data",
        409,
    )
    .with_field(field, id)
}

fn missing_reference(field: &str, id: &str, profile_id: &str) -> Problem {
    problem(
        "migration_reference_missing",
        "Migration profile references a missing entity",
        422,
    )
    .with_field("profileId", profile_id)
    .with_field(field, id)
}

fn invalid(field: &str, id: &str) -> Problem {
    problem(
        "migration_activation_record_invalid",
        "Migration activation record is invalid",
        422,
    )
    .with_field(field, id)
}

fn changed(field: &str, id: &str) -> Problem {
    problem(
        "migration_activation_entity_changed",
        "Migration activation entity changed",
        409,
    )
    .with_field(field, id)
}

fn hash<T: Serialize>(value: &T) -> Result<String, Problem> {
    serde_json::to_vec(value)
        .map(|bytes| buglogin_storage::blob::checksum(&bytes))
        .map_err(|_| invalid("record", "serialization"))
}

fn vault_problem(error: SecretError, operation: &'static str) -> Problem {
    Problem::new(
        error.code(),
        "Proxy credential vault operation failed",
        if matches!(error, SecretError::NotFound | SecretError::VersionConflict) {
            409
        } else {
            503
        },
        "migration",
    )
    .with_field("operation", operation)
}
