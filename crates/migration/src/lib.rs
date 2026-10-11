#![forbid(unsafe_code)]
pub mod activate;

pub mod cutover;
pub mod encrypted_data;
pub mod encrypted_secrets;
pub mod encrypted_vpn_configs;
pub mod inspect;
pub mod legacy;
pub mod package;
pub(crate) mod preserved_profile_activation;
pub(crate) mod profile_data_activation;
pub mod reconcile;
pub mod rollback;
pub mod signature;
pub mod snapshot;
pub mod transform;
pub mod verify;

use std::{collections::BTreeMap, path::Path};

use activate::{
    ActivationPlan, ActivationReceipt, activate_records_v3_with_vault, plan_records_v3_with_vault,
};
use buglogin_domain::{entities::VpnKind, errors::Problem};
use buglogin_storage::{secrets::OsVault, vault::NativeVault};
pub use buglogin_sync::crypto::ClientKey as MigrationKey;
use encrypted_data::{EncryptedProfileData, decrypt_packaged_profile_data};
use encrypted_secrets::{ProxyCredentialSecret, decrypt_packaged_proxy_credentials};
use encrypted_vpn_configs::{VpnConfigSecret, decrypt_packaged_vpn_configs};
use legacy::{
    ExtensionArtifact, NeutralExtensionGroupRecord, NeutralExtensionRecord, NeutralGroupRecord,
    NeutralProxyRecord, NeutralVpnRecord,
};
use package::{NeutralPackage, NeutralPackageManifest};
use preserved_profile_activation::{plan_preserved_profiles, prepare_preserved_profiles};
use profile_data_activation::{
    DecryptedFiles, decrypt_profile_files, plan_profile_data, prepare_profile_data,
};
use transform::NeutralProfileRecord;

struct PackageRecords {
    package_id: String,
    profiles: Vec<NeutralProfileRecord>,
    groups: Vec<NeutralGroupRecord>,
    proxies: Vec<NeutralProxyRecord>,
    credentials: Vec<ProxyCredentialSecret>,
    extensions: Vec<NeutralExtensionRecord>,
    extension_groups: Vec<NeutralExtensionGroupRecord>,
    extension_artifacts: Vec<ExtensionArtifact>,
    vpns: Vec<NeutralVpnRecord>,
    vpn_configs: Vec<VpnConfigSecret>,
    profile_data: EncryptedProfileData,
}

pub fn plan_package_activation(
    destination_root: &Path,
    package: &NeutralPackage,
    key: &MigrationKey,
) -> Result<ActivationPlan, Problem> {
    plan_package_activation_with_vault(destination_root, package, key, &NativeVault::new())
}

pub fn plan_package_activation_with_vault<V: OsVault>(
    destination_root: &Path,
    package: &NeutralPackage,
    key: &MigrationKey,
    vault: &V,
) -> Result<ActivationPlan, Problem> {
    let records = package_records(package, key)?;
    let profile_files = decrypt_profile_files(&records.profiles, &records.profile_data, key)?;
    let (active_profiles, preserved_profiles) = partition_profiles(&records.profiles);
    let active_files = select_profile_files(&profile_files, &active_profiles);
    let preserved_files = select_profile_files(&profile_files, &preserved_profiles);
    plan_profile_data(destination_root, &active_files)?;
    let preserved_plan =
        plan_preserved_profiles(destination_root, &preserved_profiles, &preserved_files)?;
    let mut plan = plan_records_v3_with_vault(
        destination_root,
        &active_profiles,
        &records.groups,
        &records.proxies,
        &records.credentials,
        &records.extensions,
        &records.extension_groups,
        &records.extension_artifacts,
        &records.vpns,
        &records.vpn_configs,
        vault,
    )?;
    plan.create_preserved_profile_ids = preserved_plan.create_ids;
    plan.reuse_preserved_profile_ids = preserved_plan.reuse_ids;
    Ok(plan)
}

pub fn activate_package(
    destination_root: &Path,
    run_id: &str,
    package: &NeutralPackage,
    key: Option<&MigrationKey>,
) -> Result<ActivationReceipt, Problem> {
    let key = key.ok_or_else(|| {
        Problem::new(
            "migration_encryption_key_required",
            "Migration encryption key is required",
            409,
            "migration",
        )
    })?;
    activate_package_with_vault(
        destination_root,
        run_id,
        package,
        key,
        &mut NativeVault::new(),
    )
}

pub fn activate_package_with_vault<V: OsVault>(
    destination_root: &Path,
    run_id: &str,
    package: &NeutralPackage,
    key: &MigrationKey,
    vault: &mut V,
) -> Result<ActivationReceipt, Problem> {
    let records = package_records(package, key)?;
    let profile_files = decrypt_profile_files(&records.profiles, &records.profile_data, key)?;
    let (active_profiles, preserved_profiles) = partition_profiles(&records.profiles);
    let active_files = select_profile_files(&profile_files, &active_profiles);
    let preserved_files = select_profile_files(&profile_files, &preserved_profiles);
    let prepared = prepare_profile_data(destination_root, run_id, &active_files)?;
    let preserved = match prepare_preserved_profiles(
        destination_root,
        run_id,
        &records.package_id,
        &preserved_profiles,
        &preserved_files,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            prepared.cleanup(destination_root)?;
            return Err(error);
        }
    };
    match activate_records_v3_with_vault(
        destination_root,
        run_id,
        &active_profiles,
        &records.groups,
        &records.proxies,
        &records.credentials,
        &records.extensions,
        &records.extension_groups,
        &records.extension_artifacts,
        &records.vpns,
        &records.vpn_configs,
        vault,
    ) {
        Ok(mut receipt) => {
            if let Err(error) = preserved.commit() {
                if let Err(rollback_error) = crate::rollback::rollback_activation_with_vault(
                    destination_root,
                    run_id,
                    crate::rollback::confirm_rollback("ROLLBACK_MIGRATION_RUN")?,
                    vault,
                ) {
                    return Err(rollback_error);
                }
                prepared.cleanup(destination_root)?;
                preserved.cleanup()?;
                return Err(error);
            }
            receipt.created_preserved_profiles = preserved.created_count();
            receipt.reused_preserved_profiles = preserved.reused_count();
            if let Err(error) = prepared.finalize() {
                crate::rollback::rollback_activation_with_vault(
                    destination_root,
                    run_id,
                    crate::rollback::confirm_rollback("ROLLBACK_MIGRATION_RUN")?,
                    vault,
                )?;
                preserved.cleanup()?;
                return Err(error);
            }
            if let Err(error) = preserved.finalize() {
                crate::rollback::rollback_activation_with_vault(
                    destination_root,
                    run_id,
                    crate::rollback::confirm_rollback("ROLLBACK_MIGRATION_RUN")?,
                    vault,
                )?;
                return Err(error);
            }
            Ok(receipt)
        }
        Err(error) => {
            prepared.cleanup(destination_root)?;
            preserved.cleanup()?;
            Err(error)
        }
    }
}

fn package_records(
    package: &NeutralPackage,
    key: &MigrationKey,
) -> Result<PackageRecords, Problem> {
    let manifest: NeutralPackageManifest =
        serde_json::from_slice(package.file("manifest.json").ok_or_else(|| {
            Problem::new(
                "migration_manifest_missing",
                "Migration manifest is missing",
                500,
                "migration",
            )
        })?)
        .map_err(|_| {
            Problem::new(
                "migration_manifest_invalid",
                "Migration manifest is invalid",
                500,
                "migration",
            )
        })?;
    if !manifest.security.encrypted {
        return Err(Problem::new(
            "migration_encrypted_package_required",
            "Profile import requires an encrypted migration package",
            409,
            "migration",
        ));
    }
    let profile_data = decrypt_packaged_profile_data(package, key)?;
    let credentials = decrypt_packaged_proxy_credentials(package, key)?;
    let vpn_configs = decrypt_packaged_vpn_configs(package, key)?;
    let profiles = parse_json_lines(
        package.file("entities/profiles.jsonl").ok_or_else(|| {
            Problem::new(
                "migration_profiles_missing",
                "Migration profile records are missing",
                422,
                "migration",
            )
        })?,
        "migration_profile_invalid",
        "Migration profile record is invalid",
    )?;
    let groups = package
        .file("entities/groups.jsonl")
        .map(|bytes| {
            parse_json_lines(
                bytes,
                "migration_group_invalid",
                "Migration group record is invalid",
            )
        })
        .transpose()?
        .unwrap_or_default();
    let proxies = package
        .file("entities/proxies.jsonl")
        .map(|bytes| {
            parse_json_lines(
                bytes,
                "migration_proxy_invalid",
                "Migration proxy record is invalid",
            )
        })
        .transpose()?
        .unwrap_or_default();
    let extensions = package
        .file("entities/extensions.jsonl")
        .map(|bytes| {
            parse_json_lines(
                bytes,
                "migration_extension_invalid",
                "Migration extension record is invalid",
            )
        })
        .transpose()?
        .unwrap_or_default();
    let extension_groups = package
        .file("entities/extension-groups.jsonl")
        .map(|bytes| {
            parse_json_lines(
                bytes,
                "migration_extension_group_invalid",
                "Migration extension group record is invalid",
            )
        })
        .transpose()?
        .unwrap_or_default();
    let extension_artifacts = package
        .files()
        .iter()
        .filter_map(|(path, bytes)| {
            path.strip_prefix("artifacts/extensions/")
                .map(|relative| ExtensionArtifact {
                    artifact_ref: relative.split('/').next().unwrap_or_default().to_owned(),
                    package_path: path.clone(),
                    bytes: bytes.clone(),
                })
        })
        .collect();
    let vpns = package
        .file("entities/vpns.jsonl")
        .map(|bytes| {
            parse_json_lines(
                bytes,
                "migration_vpn_invalid",
                "Migration VPN record is invalid",
            )
        })
        .transpose()?
        .unwrap_or_default();
    validate_vpn_package_records(&vpns, &vpn_configs)?;
    Ok(PackageRecords {
        package_id: manifest.package_id,
        profiles,
        groups,
        proxies,
        credentials,
        extensions,
        extension_groups,
        extension_artifacts,
        vpns,
        vpn_configs,
        profile_data,
    })
}

fn partition_profiles(
    profiles: &[NeutralProfileRecord],
) -> (Vec<NeutralProfileRecord>, Vec<NeutralProfileRecord>) {
    profiles
        .iter()
        .cloned()
        .partition(|profile| matches!(profile.data.migration_class.as_str(), "direct" | "adapter"))
}

fn select_profile_files(
    files: &DecryptedFiles,
    profiles: &[NeutralProfileRecord],
) -> DecryptedFiles {
    profiles
        .iter()
        .filter_map(|profile| {
            files
                .get(&profile.id)
                .cloned()
                .map(|profile_files| (profile.id.clone(), profile_files))
        })
        .collect()
}

fn validate_vpn_package_records(
    vpns: &[NeutralVpnRecord],
    configs: &[VpnConfigSecret],
) -> Result<(), Problem> {
    let expected = vpns
        .iter()
        .map(|vpn| {
            let kind = match vpn.data.kind.as_str() {
                "wireguard" => VpnKind::Wireguard,
                "openvpn" => VpnKind::Openvpn,
                _ => return Err(vpn_package_mismatch()),
            };
            Ok((vpn.data.config_ref.as_str(), (vpn.id.as_str(), kind)))
        })
        .collect::<Result<BTreeMap<_, _>, Problem>>()?;
    let actual = configs
        .iter()
        .map(|config| {
            (
                config.config_ref.as_str(),
                (config.vpn_id.as_str(), config.kind.clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    if expected.len() != vpns.len() || actual.len() != configs.len() || expected != actual {
        return Err(vpn_package_mismatch());
    }
    Ok(())
}

fn vpn_package_mismatch() -> Problem {
    Problem::new(
        "migration_vpn_config_set_mismatch",
        "VPN records do not match encrypted VPN configs",
        422,
        "migration",
    )
}

fn parse_json_lines<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    code: &'static str,
    title: &'static str,
) -> Result<Vec<T>, Problem> {
    bytes
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            serde_json::from_slice(line).map_err(|_| Problem::new(code, title, 422, "migration"))
        })
        .collect()
}
