use std::collections::{BTreeMap, BTreeSet, HashMap};

use buglogin_domain::{
    errors::Problem,
    identity::{SessionRecord, SessionRefreshReplay},
    ids::{AccountId, DeviceId},
};
use buglogin_storage::control::{
    ControlAuditEntry, ControlCommandResult, ControlDelete, ControlPut, ControlRecordKey,
    ExecuteControlCommand, HttpResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{account_persistence::AccountPersistence, control_persistence::ControlPersistence};

const TENANT: &str = "identity";
const RECORD_TYPE: &str = "session";
const FAMILY_RECORD_TYPE: &str = "session_family";
const REFRESH_RECORD_TYPE: &str = "device_refresh";
const DEVICE_HEAD_RECORD_TYPE: &str = "device_session_head";
const SESSION_RETENTION_SECONDS: u64 = 30 * 24 * 60 * 60;
const SESSION_CLEANUP_BATCH: usize = 128;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SessionFamilyRecord {
    account_id: AccountId,
    revoked_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expires_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceSessionHead {
    account_id: AccountId,
    family_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRefreshRecord {
    pub account_id: AccountId,
    pub token_hash: [u8; 32],
    pub access_token_hash: [u8; 32],
    pub device_id: String,
    pub family_id: String,
    pub expires_at: u64,
    pub consumed_at: Option<u64>,
    pub credential_version: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_replay: Option<SessionRefreshReplay>,
}

#[derive(Clone)]
pub struct SessionPersistence {
    store: ControlPersistence,
}

impl SessionPersistence {
    pub fn new(store: ControlPersistence) -> Self {
        Self { store }
    }

    pub fn list(&self) -> Result<Vec<(SessionRecord, u64)>, Problem> {
        self.store
            .list_versioned::<SessionRecord>(TENANT, RECORD_TYPE)?
            .into_iter()
            .map(|(record_id, record, revision)| {
                if record_id != session_id(&record.token_hash) {
                    return Err(corrupt());
                }
                Ok((record, revision))
            })
            .collect()
    }

    pub fn list_for_account(
        &self,
        account_id: &AccountId,
    ) -> Result<Vec<(SessionRecord, u64)>, Problem> {
        self.store
            .list_versioned_for_account::<SessionRecord>(TENANT, RECORD_TYPE, account_id.as_str())?
            .into_iter()
            .map(|(record_id, record, revision)| {
                if record_id != session_id(&record.token_hash) || record.account_id != *account_id {
                    return Err(corrupt());
                }
                Ok((record, revision))
            })
            .collect()
    }

    pub fn purge_expired(&self, now: u64) -> Result<usize, Problem> {
        let cutoff = now.saturating_sub(SESSION_RETENTION_SECONDS);
        for _ in 0..2 {
            let sessions = self.list()?;
            let refreshes = self
                .store
                .list_versioned::<DeviceRefreshRecord>(TENANT, REFRESH_RECORD_TYPE)?;
            let families = self
                .store
                .list_versioned::<SessionFamilyRecord>(TENANT, FAMILY_RECORD_TYPE)?;

            let mut family_expirations = BTreeMap::<String, u64>::new();
            let mut family_children = BTreeMap::<String, Vec<(String, String)>>::new();
            for (session, _) in &sessions {
                if let Some(family_id) = session.family_id.as_deref() {
                    if !is_family_id(family_id) {
                        return Err(corrupt());
                    }
                    family_expirations
                        .entry(family_id.to_owned())
                        .and_modify(|expires| *expires = (*expires).max(session.expires_at))
                        .or_insert(session.expires_at);
                    family_children
                        .entry(family_id.to_owned())
                        .or_default()
                        .push((RECORD_TYPE.into(), session_id(&session.token_hash)));
                }
            }
            for (record_id, refresh, _) in &refreshes {
                if record_id != &session_id(&refresh.token_hash)
                    || !is_family_id(&refresh.family_id)
                {
                    return Err(corrupt());
                }
                family_expirations
                    .entry(refresh.family_id.clone())
                    .and_modify(|expires| *expires = (*expires).max(refresh.expires_at))
                    .or_insert(refresh.expires_at);
                family_children
                    .entry(refresh.family_id.clone())
                    .or_default()
                    .push((REFRESH_RECORD_TYPE.into(), record_id.clone()));
            }
            for (family_id, family, _) in &families {
                if !is_family_id(family_id) {
                    return Err(corrupt());
                }
                if let Some(expires_at) = family.expires_at {
                    family_expirations
                        .entry(family_id.clone())
                        .and_modify(|expires| *expires = (*expires).max(expires_at))
                        .or_insert(expires_at);
                } else if let Some(revoked_at) = family.revoked_at {
                    family_expirations
                        .entry(family_id.clone())
                        .or_insert(revoked_at);
                }
            }

            let expired_families = family_expirations
                .into_iter()
                .filter_map(|(family_id, expires_at)| (expires_at <= cutoff).then_some(family_id))
                .collect::<BTreeSet<_>>();
            let mut child_deletes = Vec::new();
            for (session, revision) in &sessions {
                let expired = session.expires_at <= cutoff
                    && session
                        .family_id
                        .as_ref()
                        .is_none_or(|family_id| expired_families.contains(family_id));
                if expired && child_deletes.len() < SESSION_CLEANUP_BATCH {
                    child_deletes.push((
                        RECORD_TYPE,
                        session_id(&session.token_hash),
                        *revision,
                        session.family_id.clone(),
                    ));
                }
            }
            for (record_id, refresh, revision) in &refreshes {
                if refresh.expires_at <= cutoff
                    && expired_families.contains(&refresh.family_id)
                    && child_deletes.len() < SESSION_CLEANUP_BATCH
                {
                    child_deletes.push((
                        REFRESH_RECORD_TYPE,
                        record_id.clone(),
                        *revision,
                        Some(refresh.family_id.clone()),
                    ));
                }
            }
            let selected_children = child_deletes
                .iter()
                .map(|(record_type, record_id, _, _)| {
                    ((*record_type).to_owned(), record_id.clone())
                })
                .collect::<BTreeSet<_>>();
            let mut deletes = child_deletes
                .iter()
                .map(|(record_type, record_id, revision, _)| ControlDelete {
                    tenant_id: TENANT.into(),
                    record_type: (*record_type).into(),
                    record_id: record_id.clone(),
                    expected_revision: Some(*revision),
                })
                .collect::<Vec<_>>();
            let mut deleted_ids = child_deletes
                .iter()
                .map(|(record_type, record_id, _, _)| format!("{record_type}:{record_id}"))
                .collect::<Vec<_>>();

            for (family_id, _, revision) in &families {
                if !expired_families.contains(family_id) {
                    continue;
                }
                let all_children_selected = family_children
                    .get(family_id)
                    .into_iter()
                    .flatten()
                    .all(|child| selected_children.contains(child));
                if all_children_selected {
                    deletes.push(ControlDelete {
                        tenant_id: TENANT.into(),
                        record_type: FAMILY_RECORD_TYPE.into(),
                        record_id: family_id.clone(),
                        expected_revision: Some(*revision),
                    });
                    deleted_ids.push(format!("{FAMILY_RECORD_TYPE}:{family_id}"));
                }
            }
            if deletes.is_empty() {
                return Ok(0);
            }
            deleted_ids.sort();
            let batch_identity = format!("{}:{}", cutoff, deleted_ids.join("|"));
            let idempotency_key = format!(
                "session.expired.cleanup-{}",
                hex(&Sha256::digest(batch_identity.as_bytes()))
            );
            match self.store.execute(ExecuteControlCommand {
                tenant_id: TENANT.into(),
                command_id: "auth.session.expired.cleanup".into(),
                idempotency_key,
                expected_revisions: Default::default(),
                puts: vec![],
                deletes,
                audit: ControlAuditEntry {
                    tenant_id: TENANT.into(),
                    command_id: "auth.session.expired.cleanup".into(),
                    actor_id: "service".into(),
                    allowed: true,
                    detail: json!({"deletedRecords": deleted_ids.len()}),
                },
                response: success_response(),
            }) {
                Ok(_) => return Ok(deleted_ids.len()),
                Err(error) if error.code == "storage_revision_conflict" => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(0)
    }

    pub fn get(&self, token_hash: &[u8; 32]) -> Result<Option<(SessionRecord, u64)>, Problem> {
        let record_id = session_id(token_hash);
        let Some((record, revision)) =
            self.store
                .load_versioned::<SessionRecord>(TENANT, RECORD_TYPE, &record_id)?
        else {
            return Ok(None);
        };
        if record.token_hash != *token_hash {
            return Err(corrupt());
        }
        Ok(Some((record, revision)))
    }

    pub fn create(&self, record: &SessionRecord) -> Result<u64, Problem> {
        self.create_with_account_revision(record, None)
    }

    fn account_revision_put(
        &self,
        account_id: &AccountId,
        expected_revision: u64,
    ) -> Result<ControlPut, Problem> {
        AccountPersistence::new(self.store.clone())
            .prepare_credential_issuance_put(account_id, expected_revision)
    }

    pub fn create_with_account_revision(
        &self,
        record: &SessionRecord,
        account_revision: Option<u64>,
    ) -> Result<u64, Problem> {
        let session_id = session_id(&record.token_hash);
        let mut puts = vec![ControlPut {
            tenant_id: TENANT.into(),
            record_type: RECORD_TYPE.into(),
            record_id: session_id.clone(),
            expected_revision: Some(0),
            value: serde_json::to_value(record).map_err(|_| corrupt())?,
        }];
        let family_record_id = record.family_id.as_deref();
        if let Some(family_id) = family_record_id {
            if !is_family_id(family_id) {
                return Err(corrupt());
            }
            puts.push(ControlPut {
                tenant_id: TENANT.into(),
                record_type: FAMILY_RECORD_TYPE.into(),
                record_id: family_id.into(),
                expected_revision: Some(0),
                value: serde_json::to_value(SessionFamilyRecord {
                    account_id: record.account_id.clone(),
                    revoked_at: None,
                    expires_at: Some(record.expires_at),
                })
                .map_err(|_| corrupt())?,
            });
        }
        let account_put = account_revision
            .map(|revision| self.account_revision_put(&record.account_id, revision))
            .transpose()?;
        let expected_revisions = account_revision.map_or_else(BTreeMap::new, |revision| {
            BTreeMap::from([(
                ControlRecordKey {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: record.account_id.as_str().into(),
                },
                revision,
            )])
        });
        if let Some(account_put) = account_put {
            puts.push(account_put);
        }
        let result = self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "session.issue".into(),
            idempotency_key: format!("session.issue-{session_id}"),
            expected_revisions,
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "session.issue".into(),
                actor_id: record.account_id.as_str().into(),
                allowed: true,
                detail: json!({ "sessionId": session_id, "state": "issued" }),
            },
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: Value::Null,
            },
        })?;
        result
            .records
            .iter()
            .find(|stored| stored.record_type == RECORD_TYPE && stored.record_id == session_id)
            .map(|stored| stored.revision)
            .ok_or_else(unavailable)
    }

    pub fn create_device_session(
        &self,
        session: &SessionRecord,
        refresh: &DeviceRefreshRecord,
    ) -> Result<(), Problem> {
        self.create_device_session_with_account_revision(session, refresh, None)
    }

    pub fn create_device_session_with_account_revision(
        &self,
        session: &SessionRecord,
        refresh: &DeviceRefreshRecord,
        account_revision: Option<u64>,
    ) -> Result<(), Problem> {
        self.create_device_session_with_account_revision_and_deletes(
            session,
            refresh,
            account_revision,
            vec![],
        )
    }

    pub fn create_device_session_with_account_revision_and_deletes(
        &self,
        session: &SessionRecord,
        refresh: &DeviceRefreshRecord,
        account_revision: Option<u64>,
        deletes: Vec<ControlDelete>,
    ) -> Result<(), Problem> {
        self.create_device_session_with_account_revision_and_mutations(
            session,
            refresh,
            account_revision,
            Vec::new(),
            deletes,
        )
    }

    pub fn create_device_session_with_account_revision_and_mutations(
        &self,
        session: &SessionRecord,
        refresh: &DeviceRefreshRecord,
        account_revision: Option<u64>,
        additional_puts: Vec<ControlPut>,
        deletes: Vec<ControlDelete>,
    ) -> Result<(), Problem> {
        self.create_device_session_with_revision_retry(
            session,
            refresh,
            account_revision,
            additional_puts,
            deletes,
            0,
        )
    }

    fn create_device_session_with_revision_retry(
        &self,
        session: &SessionRecord,
        refresh: &DeviceRefreshRecord,
        account_revision: Option<u64>,
        mut additional_puts: Vec<ControlPut>,
        deletes: Vec<ControlDelete>,
        attempt: u8,
    ) -> Result<(), Problem> {
        validate_device_pair(session, refresh)?;
        let access_id = session_id(&session.token_hash);
        let refresh_id = session_id(&refresh.token_hash);
        let family_id = session.family_id.as_deref().ok_or_else(corrupt)?;
        let head_id = device_session_head_id(&session.account_id, &refresh.device_id);
        let current_head = self.store.load_versioned::<DeviceSessionHead>(
            TENANT,
            DEVICE_HEAD_RECORD_TYPE,
            &head_id,
        )?;
        if current_head.as_ref().is_some_and(|(head, _)| {
            head.account_id != session.account_id || !is_family_id(&head.family_id)
        }) {
            return Err(corrupt());
        }
        let head_revision = current_head.as_ref().map_or(0, |(_, revision)| *revision);
        let account_put = account_revision
            .map(|revision| self.account_revision_put(&session.account_id, revision))
            .transpose()?;
        let mut expected_revisions = account_revision.map_or_else(BTreeMap::new, |revision| {
            BTreeMap::from([(
                ControlRecordKey {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: session.account_id.as_str().into(),
                },
                revision,
            )])
        });
        expected_revisions.insert(
            ControlRecordKey {
                tenant_id: TENANT.into(),
                record_type: DEVICE_HEAD_RECORD_TYPE.into(),
                record_id: head_id.clone(),
            },
            head_revision,
        );
        let mut puts = vec![
            ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: access_id.clone(),
                expected_revision: Some(0),
                value: serde_json::to_value(session).map_err(|_| corrupt())?,
            },
            ControlPut {
                tenant_id: TENANT.into(),
                record_type: REFRESH_RECORD_TYPE.into(),
                record_id: refresh_id.clone(),
                expected_revision: Some(0),
                value: serde_json::to_value(refresh).map_err(|_| corrupt())?,
            },
            ControlPut {
                tenant_id: TENANT.into(),
                record_type: FAMILY_RECORD_TYPE.into(),
                record_id: family_id.into(),
                expected_revision: Some(0),
                value: serde_json::to_value(SessionFamilyRecord {
                    account_id: session.account_id.clone(),
                    revoked_at: None,
                    expires_at: Some(refresh.expires_at),
                })
                .map_err(|_| corrupt())?,
            },
            ControlPut {
                tenant_id: TENANT.into(),
                record_type: DEVICE_HEAD_RECORD_TYPE.into(),
                record_id: head_id.clone(),
                expected_revision: Some(head_revision),
                value: serde_json::to_value(DeviceSessionHead {
                    account_id: session.account_id.clone(),
                    family_id: family_id.into(),
                })
                .map_err(|_| corrupt())?,
            },
        ];
        if let Some(account_put) = account_put {
            puts.push(account_put);
        }
        puts.append(&mut additional_puts);
        let result = self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "session.device.issue".into(),
            idempotency_key: format!(
                "session.device.issue-{}-{}",
                &access_id[..16],
                &refresh_id[..16]
            ),
            expected_revisions,
            puts,
            deletes: deletes.clone(),
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "session.device.issue".into(),
                actor_id: session.account_id.as_str().into(),
                allowed: true,
                detail: json!({ "sessionId": access_id, "state": "issued" }),
            },
            response: success_response(),
        });
        if let Err(error) = result {
            if error.code == "storage_revision_conflict" {
                let latest_head_revision = self
                    .store
                    .revision(TENANT, DEVICE_HEAD_RECORD_TYPE, &head_id)?
                    .unwrap_or(0);
                if latest_head_revision == head_revision {
                    return Err(error);
                }
                if let Some(expected_revision) = account_revision {
                    let actual_revision = AccountPersistence::new(self.store.clone())
                        .load_active_versioned(&session.account_id)?
                        .map(|(_, revision)| revision);
                    if actual_revision != Some(expected_revision) {
                        return Err(error);
                    }
                }
                if attempt < 2 {
                    let retry_puts = session
                        .created_at
                        .map(|now| {
                            self.device_session_revocation_puts(
                                &session.account_id,
                                &refresh.device_id,
                                now,
                            )
                        })
                        .transpose()?
                        .unwrap_or_default();
                    return self.create_device_session_with_revision_retry(
                        session,
                        refresh,
                        account_revision,
                        retry_puts,
                        deletes,
                        attempt + 1,
                    );
                }
                let mut conflict = Problem::new(
                    "device_session_issue_conflict",
                    "A sign-in for this device conflicted with another request",
                    409,
                    "session.device.issue",
                );
                conflict.retryable = true;
                return Err(conflict);
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn device_session_revocation_puts(
        &self,
        account_id: &AccountId,
        device_id: &str,
        now: u64,
    ) -> Result<Vec<ControlPut>, Problem> {
        if DeviceId::new(device_id.to_owned()).is_err() {
            return Err(corrupt());
        }
        let mut active_families = BTreeSet::new();
        let mut legacy_sessions = Vec::new();
        for (_, mut session, revision) in self.store.list_versioned_for_account::<SessionRecord>(
            TENANT,
            RECORD_TYPE,
            account_id.as_str(),
        )? {
            if &session.account_id != account_id {
                return Err(corrupt());
            }
            if session.device_id.as_deref() != Some(device_id) || session.revoked_at.is_some() {
                continue;
            }
            if let Some(family_id) = session.family_id.as_deref() {
                if !is_family_id(family_id) {
                    return Err(corrupt());
                }
                active_families.insert(family_id.to_owned());
            } else {
                session.revoked_at = Some(now);
                legacy_sessions.push(ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: RECORD_TYPE.into(),
                    record_id: session_id(&session.token_hash),
                    expected_revision: Some(revision),
                    value: serde_json::to_value(session).map_err(|_| corrupt())?,
                });
            }
        }

        let mut puts = legacy_sessions;
        for (family_id, mut family, revision) in self
            .store
            .list_versioned_for_account::<SessionFamilyRecord>(
                TENANT,
                FAMILY_RECORD_TYPE,
                account_id.as_str(),
            )?
        {
            if &family.account_id != account_id || !is_family_id(&family_id) {
                return Err(corrupt());
            }
            if active_families.contains(&family_id) && family.revoked_at.is_none() {
                family.revoked_at = Some(now);
                puts.push(ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: FAMILY_RECORD_TYPE.into(),
                    record_id: family_id,
                    expected_revision: Some(revision),
                    value: serde_json::to_value(family).map_err(|_| corrupt())?,
                });
            }
        }
        Ok(puts)
    }

    pub fn get_device_refresh(
        &self,
        token_hash: &[u8; 32],
    ) -> Result<Option<(DeviceRefreshRecord, u64)>, Problem> {
        let record_id = session_id(token_hash);
        let Some((record, revision)) = self.store.load_versioned::<DeviceRefreshRecord>(
            TENANT,
            REFRESH_RECORD_TYPE,
            &record_id,
        )?
        else {
            return Ok(None);
        };
        if record.token_hash != *token_hash {
            return Err(corrupt());
        }
        Ok(Some((record, revision)))
    }

    pub fn active_device_ids(
        &self,
        account_id: &AccountId,
        now: u64,
    ) -> Result<BTreeSet<String>, Problem> {
        let revoked_families = self.family_revocations(account_id)?;
        let devices = self
            .store
            .list_versioned_for_account::<DeviceRefreshRecord>(
                TENANT,
                REFRESH_RECORD_TYPE,
                account_id.as_str(),
            )?
            .into_iter()
            .map(|(record_id, record, _)| {
                if record_id != session_id(&record.token_hash)
                    || record.account_id != *account_id
                    || DeviceId::new(record.device_id.clone()).is_err()
                    || !is_family_id(&record.family_id)
                {
                    return Err(corrupt());
                }
                if record.account_id == *account_id
                    && record.consumed_at.is_none()
                    && record.expires_at > now
                    && !revoked_families.contains_key(&record.family_id)
                {
                    Ok(Some(record.device_id))
                } else {
                    Ok(None)
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(devices.into_iter().flatten().collect())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "explicit records preserve the atomic device session rotation contract"
    )]
    pub fn rotate_device_session(
        &self,
        old_session: &SessionRecord,
        old_session_revision: u64,
        old_refresh: &DeviceRefreshRecord,
        old_refresh_revision: u64,
        family_revision: u64,
        new_session: &SessionRecord,
        new_refresh: &DeviceRefreshRecord,
        account_revision: Option<u64>,
    ) -> Result<(), Problem> {
        validate_device_pair(new_session, new_refresh)?;
        if old_session.account_id != new_session.account_id
            || old_refresh.account_id != new_refresh.account_id
            || old_refresh.family_id != new_refresh.family_id
            || old_refresh.device_id != new_refresh.device_id
            || old_refresh.access_token_hash != old_session.token_hash
            || old_session.family_id.as_deref() != Some(old_refresh.family_id.as_str())
            || old_refresh.consumed_at.is_none()
        {
            return Err(corrupt());
        }
        let old_session_id = session_id(&old_session.token_hash);
        let old_refresh_id = session_id(&old_refresh.token_hash);
        let new_session_id = session_id(&new_session.token_hash);
        let new_refresh_id = session_id(&new_refresh.token_hash);
        let account_put = account_revision
            .map(|revision| self.account_revision_put(&old_session.account_id, revision))
            .transpose()?;
        let mut expected_revisions = BTreeMap::from([(
            ControlRecordKey {
                tenant_id: TENANT.into(),
                record_type: FAMILY_RECORD_TYPE.into(),
                record_id: old_refresh.family_id.clone(),
            },
            family_revision,
        )]);
        if let Some(revision) = account_revision {
            expected_revisions.insert(
                ControlRecordKey {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: old_session.account_id.as_str().into(),
                },
                revision,
            );
        }
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "session.device.refresh".into(),
            idempotency_key: format!(
                "session.device.refresh-{}-{}-{}",
                &old_refresh_id[..16],
                &new_refresh_id[..16],
                old_refresh.consumed_at.unwrap_or_default()
            ),
            expected_revisions,
            puts: {
                let mut puts = vec![
                    ControlPut {
                        tenant_id: TENANT.into(),
                        record_type: RECORD_TYPE.into(),
                        record_id: old_session_id.clone(),
                        expected_revision: Some(old_session_revision),
                        value: serde_json::to_value(old_session).map_err(|_| corrupt())?,
                    },
                    ControlPut {
                        tenant_id: TENANT.into(),
                        record_type: REFRESH_RECORD_TYPE.into(),
                        record_id: old_refresh_id,
                        expected_revision: Some(old_refresh_revision),
                        value: serde_json::to_value(old_refresh).map_err(|_| corrupt())?,
                    },
                    ControlPut {
                        tenant_id: TENANT.into(),
                        record_type: RECORD_TYPE.into(),
                        record_id: new_session_id.clone(),
                        expected_revision: Some(0),
                        value: serde_json::to_value(new_session).map_err(|_| corrupt())?,
                    },
                    ControlPut {
                        tenant_id: TENANT.into(),
                        record_type: REFRESH_RECORD_TYPE.into(),
                        record_id: new_refresh_id,
                        expected_revision: Some(0),
                        value: serde_json::to_value(new_refresh).map_err(|_| corrupt())?,
                    },
                ];
                if let Some(account_put) = account_put {
                    puts.push(account_put);
                }
                puts
            },
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "session.device.refresh".into(),
                actor_id: old_session.account_id.as_str().into(),
                allowed: true,
                detail: json!({
                    "sessionId": old_session_id,
                    "replacementSessionId": new_session_id,
                    "state": "rotated"
                }),
            },
            response: success_response(),
        })?;
        Ok(())
    }

    pub fn family_revoked_at(
        &self,
        family_id: &str,
        account_id: &AccountId,
    ) -> Result<Option<Option<u64>>, Problem> {
        if !is_family_id(family_id) {
            return Err(corrupt());
        }
        let Some((family, _)) = self.store.load_versioned::<SessionFamilyRecord>(
            TENANT,
            FAMILY_RECORD_TYPE,
            family_id,
        )?
        else {
            // Pre-family records are accepted until a replay creates the revocation record.
            return Ok(None);
        };
        if &family.account_id != account_id {
            return Err(corrupt());
        }
        Ok(Some(family.revoked_at))
    }

    /// Device refresh records are created atomically with their family record. Unlike
    /// legacy access sessions, a missing family is corruption and must fail closed.
    pub fn device_refresh_family_revoked_at(
        &self,
        family_id: &str,
        account_id: &AccountId,
    ) -> Result<Option<u64>, Problem> {
        self.device_refresh_family_state(family_id, account_id)
            .map(|(revoked_at, _)| revoked_at)
    }

    /// Read the family's revocation state and storage revision from one snapshot.
    /// Refresh uses the revision in its rotation transaction so concurrent family
    /// revocation cannot race past the preflight check.
    pub fn device_refresh_family_state(
        &self,
        family_id: &str,
        account_id: &AccountId,
    ) -> Result<(Option<u64>, u64), Problem> {
        if !is_family_id(family_id) {
            return Err(corrupt());
        }
        let Some((family, revision)) = self.store.load_versioned::<SessionFamilyRecord>(
            TENANT,
            FAMILY_RECORD_TYPE,
            family_id,
        )?
        else {
            return Err(corrupt());
        };
        if &family.account_id != account_id {
            return Err(corrupt());
        }
        Ok((family.revoked_at, revision))
    }

    pub fn family_revocations(
        &self,
        account_id: &AccountId,
    ) -> Result<HashMap<String, u64>, Problem> {
        let mut revoked = HashMap::new();
        for (family_id, family, _) in self
            .store
            .list_versioned_for_account::<SessionFamilyRecord>(
                TENANT,
                FAMILY_RECORD_TYPE,
                account_id.as_str(),
            )?
        {
            if &family.account_id != account_id {
                return Err(corrupt());
            }
            if !is_family_id(&family_id) {
                return Err(corrupt());
            }
            if let Some(at) = family.revoked_at {
                revoked.insert(family_id, at);
            }
        }
        Ok(revoked)
    }

    pub fn account_session_revocation_puts(
        &self,
        account_id: &AccountId,
        now: u64,
    ) -> Result<Vec<ControlPut>, Problem> {
        let mut family_records = HashMap::new();
        let mut known_families = BTreeSet::new();
        for (family_id, mut family, revision) in self
            .store
            .list_versioned_for_account::<SessionFamilyRecord>(
                TENANT,
                FAMILY_RECORD_TYPE,
                account_id.as_str(),
            )?
        {
            if &family.account_id != account_id {
                return Err(corrupt());
            }
            if !is_family_id(&family_id) {
                return Err(corrupt());
            }
            known_families.insert(family_id.clone());
            if family.revoked_at.is_none() {
                family.revoked_at = Some(now);
                family_records.insert(
                    family_id.clone(),
                    ControlPut {
                        tenant_id: TENANT.into(),
                        record_type: FAMILY_RECORD_TYPE.into(),
                        record_id: family_id,
                        expected_revision: Some(revision),
                        value: serde_json::to_value(family).map_err(|_| corrupt())?,
                    },
                );
            }
        }

        let mut legacy_session_records = Vec::new();
        for (_, session, revision) in self.store.list_versioned_for_account::<SessionRecord>(
            TENANT,
            RECORD_TYPE,
            account_id.as_str(),
        )? {
            if &session.account_id != account_id {
                return Err(corrupt());
            }
            if session.revoked_at.is_some() {
                continue;
            }
            match session.family_id.as_deref() {
                Some(family_id) => {
                    if !is_family_id(family_id) {
                        return Err(corrupt());
                    }
                    if !known_families.contains(family_id) {
                        let family = SessionFamilyRecord {
                            account_id: account_id.clone(),
                            revoked_at: Some(now),
                            expires_at: Some(session.expires_at),
                        };
                        family_records.insert(
                            family_id.to_owned(),
                            ControlPut {
                                tenant_id: TENANT.into(),
                                record_type: FAMILY_RECORD_TYPE.into(),
                                record_id: family_id.to_owned(),
                                expected_revision: Some(0),
                                value: serde_json::to_value(family).map_err(|_| corrupt())?,
                            },
                        );
                        known_families.insert(family_id.to_owned());
                    }
                }
                None => {
                    let mut session = session;
                    session.revoked_at = Some(now);
                    legacy_session_records.push(ControlPut {
                        tenant_id: TENANT.into(),
                        record_type: RECORD_TYPE.into(),
                        record_id: session_id(&session.token_hash),
                        expected_revision: Some(revision),
                        value: serde_json::to_value(session).map_err(|_| corrupt())?,
                    });
                }
            }
        }
        let mut puts = family_records.into_values().collect::<Vec<_>>();
        puts.extend(legacy_session_records);
        Ok(puts)
    }

    pub fn revoke_account_sessions(
        &self,
        account_id: &AccountId,
        now: u64,
    ) -> Result<usize, Problem> {
        let puts = self.account_session_revocation_puts(account_id, now)?;
        if puts.is_empty() {
            return Ok(0);
        }
        let count = puts.len();
        self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "session.account.revoke".into(),
            idempotency_key: format!("session.account.revoke-{}-{now}", account_id.as_str()),
            expected_revisions: Default::default(),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "session.account.revoke".into(),
                actor_id: account_id.as_str().into(),
                allowed: true,
                detail: json!({"state":"revoked","recordCount":count}),
            },
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: json!({"recordCount":count}),
            },
        })?;
        Ok(count)
    }

    pub fn revoke_family(
        &self,
        family_id: &str,
        account_id: &AccountId,
        now: u64,
    ) -> Result<(), Problem> {
        if !is_family_id(family_id) {
            return Err(corrupt());
        }
        let current = self.store.load_versioned::<SessionFamilyRecord>(
            TENANT,
            FAMILY_RECORD_TYPE,
            family_id,
        )?;
        let (expected_revision, mut family) = match current {
            Some((family, revision)) => (revision, family),
            None => (
                0,
                SessionFamilyRecord {
                    account_id: account_id.clone(),
                    revoked_at: None,
                    expires_at: None,
                },
            ),
        };
        if &family.account_id != account_id {
            return Err(corrupt());
        }
        if family.revoked_at.is_some() {
            return Ok(());
        }
        family.revoked_at = Some(now);
        let result = self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "session.family.revoke".into(),
            idempotency_key: format!("session.family.revoke-{family_id}-{now}"),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: FAMILY_RECORD_TYPE.into(),
                record_id: family_id.into(),
                expected_revision: Some(expected_revision),
                value: serde_json::to_value(&family).map_err(|_| corrupt())?,
            }],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "session.family.revoke".into(),
                actor_id: account_id.as_str().into(),
                allowed: true,
                detail: json!({ "familyId": family_id, "state": "revoked" }),
            },
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: Value::Null,
            },
        });
        match result {
            Ok(_) => Ok(()),
            Err(error) => match self.family_revoked_at(family_id, account_id)? {
                Some(Some(_)) => Ok(()),
                _ => Err(error),
            },
        }
    }

    pub fn revoke_session(
        &self,
        record: &SessionRecord,
        expected_revision: u64,
        client_idempotency_key: &str,
        session_was_active: bool,
    ) -> Result<u64, Problem> {
        let record_id = session_id(&record.token_hash);
        let scope = format!(
            "{}:auth.sessions.revoke:{client_idempotency_key}",
            record.account_id.as_str()
        );
        let command_key = format!("session-revoke-{}", hex(&Sha256::digest(scope.as_bytes())));
        let request_hash = hex(&Sha256::digest(record_id.as_bytes()));
        if let Some(replay) = self.store.replay(TENANT, &command_key)? {
            return self.replayed_session_revision(replay, &record_id, &request_hash);
        }

        let mut puts = Vec::new();
        if session_was_active {
            puts.push(ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: record_id.clone(),
                expected_revision: Some(expected_revision),
                value: serde_json::to_value(record).map_err(|_| corrupt())?,
            });
        }
        if let Some(family_id) = record.family_id.as_deref() {
            if !is_family_id(family_id) {
                return Err(corrupt());
            }
            let (expected_family_revision, mut family) = match self
                .store
                .load_versioned::<SessionFamilyRecord>(TENANT, FAMILY_RECORD_TYPE, family_id)?
            {
                Some((family, revision)) => (revision, family),
                None => (
                    0,
                    SessionFamilyRecord {
                        account_id: record.account_id.clone(),
                        revoked_at: None,
                        expires_at: Some(record.expires_at),
                    },
                ),
            };
            if family.account_id != record.account_id {
                return Err(corrupt());
            }
            if family.revoked_at.is_none() {
                family.revoked_at = record.revoked_at;
                puts.push(ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: FAMILY_RECORD_TYPE.into(),
                    record_id: family_id.into(),
                    expected_revision: Some(expected_family_revision),
                    value: serde_json::to_value(family).map_err(|_| corrupt())?,
                });
            }
        }
        let command = ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.sessions.revoke".into(),
            idempotency_key: command_key.clone(),
            expected_revisions: Default::default(),
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "auth.sessions.revoke".into(),
                actor_id: record.account_id.as_str().into(),
                allowed: true,
                detail: json!({"sessionId": record_id, "state": "revoked"}),
            },
            response: HttpResponse {
                status: 204,
                headers: [("x-buglogin-input-sha256".into(), request_hash.clone())]
                    .into_iter()
                    .collect(),
                body: Value::Null,
            },
        };
        match self.store.execute(command) {
            Ok(result) => self.replayed_session_revision(result, &record_id, &request_hash),
            Err(error) => match self.store.replay(TENANT, &command_key)? {
                Some(replay) => self.replayed_session_revision(replay, &record_id, &request_hash),
                _ => Err(error),
            },
        }
    }

    fn replayed_session_revision(
        &self,
        result: ControlCommandResult,
        record_id: &str,
        request_hash: &str,
    ) -> Result<u64, Problem> {
        if result.response.status != 204 {
            return Err(corrupt());
        }
        if result
            .response
            .headers
            .get("x-buglogin-input-sha256")
            .is_none_or(|stored| stored != request_hash)
        {
            return Err(idempotency_conflict());
        }
        if let Some(record) = result
            .records
            .iter()
            .find(|stored| stored.record_type == RECORD_TYPE && stored.record_id == record_id)
        {
            return Ok(record.revision);
        }
        self.store
            .load_versioned::<SessionRecord>(TENANT, RECORD_TYPE, record_id)?
            .map(|(_, revision)| revision)
            .ok_or_else(corrupt)
    }

    pub fn rotate(
        &self,
        revoked: &SessionRecord,
        expected_revision: u64,
        replacement: &SessionRecord,
    ) -> Result<(u64, u64), Problem> {
        self.rotate_with_account_revision(revoked, expected_revision, replacement, None)
    }

    pub fn rotate_with_account_revision(
        &self,
        revoked: &SessionRecord,
        expected_revision: u64,
        replacement: &SessionRecord,
        account_revision: Option<u64>,
    ) -> Result<(u64, u64), Problem> {
        let revoked_id = session_id(&revoked.token_hash);
        let replacement_id = session_id(&replacement.token_hash);
        let account_put = account_revision
            .map(|revision| self.account_revision_put(&revoked.account_id, revision))
            .transpose()?;
        let expected_revisions = account_revision.map_or_else(BTreeMap::new, |revision| {
            BTreeMap::from([(
                ControlRecordKey {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: revoked.account_id.as_str().into(),
                },
                revision,
            )])
        });
        let mut puts = vec![
            ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: revoked_id.clone(),
                expected_revision: Some(expected_revision),
                value: serde_json::to_value(revoked).map_err(|_| corrupt())?,
            },
            ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: replacement_id.clone(),
                expected_revision: Some(0),
                value: serde_json::to_value(replacement).map_err(|_| corrupt())?,
            },
        ];
        if let Some(account_put) = account_put {
            puts.push(account_put);
        }
        let result = self.store.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "session.refresh".into(),
            idempotency_key: format!(
                "session.refresh-{}-{}-{}",
                &revoked_id[..16],
                &replacement_id[..16],
                revoked.revoked_at.unwrap_or(0)
            ),
            expected_revisions,
            puts,
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "session.refresh".into(),
                actor_id: revoked.account_id.as_str().into(),
                allowed: true,
                detail: json!({
                    "revokedSessionId": revoked_id,
                    "replacementSessionId": replacement_id,
                    "state": "rotated"
                }),
            },
            response: HttpResponse {
                status: 200,
                headers: Default::default(),
                body: Value::Null,
            },
        })?;
        let revoked_revision = result
            .records
            .iter()
            .find(|record| record.record_id == revoked_id)
            .map(|record| record.revision)
            .ok_or_else(unavailable)?;
        let replacement_revision = result
            .records
            .iter()
            .find(|record| record.record_id == replacement_id)
            .map(|record| record.revision)
            .ok_or_else(unavailable)?;
        Ok((revoked_revision, replacement_revision))
    }
}

fn session_id(hash: &[u8; 32]) -> String {
    hex(hash)
}

fn device_session_head_id(account_id: &AccountId, device_id: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"buglogin-device-session-head-v1\0");
    digest.update(account_id.as_str().as_bytes());
    digest.update([0]);
    digest.update(device_id.as_bytes());
    hex(&digest.finalize())
}

#[cfg(test)]
#[test]
fn device_session_head_ids_are_stable_and_do_not_expose_device_ids() {
    let account = AccountId::new("acct_device_head_id").unwrap();
    let first = device_session_head_id(&account, "desktop-device-alpha");
    assert_eq!(
        first,
        device_session_head_id(&account, "desktop-device-alpha")
    );
    assert_ne!(
        first,
        device_session_head_id(&account, "desktop-device-beta")
    );
    assert_ne!(
        first,
        device_session_head_id(
            &AccountId::new("acct_other").unwrap(),
            "desktop-device-alpha"
        )
    );
    assert!(!first.contains("desktop-device-alpha"));
}

fn hex(bytes: &[u8]) -> String {
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn is_family_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_device_pair(
    session: &SessionRecord,
    refresh: &DeviceRefreshRecord,
) -> Result<(), Problem> {
    if session.device_id.as_deref() != Some(refresh.device_id.as_str())
        || session.family_id.as_deref() != Some(refresh.family_id.as_str())
        || session.account_id != refresh.account_id
        || session.token_hash != refresh.access_token_hash
        || session.credential_version != refresh.credential_version
        || !is_family_id(&refresh.family_id)
        || DeviceId::new(refresh.device_id.clone()).is_err()
        || refresh.expires_at == 0
        || refresh.consumed_at.is_some()
    {
        return Err(corrupt());
    }
    Ok(())
}

fn success_response() -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: Default::default(),
        body: Value::Null,
    }
}

fn unavailable() -> Problem {
    Problem::new(
        "session_storage_unavailable",
        "Session storage is unavailable",
        503,
        "identity",
    )
    .retryable(true)
}

fn corrupt() -> Problem {
    Problem::new(
        "session_record_corrupt",
        "Session record is corrupt",
        500,
        "identity",
    )
}

fn idempotency_conflict() -> Problem {
    Problem::new(
        "idempotency_conflict",
        "Idempotency key was reused with different input",
        409,
        "identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_domain::{
        account::{Account, AccountStatus},
        identity::{issue, issue_for_device},
        ids::AccountId,
    };
    use buglogin_storage::control::{
        ControlRecordKey, ControlRecordStore, MemoryControlRecordStore,
    };

    #[test]
    fn device_session_issue_updates_head_and_revokes_prior_device_family_atomically() {
        let store = MemoryControlRecordStore::default();
        let persistence = SessionPersistence::new(ControlPersistence::new(store));
        let account_id = AccountId::new("acct_device_head_atomic").unwrap();
        let device_id = "device-head-atomic";
        let make_pair = |suffix: &str, issued_at: u64| {
            let mut issued = issue_for_device(
                account_id.clone(),
                issued_at + 3_600,
                Some(device_id.to_owned()),
            )
            .unwrap();
            issued.record.created_at = Some(issued_at);
            issued.record.last_seen_at = Some(issued_at);
            let family_id = issued.record.family_id.clone().unwrap();
            let refresh = DeviceRefreshRecord {
                account_id: account_id.clone(),
                token_hash: Sha256::digest(suffix.as_bytes()).into(),
                access_token_hash: issued.record.token_hash,
                device_id: device_id.to_owned(),
                family_id,
                expires_at: issued_at + 86_400,
                consumed_at: None,
                credential_version: None,
                refresh_replay: None,
            };
            (issued.record.clone(), refresh)
        };

        let (first_session, first_refresh) = make_pair("device-head-first", 100);
        let first_family = first_session.family_id.clone().unwrap();
        persistence
            .create_device_session_with_account_revision_and_mutations(
                &first_session,
                &first_refresh,
                None,
                Vec::new(),
                Vec::new(),
            )
            .unwrap();

        let (next_session, next_refresh) = make_pair("device-head-next", 200);
        let next_family = next_session.family_id.clone().unwrap();
        let revocations = persistence
            .device_session_revocation_puts(&account_id, device_id, 200)
            .unwrap();
        persistence
            .create_device_session_with_account_revision_and_mutations(
                &next_session,
                &next_refresh,
                None,
                revocations,
                Vec::new(),
            )
            .unwrap();

        let head_id = device_session_head_id(&account_id, device_id);
        let (head, revision) = persistence
            .store
            .load_versioned::<DeviceSessionHead>(TENANT, DEVICE_HEAD_RECORD_TYPE, &head_id)
            .unwrap()
            .unwrap();
        assert_eq!(revision, 2);
        assert_eq!(head.account_id, account_id);
        assert_eq!(head.family_id, next_family);
        assert_eq!(
            persistence
                .family_revocations(&account_id)
                .unwrap()
                .get(&first_family),
            Some(&200)
        );
    }

    #[test]
    fn device_issue_and_rotation_are_fenced_by_durable_account_status_revision() {
        let mut store = MemoryControlRecordStore::default();
        let persistence = SessionPersistence::new(ControlPersistence::new(store.clone()));
        let account_id = AccountId::new("acct_refresh_account_fence").unwrap();
        let account_key = ControlRecordKey {
            tenant_id: "control".into(),
            record_type: "account".into(),
            record_id: account_id.as_str().into(),
        };
        let account = Account::new(account_id.clone(), "2026-10-18T00:00:00Z");
        store
            .execute(ExecuteControlCommand {
                tenant_id: "control".into(),
                command_id: "account.provision".into(),
                idempotency_key: "account-refresh-fence-seed".into(),
                expected_revisions: BTreeMap::new(),
                puts: vec![ControlPut {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: account_id.as_str().into(),
                    expected_revision: Some(0),
                    value: serde_json::to_value(&account).unwrap(),
                }],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "control".into(),
                    command_id: "account.provision".into(),
                    actor_id: "service".into(),
                    allowed: true,
                    detail: Value::Null,
                },
                response: success_response(),
            })
            .unwrap();

        let issued = issue(account_id.clone(), 10_000).unwrap();
        let mut old_session = issued.record.clone();
        old_session.device_id = Some("desktop-device-refresh-fence".into());
        let family_id = old_session.family_id.clone().unwrap();
        let old_refresh = DeviceRefreshRecord {
            account_id: account_id.clone(),
            token_hash: Sha256::digest(b"old-refresh-fence").into(),
            access_token_hash: old_session.token_hash,
            device_id: "desktop-device-refresh-fence".into(),
            family_id: family_id.clone(),
            expires_at: 20_000,
            consumed_at: None,
            credential_version: None,
            refresh_replay: None,
        };
        persistence
            .create_device_session_with_account_revision(&old_session, &old_refresh, Some(1))
            .unwrap();

        let (current_account, current_account_revision) = store
            .get("control", "account", account_id.as_str())
            .unwrap()
            .map(|record| {
                let revision = record.revision;
                let account = serde_json::from_value::<Account>(record.value).unwrap();
                (account, revision)
            })
            .unwrap();
        let suspended = current_account
            .transition_status(
                current_account.revision,
                AccountStatus::Suspended,
                "2026-10-18T00:01:00Z",
            )
            .unwrap();
        store
            .execute(ExecuteControlCommand {
                tenant_id: "control".into(),
                command_id: "account.status.change".into(),
                idempotency_key: "account-refresh-fence-suspend".into(),
                expected_revisions: BTreeMap::from([(
                    account_key.clone(),
                    current_account_revision,
                )]),
                puts: vec![ControlPut {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: account_id.as_str().into(),
                    expected_revision: Some(current_account_revision),
                    value: serde_json::to_value(suspended).unwrap(),
                }],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "control".into(),
                    command_id: "account.status.change".into(),
                    actor_id: "operator".into(),
                    allowed: true,
                    detail: Value::Null,
                },
                response: success_response(),
            })
            .unwrap();

        let stale_browser_issue = issue(account_id.clone(), 10_000).unwrap().record.clone();
        assert_eq!(
            persistence
                .create_with_account_revision(&stale_browser_issue, Some(current_account_revision))
                .unwrap_err()
                .code,
            "storage_revision_conflict"
        );
        assert!(
            persistence
                .get(&stale_browser_issue.token_hash)
                .unwrap()
                .is_none()
        );

        let mut new_issue = issue(account_id.clone(), 10_000).unwrap().record.clone();
        new_issue.device_id = Some("desktop-device-refresh-fence".into());
        let new_issue_refresh = DeviceRefreshRecord {
            account_id: account_id.clone(),
            token_hash: Sha256::digest(b"new-issue-refresh-fence").into(),
            access_token_hash: new_issue.token_hash,
            device_id: "desktop-device-refresh-fence".into(),
            family_id: new_issue.family_id.clone().unwrap(),
            expires_at: 20_000,
            consumed_at: None,
            credential_version: None,
            refresh_replay: None,
        };
        assert_eq!(
            persistence
                .create_device_session_with_account_revision(
                    &new_issue,
                    &new_issue_refresh,
                    Some(current_account_revision),
                )
                .unwrap_err()
                .code,
            "storage_revision_conflict"
        );
        assert!(persistence.get(&new_issue.token_hash).unwrap().is_none());
        assert!(
            persistence
                .get_device_refresh(&new_issue_refresh.token_hash)
                .unwrap()
                .is_none()
        );

        let mut replacement = old_session.clone();
        replacement.token_hash = Sha256::digest(b"replacement-session-fence").into();
        replacement.expires_at = 11_000;
        replacement.revoked_at = None;
        let replacement_refresh = DeviceRefreshRecord {
            account_id: account_id.clone(),
            token_hash: Sha256::digest(b"replacement-refresh-fence").into(),
            access_token_hash: replacement.token_hash,
            device_id: "desktop-device-refresh-fence".into(),
            family_id,
            expires_at: old_refresh.expires_at,
            consumed_at: None,
            credential_version: None,
            refresh_replay: None,
        };
        let mut consumed_refresh = old_refresh.clone();
        consumed_refresh.consumed_at = Some(10);
        let mut revoked_session = old_session.clone();
        revoked_session.revoked_at = Some(10);

        assert_eq!(
            persistence
                .rotate_device_session(
                    &revoked_session,
                    1,
                    &consumed_refresh,
                    1,
                    1,
                    &replacement,
                    &replacement_refresh,
                    Some(current_account_revision),
                )
                .unwrap_err()
                .code,
            "storage_revision_conflict"
        );
        let persisted_old_session = persistence.get(&old_session.token_hash).unwrap().unwrap().0;
        assert_eq!(persisted_old_session.revoked_at, None);
        assert_eq!(
            persistence
                .get_device_refresh(&old_refresh.token_hash)
                .unwrap()
                .unwrap()
                .0
                .consumed_at,
            None
        );
        assert!(persistence.get(&replacement.token_hash).unwrap().is_none());
    }

    #[test]
    fn device_rotation_is_fenced_by_concurrent_family_revocation() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let account_id = AccountId::new("acct_refresh_family_fence").unwrap();
        let device_id = "desktop-family-fence".to_owned();
        let mut old_session = issue_for_device(account_id.clone(), 10_000, Some(device_id.clone()))
            .unwrap()
            .record
            .clone();
        let family_id = old_session.family_id.clone().unwrap();
        let old_refresh = DeviceRefreshRecord {
            account_id: account_id.clone(),
            token_hash: Sha256::digest(b"old-family-fence-refresh").into(),
            access_token_hash: old_session.token_hash,
            device_id: device_id.clone(),
            family_id: family_id.clone(),
            expires_at: 20_000,
            consumed_at: None,
            credential_version: None,
            refresh_replay: None,
        };
        persistence
            .create_device_session(&old_session, &old_refresh)
            .unwrap();
        let (revoked_at, family_revision) = persistence
            .device_refresh_family_state(&family_id, &account_id)
            .unwrap();
        assert_eq!(revoked_at, None);

        persistence
            .revoke_family(&family_id, &account_id, 100)
            .unwrap();
        old_session.revoked_at = Some(100);
        let mut consumed_refresh = old_refresh.clone();
        consumed_refresh.consumed_at = Some(100);
        let mut replacement = issue_for_device(account_id.clone(), 11_000, Some(device_id.clone()))
            .unwrap()
            .record
            .clone();
        replacement.family_id = Some(family_id.clone());
        let replacement_refresh = DeviceRefreshRecord {
            account_id: account_id.clone(),
            token_hash: Sha256::digest(b"replacement-family-fence-refresh").into(),
            access_token_hash: replacement.token_hash,
            device_id,
            family_id,
            expires_at: old_refresh.expires_at,
            consumed_at: None,
            credential_version: None,
            refresh_replay: None,
        };

        assert_eq!(
            persistence
                .rotate_device_session(
                    &old_session,
                    1,
                    &consumed_refresh,
                    1,
                    family_revision,
                    &replacement,
                    &replacement_refresh,
                    None,
                )
                .unwrap_err()
                .code,
            "storage_revision_conflict"
        );
        assert_eq!(
            persistence
                .get(&old_session.token_hash)
                .unwrap()
                .unwrap()
                .0
                .revoked_at,
            None
        );
        assert_eq!(
            persistence
                .get_device_refresh(&old_refresh.token_hash)
                .unwrap()
                .unwrap()
                .0
                .consumed_at,
            None
        );
        assert!(persistence.get(&replacement.token_hash).unwrap().is_none());
    }

    #[test]
    fn device_code_consumption_and_session_issue_commit_or_rollback_together() {
        let mut store = MemoryControlRecordStore::default();
        let persistence = SessionPersistence::new(ControlPersistence::new(store.clone()));
        let account_id = AccountId::new("acct_device_exchange_atomic").unwrap();
        let account = Account::new(account_id.clone(), "2026-10-18T00:00:00Z");
        store
            .execute(ExecuteControlCommand {
                tenant_id: "control".into(),
                command_id: "account.provision".into(),
                idempotency_key: "account-device-exchange-atomic".into(),
                expected_revisions: BTreeMap::new(),
                puts: vec![ControlPut {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: account_id.as_str().into(),
                    expected_revision: Some(0),
                    value: serde_json::to_value(&account).unwrap(),
                }],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "control".into(),
                    command_id: "account.provision".into(),
                    actor_id: "service".into(),
                    allowed: true,
                    detail: Value::Null,
                },
                response: success_response(),
            })
            .unwrap();

        let authorization_key = ControlRecordKey {
            tenant_id: "identity".into(),
            record_type: "device_authorization".into(),
            record_id: "auth_atomic_success".into(),
        };
        let user_code_key = ControlRecordKey {
            tenant_id: "identity".into(),
            record_type: "device_user_code".into(),
            record_id: "index_atomic_success".into(),
        };
        store
            .execute(ExecuteControlCommand {
                tenant_id: "identity".into(),
                command_id: "test.seed.device_authorization".into(),
                idempotency_key: "seed-device-authorization-atomic".into(),
                expected_revisions: BTreeMap::new(),
                puts: vec![
                    ControlPut {
                        tenant_id: "identity".into(),
                        record_type: authorization_key.record_type.clone(),
                        record_id: authorization_key.record_id.clone(),
                        expected_revision: Some(0),
                        value: json!({"state":"approved"}),
                    },
                    ControlPut {
                        tenant_id: "identity".into(),
                        record_type: user_code_key.record_type.clone(),
                        record_id: user_code_key.record_id.clone(),
                        expected_revision: Some(0),
                        value: json!({"authorization":"auth_atomic_success"}),
                    },
                ],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "identity".into(),
                    command_id: "test.seed.device_authorization".into(),
                    actor_id: "test".into(),
                    allowed: true,
                    detail: Value::Null,
                },
                response: success_response(),
            })
            .unwrap();

        let mut issued = issue_for_device(
            account_id.clone(),
            10_000,
            Some("device-atomic-exchange".into()),
        )
        .unwrap();
        issued.record.credential_version = Some(1);
        let access_hash = issued.record.token_hash;
        let refresh = DeviceRefreshRecord {
            account_id: account_id.clone(),
            token_hash: Sha256::digest(b"device-exchange-atomic-refresh").into(),
            access_token_hash: access_hash,
            device_id: "device-atomic-exchange".into(),
            family_id: issued.record.family_id.clone().unwrap(),
            expires_at: 20_000,
            consumed_at: None,
            credential_version: Some(1),
            refresh_replay: None,
        };
        persistence
            .create_device_session_with_account_revision_and_deletes(
                &issued.record,
                &refresh,
                Some(1),
                vec![
                    ControlDelete {
                        tenant_id: authorization_key.tenant_id.clone(),
                        record_type: authorization_key.record_type.clone(),
                        record_id: authorization_key.record_id.clone(),
                        expected_revision: Some(1),
                    },
                    ControlDelete {
                        tenant_id: user_code_key.tenant_id.clone(),
                        record_type: user_code_key.record_type.clone(),
                        record_id: user_code_key.record_id.clone(),
                        expected_revision: Some(1),
                    },
                ],
            )
            .unwrap();
        assert!(
            store
                .get(
                    &authorization_key.tenant_id,
                    &authorization_key.record_type,
                    &authorization_key.record_id,
                )
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get(
                    &user_code_key.tenant_id,
                    &user_code_key.record_type,
                    &user_code_key.record_id,
                )
                .unwrap()
                .is_none()
        );
        assert!(persistence.get(&access_hash).unwrap().is_some());
        assert!(
            persistence
                .get_device_refresh(&refresh.token_hash)
                .unwrap()
                .is_some()
        );

        let stale_authorization_key = ControlRecordKey {
            tenant_id: "identity".into(),
            record_type: "device_authorization".into(),
            record_id: "auth_atomic_rollback".into(),
        };
        store
            .execute(ExecuteControlCommand {
                tenant_id: "identity".into(),
                command_id: "test.seed.device_authorization".into(),
                idempotency_key: "seed-device-authorization-atomic-rollback".into(),
                expected_revisions: BTreeMap::new(),
                puts: vec![ControlPut {
                    tenant_id: "identity".into(),
                    record_type: stale_authorization_key.record_type.clone(),
                    record_id: stale_authorization_key.record_id.clone(),
                    expected_revision: Some(0),
                    value: json!({"state":"approved"}),
                }],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "identity".into(),
                    command_id: "test.seed.device_authorization".into(),
                    actor_id: "test".into(),
                    allowed: true,
                    detail: Value::Null,
                },
                response: success_response(),
            })
            .unwrap();

        issued.record.token_hash = Sha256::digest(b"device-exchange-atomic-rollback-access").into();
        let rollback_access_hash = issued.record.token_hash;
        let mut rollback_refresh = refresh;
        rollback_refresh.token_hash =
            Sha256::digest(b"device-exchange-atomic-rollback-refresh").into();
        rollback_refresh.access_token_hash = rollback_access_hash;
        assert_eq!(
            persistence
                .create_device_session_with_account_revision_and_deletes(
                    &issued.record,
                    &rollback_refresh,
                    Some(2),
                    vec![ControlDelete {
                        tenant_id: stale_authorization_key.tenant_id.clone(),
                        record_type: stale_authorization_key.record_type.clone(),
                        record_id: stale_authorization_key.record_id.clone(),
                        expected_revision: Some(99),
                    }],
                )
                .unwrap_err()
                .code,
            "storage_revision_conflict"
        );
        assert!(
            store
                .get(
                    &stale_authorization_key.tenant_id,
                    &stale_authorization_key.record_type,
                    &stale_authorization_key.record_id,
                )
                .unwrap()
                .is_some()
        );
        assert!(persistence.get(&rollback_access_hash).unwrap().is_none());
        assert!(
            persistence
                .get_device_refresh(&rollback_refresh.token_hash)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn rotation_is_atomic_when_replacement_conflicts() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let mut current = issue(AccountId::new("acct_rotation_atomic").unwrap(), 100)
            .unwrap()
            .record
            .clone();
        assert_eq!(persistence.create(&current).unwrap(), 1);
        let replacement = issue(current.account_id.clone(), 200)
            .unwrap()
            .record
            .clone();
        assert_eq!(persistence.create(&replacement).unwrap(), 1);
        current.revoked_at = Some(50);

        assert!(persistence.rotate(&current, 1, &replacement).is_err());
        let loaded = persistence.list().unwrap();
        let stored_current = loaded
            .iter()
            .find(|(record, _)| record.token_hash == current.token_hash)
            .unwrap();
        assert_eq!(stored_current.0.revoked_at, None);
        assert_eq!(stored_current.1, 1);
    }

    #[test]
    fn browser_session_refresh_is_fenced_by_durable_account_revision() {
        let mut store = MemoryControlRecordStore::default();
        let persistence = SessionPersistence::new(ControlPersistence::new(store.clone()));
        let account_id = AccountId::new("acct_browser_refresh_fence").unwrap();
        let account = Account::new(account_id.clone(), "2026-10-18T00:00:00Z");
        store
            .execute(ExecuteControlCommand {
                tenant_id: "control".into(),
                command_id: "account.provision".into(),
                idempotency_key: "account-browser-refresh-fence-seed".into(),
                expected_revisions: BTreeMap::new(),
                puts: vec![ControlPut {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: account_id.as_str().into(),
                    expected_revision: Some(0),
                    value: serde_json::to_value(&account).unwrap(),
                }],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "control".into(),
                    command_id: "account.provision".into(),
                    actor_id: "service".into(),
                    allowed: true,
                    detail: Value::Null,
                },
                response: success_response(),
            })
            .unwrap();

        let current = issue(account_id.clone(), 10_000).unwrap().record.clone();
        persistence
            .create_with_account_revision(&current, Some(1))
            .unwrap();

        let (persisted_account, account_revision) = store
            .get("control", "account", account_id.as_str())
            .unwrap()
            .map(|record| {
                (
                    serde_json::from_value::<Account>(record.value).unwrap(),
                    record.revision,
                )
            })
            .unwrap();
        let updated_account = persisted_account
            .advance_revision(account_revision)
            .unwrap();
        store
            .execute(ExecuteControlCommand {
                tenant_id: "control".into(),
                command_id: "account.revision.advance".into(),
                idempotency_key: "account-browser-refresh-fence-advance".into(),
                expected_revisions: BTreeMap::from([(
                    ControlRecordKey {
                        tenant_id: "control".into(),
                        record_type: "account".into(),
                        record_id: account_id.as_str().into(),
                    },
                    account_revision,
                )]),
                puts: vec![ControlPut {
                    tenant_id: "control".into(),
                    record_type: "account".into(),
                    record_id: account_id.as_str().into(),
                    expected_revision: Some(account_revision),
                    value: serde_json::to_value(updated_account).unwrap(),
                }],
                deletes: vec![],
                audit: ControlAuditEntry {
                    tenant_id: "control".into(),
                    command_id: "account.revision.advance".into(),
                    actor_id: "operator".into(),
                    allowed: true,
                    detail: Value::Null,
                },
                response: success_response(),
            })
            .unwrap();

        let mut revoked = current.clone();
        revoked.revoked_at = Some(100);
        let mut replacement = issue(account_id, 20_000).unwrap().record.clone();
        replacement.family_id = current.family_id.clone();
        assert_eq!(
            persistence
                .rotate_with_account_revision(&revoked, 1, &replacement, Some(1))
                .unwrap_err()
                .code,
            "storage_revision_conflict"
        );
        let persisted_current = persistence.get(&current.token_hash).unwrap().unwrap().0;
        assert_eq!(persisted_current.revoked_at, None);
        assert!(persistence.get(&replacement.token_hash).unwrap().is_none());
    }

    #[test]
    fn persists_only_hashed_session_and_revision() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let issued = issue(AccountId::new("acct_session").unwrap(), 100).unwrap();
        let raw = issued.raw_token.clone();
        assert_eq!(persistence.create(&issued.record).unwrap(), 1);
        let loaded = persistence.list().unwrap();
        assert_eq!(loaded, vec![(issued.record.clone(), 1)]);
        assert!(!serde_json::to_string(&loaded).unwrap().contains(&raw));
    }

    #[test]
    fn expired_session_and_family_are_removed_after_retention_window() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let issued = issue(AccountId::new("acct_expired_session").unwrap(), 100).unwrap();
        let family_id = issued.record.family_id.clone().unwrap();
        persistence.create(&issued.record).unwrap();

        assert_eq!(
            persistence
                .purge_expired(100 + SESSION_RETENTION_SECONDS)
                .unwrap(),
            2
        );
        assert!(persistence.list().unwrap().is_empty());
        assert!(
            persistence
                .store
                .load::<SessionFamilyRecord>(TENANT, FAMILY_RECORD_TYPE, &family_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn device_family_is_retained_until_its_refresh_expiry_passes() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let mut issued = issue(AccountId::new("acct_refresh_retention").unwrap(), 200).unwrap();
        issued.record.device_id = Some("desktop-device-01".into());
        let family_id = issued.record.family_id.clone().unwrap();
        let refresh = DeviceRefreshRecord {
            account_id: issued.record.account_id.clone(),
            token_hash: Sha256::digest(b"refresh-token").into(),
            access_token_hash: issued.record.token_hash,
            device_id: "desktop-device-01".into(),
            family_id: family_id.clone(),
            expires_at: 10_000,
            consumed_at: None,
            credential_version: None,
            refresh_replay: None,
        };
        persistence
            .create_device_session(&issued.record, &refresh)
            .unwrap();

        assert_eq!(
            persistence
                .purge_expired(200 + SESSION_RETENTION_SECONDS)
                .unwrap(),
            0
        );
        assert_eq!(persistence.list().unwrap().len(), 1);
        assert_eq!(
            persistence
                .purge_expired(10_000 + SESSION_RETENTION_SECONDS)
                .unwrap(),
            3
        );
        assert!(persistence.list().unwrap().is_empty());
        assert!(
            persistence
                .get_device_refresh(&refresh.token_hash)
                .unwrap()
                .is_none()
        );
        assert!(
            persistence
                .store
                .load::<SessionFamilyRecord>(TENANT, FAMILY_RECORD_TYPE, &family_id)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn device_session_and_refresh_must_share_credential_version() {
        let mut issued = issue(AccountId::new("acct_refresh_version").unwrap(), 200).unwrap();
        issued.record.device_id = Some("desktop-device-01".into());
        issued.record.credential_version = Some(2);
        let refresh = DeviceRefreshRecord {
            account_id: issued.record.account_id.clone(),
            token_hash: Sha256::digest(b"refresh-token").into(),
            access_token_hash: issued.record.token_hash,
            device_id: "desktop-device-01".into(),
            family_id: issued.record.family_id.clone().unwrap(),
            expires_at: 10_000,
            consumed_at: None,
            credential_version: Some(1),
            refresh_replay: None,
        };

        assert_eq!(
            validate_device_pair(&issued.record, &refresh)
                .unwrap_err()
                .code,
            "session_record_corrupt"
        );
    }

    #[test]
    fn device_refresh_family_must_exist_and_use_canonical_id() {
        let persistence =
            SessionPersistence::new(ControlPersistence::new(MemoryControlRecordStore::default()));
        let account_id = AccountId::new("acct_refresh_family").unwrap();

        assert_eq!(
            persistence
                .device_refresh_family_revoked_at(&"a".repeat(64), &account_id)
                .unwrap_err()
                .code,
            "session_record_corrupt"
        );
        assert!(!is_family_id(&"A".repeat(64)));
    }
}
