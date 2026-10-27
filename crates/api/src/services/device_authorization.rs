use buglogin_domain::{
    errors::Problem,
    ids::{AccountId, DeviceId},
};
use buglogin_storage::control::{
    ControlAuditEntry, ControlDelete, ControlPut, ExecuteControlCommand, HttpResponse,
};
use getrandom::fill;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write;
use zeroize::{Zeroize, Zeroizing};

use super::control_persistence::ControlPersistence;

const TENANT: &str = "identity";
const RECORD_TYPE: &str = "device_authorization";
const INDEX_TYPE: &str = "device_user_code";
const DEVICE_TTL: u64 = 600;
const DEFAULT_INTERVAL: u64 = 5;
const EXPIRED_CLEANUP_BATCH: usize = 128;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceAuthorizationRecord {
    pub device_code_hash: [u8; 32],
    pub user_code_hash: [u8; 32],
    pub device_id: String,
    pub client_name: String,
    pub account_id: Option<AccountId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_version: Option<u32>,
    pub expires_at: u64,
    pub interval: u64,
    pub next_poll_at: u64,
    pub approved_at: Option<u64>,
    pub denied_at: Option<u64>,
    pub consumed_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct UserCodeIndex {
    device_record_id: String,
}

#[derive(Clone, Eq, PartialEq)]
pub struct DeviceAuthorizationStart {
    pub device_code: String,
    pub user_code: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub(crate) struct DeviceAuthorizationConsumption {
    device_record_id: String,
    device_record_revision: u64,
    user_code_index_id: String,
    user_code_index_revision: u64,
}

impl DeviceAuthorizationConsumption {
    pub(crate) fn into_deletes(self) -> Vec<ControlDelete> {
        vec![
            ControlDelete {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: self.device_record_id,
                expected_revision: Some(self.device_record_revision),
            },
            ControlDelete {
                tenant_id: TENANT.into(),
                record_type: INDEX_TYPE.into(),
                record_id: self.user_code_index_id,
                expected_revision: Some(self.user_code_index_revision),
            },
        ]
    }
}

impl std::fmt::Debug for DeviceAuthorizationStart {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DeviceAuthorizationStart")
            .field("device_code", &"[REDACTED]")
            .field("user_code", &"[REDACTED]")
            .field("expires_in", &self.expires_in)
            .field("interval", &self.interval)
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DevicePoll {
    Pending,
    SlowDown {
        interval: u64,
    },
    Approved {
        account_id: AccountId,
        device_id: String,
        credential_version: Option<u32>,
    },
}

#[derive(Clone)]
pub struct DeviceAuthorizationService {
    persistence: ControlPersistence,
}

impl DeviceAuthorizationService {
    pub fn new(persistence: ControlPersistence) -> Self {
        Self { persistence }
    }

    pub fn start(
        &self,
        device_id: &str,
        client_name: &str,
        now: u64,
    ) -> Result<DeviceAuthorizationStart, Problem> {
        validate_device_id(device_id)?;
        let client_name = client_name.trim();
        if client_name.chars().count() > 128 {
            return Err(problem(
                "device_client_name_invalid",
                "Client name is invalid",
                400,
            ));
        }
        let (device_code, device_hash) = random_hex_code()?;
        let mut device_code = Zeroizing::new(device_code);
        let (user_code, user_hash) = random_user_code()?;
        let mut user_code = Zeroizing::new(user_code);
        let device_record_id = hash_id(&device_hash);
        let user_record_id = hash_id(&user_hash);
        let record = DeviceAuthorizationRecord {
            device_code_hash: device_hash,
            user_code_hash: user_hash,
            device_id: device_id.to_owned(),
            client_name: client_name.to_owned(),
            account_id: None,
            credential_version: None,
            expires_at: crate::clock::checked_add_seconds(now, DEVICE_TTL, "device-auth")?,
            interval: DEFAULT_INTERVAL,
            next_poll_at: now,
            approved_at: None,
            denied_at: None,
            consumed_at: None,
        };
        let index = UserCodeIndex {
            device_record_id: device_record_id.clone(),
        };
        self.persistence.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.device.start".into(),
            idempotency_key: format!("auth.device.start-{device_record_id}"),
            expected_revisions: Default::default(),
            puts: vec![
                ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: RECORD_TYPE.into(),
                    record_id: device_record_id,
                    expected_revision: Some(0),
                    value: serde_json::to_value(record).map_err(|_| corrupt())?,
                },
                ControlPut {
                    tenant_id: TENANT.into(),
                    record_type: INDEX_TYPE.into(),
                    record_id: user_record_id,
                    expected_revision: Some(0),
                    value: serde_json::to_value(index).map_err(|_| corrupt())?,
                },
            ],
            deletes: vec![],
            audit: audit(
                "auth.device.start",
                "anonymous",
                serde_json::json!({"state":"issued"}),
            ),
            response: response(),
        })?;
        Ok(DeviceAuthorizationStart {
            device_code: std::mem::take(&mut *device_code),
            user_code: std::mem::take(&mut *user_code),
            expires_in: DEVICE_TTL,
            interval: DEFAULT_INTERVAL,
        })
    }

    pub fn purge_expired_authorizations(&self, now: u64) -> Result<usize, Problem> {
        for _ in 0..2 {
            let expired = self
                .persistence
                .list_versioned::<DeviceAuthorizationRecord>(TENANT, RECORD_TYPE)?
                .into_iter()
                .filter(|(_, record, _)| record.expires_at <= now)
                .take(EXPIRED_CLEANUP_BATCH)
                .collect::<Vec<_>>();
            if expired.is_empty() {
                return Ok(0);
            }

            let mut deletes = Vec::with_capacity(expired.len() * 2);
            let mut batch_ids = Vec::with_capacity(expired.len());
            for (record_id, record, revision) in &expired {
                deletes.push(ControlDelete {
                    tenant_id: TENANT.into(),
                    record_type: RECORD_TYPE.into(),
                    record_id: record_id.clone(),
                    expected_revision: Some(*revision),
                });
                let index_id = hash_id(&record.user_code_hash);
                if let Some((index, index_revision)) = self
                    .persistence
                    .load_versioned::<UserCodeIndex>(TENANT, INDEX_TYPE, &index_id)?
                    && index.device_record_id == *record_id
                {
                    deletes.push(ControlDelete {
                        tenant_id: TENANT.into(),
                        record_type: INDEX_TYPE.into(),
                        record_id: index_id,
                        expected_revision: Some(index_revision),
                    });
                }
                batch_ids.push(record_id.as_str());
            }

            let batch_identity = format!("{}:{}", now, batch_ids.join(":"));
            let idempotency_key = format!(
                "auth.device.expired.cleanup-{}",
                hash_id(&hash(batch_identity.as_bytes()))
            );
            match self.persistence.execute(ExecuteControlCommand {
                tenant_id: TENANT.into(),
                command_id: "auth.device.expired.cleanup".into(),
                idempotency_key,
                expected_revisions: Default::default(),
                puts: vec![],
                deletes,
                audit: audit(
                    "auth.device.expired.cleanup",
                    "service",
                    serde_json::json!({"deleted": expired.len()}),
                ),
                response: response(),
            }) {
                Ok(_) => return Ok(expired.len()),
                Err(error) if error.code == "storage_revision_conflict" => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(0)
    }

    pub fn approve(&self, user_code: &str, account_id: AccountId, now: u64) -> Result<(), Problem> {
        self.approve_with_credential_version(user_code, account_id, None, now)
    }

    pub fn approve_with_credential_version(
        &self,
        user_code: &str,
        account_id: AccountId,
        credential_version: Option<u32>,
        now: u64,
    ) -> Result<(), Problem> {
        let user_hash = hash(user_code.as_bytes());
        let user_record_id = hash_id(&user_hash);
        let (_, index_revision) = self
            .persistence
            .load_versioned::<UserCodeIndex>(TENANT, INDEX_TYPE, &user_record_id)?
            .ok_or_else(|| problem("invalid_user_code", "User code is invalid", 400))?;
        let record_id = user_record_id_from_index(&self.persistence, &user_record_id)?;
        let (mut record, revision) = self
            .persistence
            .load_versioned::<DeviceAuthorizationRecord>(TENANT, RECORD_TYPE, &record_id)?
            .ok_or_else(|| problem("invalid_user_code", "User code is invalid", 400))?;
        if record.user_code_hash != user_hash || record.expires_at <= now {
            return Err(problem(
                "expired_token",
                "Device authorization expired",
                400,
            ));
        }
        if record.consumed_at.is_some() || record.denied_at.is_some() {
            return Err(problem(
                "invalid_user_code",
                "Device authorization is no longer active",
                400,
            ));
        }
        if record.approved_at.is_some() {
            if record.account_id.as_ref() == Some(&account_id)
                && record.credential_version == credential_version
            {
                // A client may lose the approval response and retry with the
                // same user code. Treat that replay as success only for the
                // account and credential version that performed the approval.
                return Ok(());
            }
            return Err(problem(
                "authorization_already_approved",
                "Device authorization already approved",
                409,
            ));
        }
        record.account_id = Some(account_id.clone());
        record.credential_version = credential_version;
        record.approved_at = Some(now);
        let result = self.persistence.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.device.approve".into(),
            idempotency_key: format!("auth.device.approve-{user_record_id}-{now}"),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: record_id.clone(),
                expected_revision: Some(revision),
                value: serde_json::to_value(record).map_err(|_| corrupt())?,
            }],
            deletes: vec![],
            audit: audit(
                "auth.device.approve",
                account_id.as_str(),
                serde_json::json!({"state":"approved"}),
            ),
            response: response(),
        });
        if let Err(error) = result {
            // A concurrent retry may have committed the same approval while
            // this request lost the revision race, or the commit response may
            // have been lost. Confirm the durable state before returning an
            // error so the same actor receives an idempotent success.
            let approved_by_same_actor = self
                .persistence
                .load_versioned::<DeviceAuthorizationRecord>(TENANT, RECORD_TYPE, &record_id)
                .ok()
                .flatten()
                .is_some_and(|(latest, _)| {
                    latest.approved_at.is_some()
                        && latest.consumed_at.is_none()
                        && latest.denied_at.is_none()
                        && latest.account_id.as_ref() == Some(&account_id)
                        && latest.credential_version == credential_version
                });
            if !approved_by_same_actor {
                return Err(error);
            }
        }
        let _ = index_revision;
        Ok(())
    }

    pub fn poll(&self, device_code: &str, now: u64) -> Result<DevicePoll, Problem> {
        self.poll_inner(device_code, now, true)
    }

    pub fn poll_for_exchange(&self, device_code: &str, now: u64) -> Result<DevicePoll, Problem> {
        self.poll_inner(device_code, now, false)
    }

    fn poll_inner(
        &self,
        device_code: &str,
        now: u64,
        consume_approved: bool,
    ) -> Result<DevicePoll, Problem> {
        let device_hash = hash(device_code.as_bytes());
        let device_record_id = hash_id(&device_hash);
        let (mut record, revision) = self
            .persistence
            .load_versioned::<DeviceAuthorizationRecord>(TENANT, RECORD_TYPE, &device_record_id)?
            .ok_or_else(|| problem("invalid_grant", "Device code is invalid", 400))?;
        if record.device_code_hash != device_hash || record.expires_at <= now {
            return Err(problem(
                "expired_token",
                "Device authorization expired",
                400,
            ));
        }
        if record.consumed_at.is_some() || record.denied_at.is_some() {
            return Err(problem(
                "invalid_grant",
                "Device authorization is no longer active",
                400,
            ));
        }
        if record.approved_at.is_none() {
            if now < record.next_poll_at {
                record.interval =
                    crate::clock::checked_add_seconds(record.interval, 5, "device-auth")?;
                record.next_poll_at =
                    crate::clock::checked_add_seconds(now, record.interval, "device-auth")?;
                let interval = record.interval;
                self.persist_poll(&device_record_id, revision, record)?;
                return Ok(DevicePoll::SlowDown { interval });
            }
            record.next_poll_at =
                crate::clock::checked_add_seconds(now, record.interval, "device-auth")?;
            self.persist_poll(&device_record_id, revision, record)?;
            return Ok(DevicePoll::Pending);
        }
        let account_id = record.account_id.clone().ok_or_else(corrupt)?;
        if consume_approved {
            self.consume_approved_record(device_record_id, record.clone(), revision)?;
        }
        Ok(DevicePoll::Approved {
            account_id,
            device_id: record.device_id,
            credential_version: record.credential_version,
        })
    }

    pub fn consume_approved_for_exchange(
        &self,
        device_code: &str,
        account_id: &AccountId,
        device_id: &str,
        credential_version: Option<u32>,
        now: u64,
    ) -> Result<(), Problem> {
        let consumption = self.prepare_approved_exchange_consumption(
            device_code,
            account_id,
            device_id,
            credential_version,
            now,
        )?;
        let idempotency_key = format!("auth.device.consume-{}", consumption.device_record_id);
        self.persistence.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.device.consume".into(),
            idempotency_key,
            expected_revisions: Default::default(),
            puts: vec![],
            deletes: consumption.into_deletes(),
            audit: audit(
                "auth.device.consume",
                account_id.as_str(),
                serde_json::json!({"state":"consumed"}),
            ),
            response: response(),
        })?;
        Ok(())
    }

    pub(crate) fn prepare_approved_exchange_consumption(
        &self,
        device_code: &str,
        account_id: &AccountId,
        device_id: &str,
        credential_version: Option<u32>,
        now: u64,
    ) -> Result<DeviceAuthorizationConsumption, Problem> {
        let device_hash = hash(device_code.as_bytes());
        let device_record_id = hash_id(&device_hash);
        let (record, revision) = self
            .persistence
            .load_versioned::<DeviceAuthorizationRecord>(TENANT, RECORD_TYPE, &device_record_id)?
            .ok_or_else(|| problem("invalid_grant", "Device code is invalid", 400))?;
        if record.device_code_hash != device_hash || record.expires_at <= now {
            return Err(problem(
                "expired_token",
                "Device authorization expired",
                400,
            ));
        }
        if record.approved_at.is_none()
            || record.denied_at.is_some()
            || record.account_id.as_ref() != Some(account_id)
            || record.device_id != device_id
            || record.credential_version != credential_version
        {
            return Err(problem(
                "invalid_grant",
                "Device authorization is no longer valid",
                400,
            ));
        }
        if record.consumed_at.is_some() {
            return Err(problem(
                "invalid_grant",
                "Device authorization is no longer active",
                400,
            ));
        }
        let user_code_index_id = hash_id(&record.user_code_hash);
        let (index, user_code_index_revision) = self
            .persistence
            .load_versioned::<UserCodeIndex>(TENANT, INDEX_TYPE, &user_code_index_id)?
            .ok_or_else(corrupt)?;
        if index.device_record_id != device_record_id {
            return Err(corrupt());
        }
        Ok(DeviceAuthorizationConsumption {
            device_record_id,
            device_record_revision: revision,
            user_code_index_id,
            user_code_index_revision,
        })
    }

    fn consume_approved_record(
        &self,
        device_record_id: String,
        record: DeviceAuthorizationRecord,
        revision: u64,
    ) -> Result<(), Problem> {
        let account_id = record.account_id.clone().ok_or_else(corrupt)?;
        let index_id = hash_id(&record.user_code_hash);
        let (index, index_revision) = self
            .persistence
            .load_versioned::<UserCodeIndex>(TENANT, INDEX_TYPE, &index_id)?
            .ok_or_else(corrupt)?;
        if index.device_record_id != device_record_id {
            return Err(corrupt());
        }
        let consume_nonce = random_idempotency_nonce()?;
        self.persistence.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.device.consume".into(),
            idempotency_key: format!("auth.device.consume-{device_record_id}-{consume_nonce}"),
            expected_revisions: Default::default(),
            puts: vec![],
            deletes: vec![
                ControlDelete {
                    tenant_id: TENANT.into(),
                    record_type: RECORD_TYPE.into(),
                    record_id: device_record_id,
                    expected_revision: Some(revision),
                },
                ControlDelete {
                    tenant_id: TENANT.into(),
                    record_type: INDEX_TYPE.into(),
                    record_id: index_id,
                    expected_revision: Some(index_revision),
                },
            ],
            audit: audit(
                "auth.device.consume",
                account_id.as_str(),
                serde_json::json!({"state":"consumed"}),
            ),
            response: response(),
        })?;
        Ok(())
    }

    fn persist_poll(
        &self,
        device_record_id: &str,
        revision: u64,
        record: DeviceAuthorizationRecord,
    ) -> Result<(), Problem> {
        self.persistence.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.device.poll".into(),
            idempotency_key: format!(
                "auth.device.poll-{device_record_id}-{}",
                record.next_poll_at
            ),
            expected_revisions: Default::default(),
            puts: vec![ControlPut {
                tenant_id: TENANT.into(),
                record_type: RECORD_TYPE.into(),
                record_id: device_record_id.to_owned(),
                expected_revision: Some(revision),
                value: serde_json::to_value(record).map_err(|_| corrupt())?,
            }],
            deletes: vec![],
            audit: audit(
                "auth.device.poll",
                "anonymous",
                serde_json::json!({"state":"polled"}),
            ),
            response: response(),
        })?;
        Ok(())
    }
}

fn user_record_id_from_index(
    persistence: &ControlPersistence,
    user_record_id: &str,
) -> Result<String, Problem> {
    persistence
        .load::<UserCodeIndex>(TENANT, INDEX_TYPE, user_record_id)?
        .map(|index| index.device_record_id)
        .ok_or_else(|| problem("invalid_user_code", "User code is invalid", 400))
}

fn random_hex_code() -> Result<(String, [u8; 32]), Problem> {
    let mut bytes = [0u8; 32];
    if fill(&mut bytes).is_err() {
        bytes.zeroize();
        return Err(problem(
            "identity_entropy_unavailable",
            "Identity unavailable",
            503,
        ));
    }
    let code = hex(&bytes);
    bytes.zeroize();
    let code_hash = hash(code.as_bytes());
    Ok((code, code_hash))
}

fn random_user_code() -> Result<(String, [u8; 32]), Problem> {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
    let mut bytes = [0u8; 8];
    if fill(&mut bytes).is_err() {
        bytes.zeroize();
        return Err(problem(
            "identity_entropy_unavailable",
            "Identity unavailable",
            503,
        ));
    }
    let mut code = String::with_capacity(9);
    for (index, byte) in bytes.iter().enumerate() {
        if index == 4 {
            code.push('-');
        }
        code.push(ALPHABET[(*byte as usize) % ALPHABET.len()] as char);
    }
    bytes.zeroize();
    let code_hash = hash(code.as_bytes());
    Ok((code, code_hash))
}

fn random_idempotency_nonce() -> Result<String, Problem> {
    let mut bytes = [0u8; 16];
    fill(&mut bytes)
        .map_err(|_| problem("identity_entropy_unavailable", "Identity unavailable", 503))?;
    Ok(hex(&bytes))
}

fn hash(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn hash_id(value: &[u8; 32]) -> String {
    let mut result = String::with_capacity(64);
    for byte in value {
        let _ = write!(result, "{byte:02x}");
    }
    result
}

fn validate_device_id(value: &str) -> Result<(), Problem> {
    DeviceId::new(value)
        .map(|_| ())
        .map_err(|_| problem("device_id_invalid", "Device ID is invalid", 400))
}

fn audit(command_id: &str, actor_id: &str, detail: serde_json::Value) -> ControlAuditEntry {
    ControlAuditEntry {
        tenant_id: TENANT.into(),
        command_id: command_id.into(),
        actor_id: actor_id.into(),
        allowed: true,
        detail,
    }
}

fn response() -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: Default::default(),
        body: serde_json::Value::Null,
    }
}
fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "identity")
}
fn corrupt() -> Problem {
    Problem::new(
        "device_authorization_corrupt",
        "Device authorization is corrupt",
        500,
        "identity",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::control_persistence::ControlPersistence;
    use buglogin_storage::control::MemoryControlRecordStore;

    fn service() -> DeviceAuthorizationService {
        DeviceAuthorizationService::new(
            ControlPersistence::new(MemoryControlRecordStore::default()),
        )
    }

    #[test]
    fn client_name_limit_matches_openapi_character_length() {
        let service = service();
        assert!(service.start("device-1234", &"é".repeat(128), 100).is_ok());
        assert_eq!(
            service
                .start("device-5678", &"é".repeat(129), 100)
                .expect_err("129-character client name must be rejected")
                .code,
            "device_client_name_invalid"
        );
    }

    #[test]
    fn starting_device_flow_purges_expired_authorization_and_code_index() {
        let service = service();
        let expired = service.start("device-1234", "Desktop", 100).unwrap();
        assert_eq!(
            service
                .persistence
                .list_versioned::<DeviceAuthorizationRecord>(TENANT, RECORD_TYPE)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            service
                .persistence
                .list_versioned::<UserCodeIndex>(TENANT, INDEX_TYPE)
                .unwrap()
                .len(),
            1
        );

        assert_eq!(service.purge_expired_authorizations(700).unwrap(), 1);
        let current = service.start("device-5678", "Desktop", 700).unwrap();
        assert_eq!(
            service
                .persistence
                .list_versioned::<DeviceAuthorizationRecord>(TENANT, RECORD_TYPE)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            service
                .persistence
                .list_versioned::<UserCodeIndex>(TENANT, INDEX_TYPE)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            service.poll(&expired.device_code, 700).unwrap_err().code,
            "invalid_grant"
        );
        assert!(service.poll(&current.device_code, 700).is_ok());
    }

    #[test]
    fn start_approve_poll_is_single_use_and_persists_only_hashes() {
        let service = service();
        let started = service.start("device-1234", "Desktop", 100).unwrap();
        let persisted = serde_json::to_string(
            &service
                .persistence
                .list_versioned::<DeviceAuthorizationRecord>(TENANT, RECORD_TYPE)
                .unwrap(),
        )
        .unwrap();
        assert!(!persisted.contains(&started.device_code));
        assert!(!persisted.contains(&started.user_code));
        assert!(!started.device_code.is_empty());
        assert_eq!(
            service.poll(&started.device_code, 100).unwrap(),
            DevicePoll::Pending
        );
        service
            .approve_with_credential_version(
                &started.user_code,
                AccountId::new("acct_device").unwrap(),
                Some(7),
                101,
            )
            .unwrap();
        let approved = service.poll(&started.device_code, 105).unwrap();
        assert_eq!(
            approved,
            DevicePoll::Approved {
                account_id: AccountId::new("acct_device").unwrap(),
                device_id: "device-1234".into(),
                credential_version: Some(7),
            }
        );
        assert_eq!(
            service.poll(&started.device_code, 106).unwrap_err().code,
            "invalid_grant"
        );
        assert!(
            service
                .persistence
                .list_versioned::<DeviceAuthorizationRecord>(TENANT, RECORD_TYPE)
                .unwrap()
                .is_empty()
        );
        assert!(
            service
                .persistence
                .list_versioned::<UserCodeIndex>(TENANT, INDEX_TYPE)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn early_poll_slows_down_and_survives_restart_store() {
        let store = MemoryControlRecordStore::default();
        let persistence = ControlPersistence::new(store);
        let service = DeviceAuthorizationService::new(persistence.clone());
        let started = service.start("device-5678", "Desktop", 100).unwrap();
        assert_eq!(
            service.poll(&started.device_code, 101).unwrap(),
            DevicePoll::Pending
        );
        assert_eq!(
            service.poll(&started.device_code, 102).unwrap(),
            DevicePoll::SlowDown { interval: 10 }
        );
        let reopened = DeviceAuthorizationService::new(persistence);
        assert_eq!(
            reopened.poll(&started.device_code, 103).unwrap(),
            DevicePoll::SlowDown { interval: 15 }
        );
    }

    #[test]
    fn simultaneous_exchange_consumes_authorization_once() {
        let service = service();
        let started = service.start("device-race", "Desktop", 100).unwrap();
        service
            .approve(
                &started.user_code,
                AccountId::new("acct_race").unwrap(),
                101,
            )
            .unwrap();
        let first = service.clone();
        let second = service;
        std::thread::scope(|scope| {
            let left = scope.spawn(|| first.poll(&started.device_code, 105));
            let right = scope.spawn(|| second.poll(&started.device_code, 105));
            let results = [left.join().unwrap(), right.join().unwrap()];
            assert_eq!(
                results
                    .iter()
                    .filter(|result| matches!(result, Ok(DevicePoll::Approved { .. })))
                    .count(),
                1
            );
            assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        });
    }
}
