use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;

use buglogin_domain::errors::Problem;
use buglogin_sync::crypto::ClientKey;
use postgres::NoTls;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyStoragePolicy {
    pub schema: String,
    pub workspace_max_storage_bytes: BTreeMap<String, u64>,
}

#[derive(Clone, Debug)]
pub struct PostgresControlExport {
    pub canonical_json: Vec<u8>,
    pub database: String,
    pub workspace_count: usize,
    pub membership_count: usize,
    pub source_share_grant_count: usize,
    pub share_grant_count: usize,
    pub pending_share_count: usize,
    pub pending_invite_count: usize,
    pub group_count: usize,
    pub profile_count: usize,
    pub profile_group_assignment_count: usize,
    pub legacy_password_account_count: usize,
    pub google_only_account_count: usize,
}

struct PendingInvite {
    id: String,
    workspace_id: String,
    email_hash: String,
    role: String,
    created_at: String,
    expires_at: String,
    created_by: String,
}

pub fn export_read_only_postgres_control_plane(
    database_url: &str,
    expected_database: &str,
    source_instance_id: &str,
    policy: &LegacyStoragePolicy,
    migration_key: &ClientKey,
) -> Result<PostgresControlExport, Problem> {
    if expected_database.trim().is_empty()
        || source_instance_id.trim().is_empty()
        || policy.schema != "buglogin.legacy-storage-policy/1"
    {
        return Err(invalid("source identity or export policy"));
    }
    validate_local_database_url(database_url, expected_database)?;

    let mut config =
        postgres::Config::from_str(database_url).map_err(|_| invalid("legacy PostgreSQL URL"))?;
    config
        .connect_timeout(Duration::from_secs(5))
        .application_name("buglogin-migration-export");
    let mut client = config
        .connect(NoTls)
        .map_err(|_| unavailable("legacy PostgreSQL connection failed"))?;
    let mut transaction = client
        .build_transaction()
        .isolation_level(postgres::IsolationLevel::RepeatableRead)
        .read_only(true)
        .start()
        .map_err(|_| unavailable("read-only export transaction could not start"))?;
    transaction
        .batch_execute("SET LOCAL statement_timeout = '30s'; SET LOCAL lock_timeout = '5s'")
        .map_err(|_| unavailable("read-only export limits could not be set"))?;

    let database: String = transaction
        .query_one("SELECT current_database()", &[])
        .and_then(|row| row.try_get(0))
        .map_err(|_| unavailable("source database identity could not be verified"))?;
    if database != expected_database {
        return Err(invalid("source database does not match requested identity"));
    }
    let transaction_read_only: String = transaction
        .query_one("SHOW transaction_read_only", &[])
        .and_then(|row| row.try_get(0))
        .map_err(|_| unavailable("source read-only state could not be verified"))?;
    if transaction_read_only != "on" {
        return Err(invalid("source transaction is not read only"));
    }

    let workspaces = transaction
        .query(
            "SELECT w.id, w.name, w.mode, w.created_by, \
                    to_char(w.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'), \
                    s.profile_limit, s.member_limit, e.state, \
                    (SELECT count(*) FROM workspace_subscriptions s2 WHERE s2.workspace_id=w.id) \
             FROM workspaces w \
             JOIN workspace_subscriptions s ON s.workspace_id=w.id \
             JOIN entitlements e ON e.workspace_id=w.id \
             ORDER BY w.id",
            &[],
        )
        .map_err(|_| unavailable("workspace source rows could not be read"))?;
    let mut workspace_ids = BTreeSet::new();
    let mut organization_values = Vec::with_capacity(workspaces.len());
    let mut workspace_values = Vec::with_capacity(workspaces.len());
    for row in &workspaces {
        let id: String = row.get(0);
        if !workspace_ids.insert(id.clone()) || row.get::<_, i64>(8) != 1 {
            return Err(invalid("workspace subscription cardinality"));
        }
        let storage = policy
            .workspace_max_storage_bytes
            .get(&id)
            .copied()
            .filter(|bytes| *bytes > 0)
            .ok_or_else(|| invalid("workspace storage policy is missing or zero"))?;
        let name: String = row.get(1);
        let mode: String = row.get(2);
        let created_by: String = row.get(3);
        let created_at: String = row.get(4);
        let profile_limit = positive_limit(row.get(5), "workspace profile limit")?;
        let member_limit = positive_limit(row.get(6), "workspace member limit")?;
        let entitlement_state: String = row.get(7);
        let organization_id = format!("legacy-workspace-org:{id}");
        organization_values.push(json!({
            "id": organization_id,
            "name": format!("{name} Organization"),
            "slug": format!("legacy-{}", short_hash(&id)),
            "status": "active",
            "createdBy": created_by,
            "createdAt": created_at,
            "updatedAt": created_at
        }));
        workspace_values.push(json!({
            "id": id,
            "organizationId": organization_id,
            "name": name,
            "mode": mode,
            "createdAt": created_at,
            "createdBy": created_by,
            "profileLimit": profile_limit,
            "memberLimit": member_limit,
            "maxStorageBytes": storage,
            "entitlementState": entitlement_state,
            "localEntitlementState": entitlement_state
        }));
    }
    if workspace_ids != policy.workspace_max_storage_bytes.keys().cloned().collect() {
        return Err(invalid("workspace storage policy coverage"));
    }

    let memberships = transaction
        .query(
            "SELECT m.workspace_id, m.user_id, u.email, m.role, m.resource_access_mode, \
                    COALESCE(c.platform_role, CASE WHEN p.email IS NOT NULL THEN 'platform_admin' END), \
                    c.auth_provider, \
                    to_char(m.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') \
             FROM workspace_memberships m \
             JOIN users u ON u.id=m.user_id \
             LEFT JOIN user_credentials c ON c.user_id=m.user_id \
             LEFT JOIN platform_admin_emails p ON lower(p.email)=lower(u.email) \
             ORDER BY m.workspace_id, lower(u.email), m.user_id",
            &[],
        )
        .map_err(|_| unavailable("workspace membership source rows could not be read"))?;
    let mut member_keys = BTreeSet::new();
    let mut platform_member_keys = BTreeSet::new();
    let mut seen_membership_keys = BTreeSet::new();
    let mut membership_values = Vec::with_capacity(memberships.len());
    for row in &memberships {
        let workspace_id: String = row.get(0);
        let user_id: String = row.get(1);
        let email: String = row.get(2);
        let email_key = email.trim().to_ascii_lowercase();
        if !workspace_ids.contains(&workspace_id)
            || !seen_membership_keys.insert((workspace_id.clone(), email_key.clone()))
        {
            return Err(invalid(
                "membership identity is duplicate or has no workspace",
            ));
        }
        let platform_role: Option<String> = row.get(5);
        if platform_role.as_deref() == Some("platform_admin") {
            platform_member_keys.insert((workspace_id.clone(), email_key));
        } else {
            member_keys.insert((workspace_id.clone(), email_key));
        }
        membership_values.push(json!({
            "workspaceId": workspace_id,
            "userId": user_id,
            "email": email,
            "legacyAuthProvider": row.get::<_, Option<String>>(6),
            "role": row.get::<_, String>(3),
            "resourceAccessMode": row.get::<_, String>(4),
            "customCapabilities": null,
            "accountState": "active",
            "platformRole": platform_role,
            "accessSource": if platform_role.as_deref() == Some("platform_admin") { "platform" } else { "direct" },
            "createdAt": row.get::<_, String>(7)
        }));
    }

    let password_rows = transaction
        .query(
            "SELECT user_id, password_salt, password_hash, auth_provider \
             FROM user_credentials ORDER BY user_id",
            &[],
        )
        .map_err(|_| unavailable("account credential source rows could not be read"))?;
    let users = transaction
        .query("SELECT id FROM users ORDER BY id", &[])
        .map_err(|_| unavailable("account identity source rows could not be read"))?;
    let source_user_ids = users
        .iter()
        .map(|row| row.get::<_, String>(0))
        .collect::<BTreeSet<_>>();
    let mut legacy_credentials = Vec::new();
    let mut credential_users = BTreeSet::new();
    let mut google_only_account_count = 0;
    for row in &password_rows {
        let user_id: String = row.get(0);
        let provider: String = row.get(3);
        if !credential_users.insert(user_id.clone()) {
            return Err(invalid("duplicate account credential"));
        }
        if !matches!(provider.as_str(), "password" | "password_google" | "google") {
            return Err(invalid("unsupported account credential provider"));
        }
        if provider != "google" {
            legacy_credentials.push(super::LegacyScryptCredential {
                source_user_id: user_id,
                salt: row.get(1),
                hash: row.get(2),
            });
        } else {
            google_only_account_count += 1;
        }
    }
    if credential_users != source_user_ids {
        return Err(invalid("account credential export coverage"));
    }
    let encrypted_credentials = if legacy_credentials.is_empty() {
        None
    } else {
        Some(
            crate::legacy_account_credentials::encrypt_source_credentials(
                &legacy_credentials,
                migration_key,
            )?,
        )
    };
    let legacy_password_account_count = legacy_credentials.len();

    let group_rows = transaction
        .query(
            "SELECT id, workspace_id, name, color, sync_enabled, \
                    CASE WHEN last_sync IS NULL THEN NULL ELSE to_char(last_sync AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') END, \
                    created_by, to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'), \
                    to_char(updated_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') \
             FROM workspace_groups ORDER BY workspace_id, id",
            &[],
        )
        .map_err(|_| unavailable("workspace group source rows could not be read"))?;
    let mut group_ids = BTreeMap::<String, String>::new();
    let mut group_values = Vec::with_capacity(group_rows.len());
    for row in &group_rows {
        let id: String = row.get(0);
        let workspace_id: String = row.get(1);
        if !workspace_ids.contains(&workspace_id)
            || group_ids.insert(id.clone(), workspace_id.clone()).is_some()
        {
            return Err(invalid("workspace group identity is duplicate or dangling"));
        }
        group_values.push(json!({
            "id": id,
            "workspaceId": workspace_id,
            "name": row.get::<_, String>(2),
            "color": row.get::<_, Option<String>>(3),
            "syncEnabled": row.get::<_, bool>(4),
            "lastSync": row.get::<_, Option<String>>(5),
            "createdBy": row.get::<_, String>(6),
            "createdAt": row.get::<_, String>(7),
            "updatedAt": row.get::<_, String>(8)
        }));
    }

    let profile_rows = transaction
        .query(
            "SELECT id, workspace_id, name, browser_str, version, release_type, proxy_id, vpn_id, group_id, extension_group_id, ephemeral, \
                    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'), \
                    COALESCE(tags, ARRAY[]::text[]), note \
             FROM workspace_profiles ORDER BY workspace_id, id",
            &[],
        )
        .map_err(|_| unavailable("hosted profile source rows could not be read"))?;
    let mut profile_ids = BTreeSet::new();
    let mut profile_workspaces = BTreeMap::new();
    let mut profile_values = Vec::with_capacity(profile_rows.len());
    for row in &profile_rows {
        let id: String = row.get(0);
        let workspace_id: String = row.get(1);
        if !profile_ids.insert(id.clone()) || !workspace_ids.contains(&workspace_id) {
            return Err(invalid("hosted profile identity or workspace"));
        }
        profile_workspaces.insert(id.clone(), workspace_id.clone());
        let created_at: String = row.get(11);
        profile_values.push(json!({
            "id": id,
            "workspaceId": workspace_id,
            "name": row.get::<_, String>(2),
            "runtime": row.get::<_, String>(3),
            "runtimeVersion": row.get::<_, Option<String>>(4),
            "releaseType": row.get::<_, String>(5),
            "proxyId": row.get::<_, Option<String>>(6),
            "vpnId": row.get::<_, Option<String>>(7),
            "groupId": row.get::<_, Option<String>>(8),
            "extensionGroupId": row.get::<_, Option<String>>(9),
            "ephemeral": row.get::<_, bool>(10),
            "createdAt": created_at,
            "updatedAt": created_at,
            "tags": row.get::<_, Vec<String>>(12),
            "note": row.get::<_, Option<String>>(13)
        }));
    }

    let assignment_rows = transaction
        .query(
            "SELECT id, workspace_id, group_id FROM workspace_profiles WHERE group_id IS NOT NULL ORDER BY workspace_id, id",
            &[],
        )
        .map_err(|_| unavailable("profile group assignments could not be read"))?;
    let mut assignment_values = Vec::with_capacity(assignment_rows.len());
    for row in &assignment_rows {
        let workspace_id: String = row.get(1);
        let group_id: String = row.get(2);
        if !workspace_ids.contains(&workspace_id) || group_ids.get(&group_id) != Some(&workspace_id)
        {
            return Err(invalid(
                "profile group assignment is dangling or cross-workspace",
            ));
        }
        assignment_values.push(json!({
            "profileId": row.get::<_, String>(0),
            "workspaceId": workspace_id,
            "groupId": group_id
        }));
    }

    let invite_rows = transaction
        .query(
            "SELECT id, workspace_id, email, role, \
                    to_char(created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'), \
                    to_char(expires_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'), created_by \
             FROM invites WHERE consumed_at IS NULL ORDER BY workspace_id, lower(email), id",
            &[],
        )
        .map_err(|_| unavailable("pending invite source rows could not be read"))?;
    let mut invites_by_identity = BTreeMap::<(String, String), Vec<PendingInvite>>::new();
    let mut pending_invite_values = Vec::with_capacity(invite_rows.len());
    for row in &invite_rows {
        let workspace_id: String = row.get(1);
        let email: String = row.get(2);
        let identity_sha256 = identity_hash(&email);
        let id: String = row.get(0);
        let role: String = row.get(3);
        let created_at: String = row.get(4);
        let expires_at: String = row.get(5);
        let created_by: String = row.get(6);
        pending_invite_values.push(json!({
            "id": id,
            "workspaceId": workspace_id,
            "identitySha256": identity_sha256,
            "role": role,
            "createdAt": created_at,
            "expiresAt": expires_at,
            "createdBy": created_by
        }));
        let invite = PendingInvite {
            id,
            workspace_id: workspace_id.clone(),
            email_hash: identity_sha256,
            role,
            created_at,
            expires_at,
            created_by,
        };
        invites_by_identity
            .entry((workspace_id, email.trim().to_ascii_lowercase()))
            .or_default()
            .push(invite);
    }

    let grant_rows = transaction
        .query(
            "SELECT g.id, g.workspace_id, g.resource_type, g.resource_id, \
                    COALESCE(p.name, wg.name), g.recipient_email, g.access_mode, \
                    to_char(g.created_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"'), g.created_by, \
                    CASE WHEN g.revoked_at IS NULL THEN NULL ELSE to_char(g.revoked_at AT TIME ZONE 'UTC', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') END \
             FROM share_grants g \
             LEFT JOIN workspace_profiles p ON g.resource_type='profile' AND p.id=g.resource_id AND p.workspace_id=g.workspace_id \
             LEFT JOIN workspace_groups wg ON g.resource_type='group' AND wg.id=g.resource_id AND wg.workspace_id=g.workspace_id \
             ORDER BY g.workspace_id, g.resource_type, g.resource_id, g.id",
            &[],
        )
        .map_err(|_| unavailable("share grant source rows could not be read"))?;
    let mut share_values = Vec::new();
    let mut pending_share_values = Vec::new();
    for row in &grant_rows {
        let id: String = row.get(0);
        let workspace_id: String = row.get(1);
        let resource_type: String = row.get(2);
        let resource_id: String = row.get(3);
        let resource_name: Option<String> = row.get(4);
        let recipient_email: String = row.get(5);
        let access_mode: String = row.get(6);
        let created_at: String = row.get(7);
        let created_by: String = row.get(8);
        let revoked_at: Option<String> = row.get(9);
        let email_key = recipient_email.trim().to_ascii_lowercase();
        if resource_type == "profile"
            && revoked_at.is_none()
            && profile_workspaces.get(&resource_id) != Some(&workspace_id)
        {
            pending_share_values.push(json!({
                "id": id,
                "workspaceId": workspace_id,
                "resourceType": resource_type,
                "resourceId": resource_id,
                "resourceName": resource_name,
                "recipientIdentitySha256": identity_hash(&recipient_email),
                "accessMode": access_mode,
                "createdAt": created_at,
                "createdBy": created_by,
                "revokedAt": null,
                "disposition": "orphaned_profile",
                "inviteId": null,
                "inviteWorkspaceId": null,
                "inviteIdentitySha256": null,
                "inviteConsumedAt": null,
                "inviteRole": null,
                "inviteCreatedAt": null,
                "inviteExpiresAt": null,
                "inviteCreatedBy": null
            }));
            continue;
        }
        if revoked_at.is_none()
            && platform_member_keys.contains(&(workspace_id.clone(), email_key.clone()))
        {
            pending_share_values.push(json!({
                "id": id,
                "workspaceId": workspace_id,
                "resourceType": resource_type,
                "resourceId": resource_id,
                "resourceName": resource_name,
                "recipientIdentitySha256": identity_hash(&recipient_email),
                "accessMode": access_mode,
                "createdAt": created_at,
                "createdBy": created_by,
                "revokedAt": null,
                "disposition": "platform_recipient",
                "inviteId": null,
                "inviteWorkspaceId": null,
                "inviteIdentitySha256": null,
                "inviteConsumedAt": null,
                "inviteRole": null,
                "inviteCreatedAt": null,
                "inviteExpiresAt": null,
                "inviteCreatedBy": null
            }));
            continue;
        }
        let grant = json!({
            "id": id,
            "workspaceId": workspace_id,
            "resourceType": resource_type,
            "resourceId": resource_id,
            "resourceName": resource_name,
            "recipientEmail": recipient_email,
            "accessMode": access_mode,
            "createdAt": created_at,
            "createdBy": created_by,
            "revokedAt": revoked_at
        });
        if member_keys.contains(&(workspace_id.clone(), email_key.clone())) {
            share_values.push(grant);
            continue;
        }
        let identity = identity_hash(&recipient_email);
        if revoked_at.is_some() {
            pending_share_values.push(json!({
                "id": id,
                "workspaceId": workspace_id,
                "resourceType": resource_type,
                "resourceId": resource_id,
                "resourceName": resource_name,
                "recipientIdentitySha256": identity,
                "accessMode": access_mode,
                "createdAt": created_at,
                "createdBy": created_by,
                "revokedAt": revoked_at,
                "disposition": "revoked_without_member",
                "inviteId": null,
                "inviteWorkspaceId": null,
                "inviteIdentitySha256": null,
                "inviteConsumedAt": null,
                "inviteRole": null,
                "inviteCreatedAt": null,
                "inviteExpiresAt": null,
                "inviteCreatedBy": null
            }));
            continue;
        }
        let matches = invites_by_identity
            .get(&(workspace_id.clone(), email_key))
            .map(Vec::as_slice)
            .unwrap_or_default();
        if matches.len() != 1 {
            return Err(invalid(
                "active share recipient is neither a workspace member nor one unique pending invite",
            ));
        }
        let invite = &matches[0];
        if invite.workspace_id != workspace_id || invite.email_hash != identity {
            return Err(invalid(
                "pending invite identity does not match share recipient",
            ));
        }
        pending_share_values.push(json!({
            "id": id,
            "workspaceId": workspace_id,
            "resourceType": resource_type,
            "resourceId": resource_id,
            "resourceName": resource_name,
            "recipientIdentitySha256": identity,
            "accessMode": access_mode,
            "createdAt": created_at,
            "createdBy": created_by,
            "revokedAt": null,
            "disposition": "pending_invite",
            "inviteId": invite.id,
            "inviteWorkspaceId": invite.workspace_id,
            "inviteIdentitySha256": invite.email_hash,
            "inviteConsumedAt": null,
            "inviteRole": invite.role,
            "inviteCreatedAt": invite.created_at,
            "inviteExpiresAt": invite.expires_at,
            "inviteCreatedBy": invite.created_by
        }));
    }
    if share_values.len() + pending_share_values.len() != grant_rows.len() {
        return Err(invalid("share grant export coverage"));
    }

    let workspace_count = workspace_values.len();
    let membership_count = memberships.len();
    let share_grant_count = share_values.len();
    let pending_share_count = pending_share_values.len();
    let pending_invite_count = pending_invite_values.len();
    let group_count = group_rows.len();
    let profile_count = profile_rows.len();
    let profile_group_assignment_count = assignment_rows.len();
    let value = json!({
        "organizations": organization_values,
        "workspaces": workspace_values,
        "memberships": membership_values,
        "shareGrants": share_values,
        "profiles": profile_values,
        "groups": group_values,
        "profileGroupAssignments": assignment_values,
        "pendingShareGrants": pending_share_values,
        "pendingInvites": pending_invite_values,
        "legacyCredentials": encrypted_credentials
    });
    let source_json =
        serde_json::to_vec(&value).map_err(|_| invalid("control export serialization"))?;
    let canonical_json =
        super::canonicalize_legacy_control_plane_json(&source_json, source_instance_id)?;
    transaction
        .commit()
        .map_err(|_| unavailable("read-only export transaction could not finish"))?;
    Ok(PostgresControlExport {
        canonical_json,
        database,
        workspace_count,
        membership_count,
        source_share_grant_count: grant_rows.len(),
        share_grant_count,
        pending_share_count,
        pending_invite_count,
        group_count,
        profile_count,
        profile_group_assignment_count,
        legacy_password_account_count,
        google_only_account_count,
    })
}

fn validate_local_database_url(database_url: &str, expected_database: &str) -> Result<(), Problem> {
    let url = Url::parse(database_url).map_err(|_| invalid("legacy PostgreSQL URL"))?;
    let host = url
        .host_str()
        .ok_or_else(|| invalid("legacy PostgreSQL host"))?;
    let loopback = host.eq_ignore_ascii_case("localhost")
        || IpAddr::from_str(host).is_ok_and(|address| address.is_loopback());
    let database = url.path().trim_start_matches('/');
    let ssl_mode_is_local = url
        .query_pairs()
        .filter(|(key, _)| key.eq_ignore_ascii_case("sslmode"))
        .all(|(_, value)| value == "disable");
    if !loopback
        || database != expected_database
        || url.username().is_empty()
        || url.password().is_none_or(str::is_empty)
        || !ssl_mode_is_local
    {
        return Err(invalid(
            "legacy PostgreSQL source must be authenticated and reached through a loopback tunnel",
        ));
    }
    Ok(())
}

fn positive_limit(value: i32, field: &'static str) -> Result<u32, Problem> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| invalid(field))
}

fn identity_hash(email: &str) -> String {
    let normalized = email.trim().to_ascii_lowercase();
    Sha256::digest(normalized.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn short_hash(value: &str) -> String {
    identity_hash(value)[..24].to_owned()
}

fn invalid(detail: &str) -> Problem {
    Problem::new(
        "migration_legacy_export_invalid",
        "Legacy control-plane export is invalid",
        422,
        "migration-postgres-export",
    )
    .with_field("reason", detail)
}

fn unavailable(detail: &str) -> Problem {
    Problem::new(
        "migration_legacy_export_unavailable",
        "Legacy control-plane export could not be completed",
        503,
        "migration-postgres-export",
    )
    .with_field("reason", detail)
}

#[cfg(test)]
mod tests {
    use super::validate_local_database_url;

    #[test]
    fn accepts_the_named_database_only_through_an_authenticated_loopback_tunnel() {
        assert!(
            validate_local_database_url(
                "postgres://exporter:secret@127.0.0.1:5432/buglogin",
                "buglogin"
            )
            .is_ok()
        );
        assert!(
            validate_local_database_url(
                "postgres://exporter:secret@db.example.com:5432/buglogin",
                "buglogin"
            )
            .is_err()
        );
        assert!(
            validate_local_database_url(
                "postgres://exporter:secret@127.0.0.1:5432/other",
                "buglogin"
            )
            .is_err()
        );
        assert!(
            validate_local_database_url("postgres://127.0.0.1:5432/buglogin", "buglogin").is_err()
        );
    }
}
