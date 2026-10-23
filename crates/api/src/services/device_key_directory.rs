use std::collections::BTreeSet;

use buglogin_domain::{
    errors::Problem,
    ids::{AccountId, DeviceId},
};
use buglogin_storage::control::{
    ControlAuditEntry, ControlPut, ExecuteControlCommand, HttpResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::control_persistence::ControlPersistence;

const TENANT: &str = "identity";
const RECORD_TYPE: &str = "device_encryption_key";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceEncryptionKey {
    pub account_id: AccountId,
    pub device_id: String,
    pub schema_version: u8,
    pub public_key: String,
    pub fingerprint: String,
    pub registered_at: u64,
}

#[derive(Clone)]
pub struct DeviceKeyDirectoryService {
    persistence: ControlPersistence,
}

impl DeviceKeyDirectoryService {
    pub fn new(persistence: ControlPersistence) -> Self {
        Self { persistence }
    }

    pub fn register(
        &self,
        account_id: &AccountId,
        device_id: &str,
        public_key_hex: &str,
        idempotency_key: &str,
        now: u64,
    ) -> Result<DeviceEncryptionKey, Problem> {
        validate_device_id(device_id)?;
        if !(16..=128).contains(&idempotency_key.len()) {
            return Err(problem(
                "idempotency_key_required",
                "Valid idempotency key required",
                400,
            ));
        }
        let (record, put) =
            self.prepare_registration(account_id, device_id, public_key_hex, now)?;
        let Some(put) = put else {
            return Ok(record);
        };
        let record_id = put.record_id.clone();
        let fingerprint = record.fingerprint.clone();
        let result = self.persistence.execute(ExecuteControlCommand {
            tenant_id: TENANT.into(),
            command_id: "auth.device.encryptionkey.register".into(),
            idempotency_key: format!(
                "device-key-register-{}",
                hex(&Sha256::digest(
                    format!("{}:{device_id}:{idempotency_key}", account_id.as_str()).as_bytes()
                ))
            ),
            expected_revisions: Default::default(),
            puts: vec![put],
            deletes: vec![],
            audit: ControlAuditEntry {
                tenant_id: TENANT.into(),
                command_id: "auth.device.encryptionkey.register".into(),
                actor_id: account_id.as_str().into(),
                allowed: true,
                detail: json!({
                    "deviceId": device_id,
                    "fingerprint": fingerprint,
                    "state": "registered"
                }),
            },
            response: HttpResponse {
                status: 201,
                headers: Default::default(),
                body: Value::Null,
            },
        });
        match result {
            Ok(_) => Ok(record),
            Err(error) if error.code == "storage_revision_conflict" => {
                let Some((existing, _)) = self.persistence.load_versioned::<DeviceEncryptionKey>(
                    TENANT,
                    RECORD_TYPE,
                    &record_id,
                )?
                else {
                    return Err(error);
                };
                if existing.account_id == *account_id
                    && existing.device_id == device_id
                    && existing.public_key == record.public_key
                {
                    Ok(existing)
                } else {
                    Err(problem(
                        "device_encryption_key_conflict",
                        "This device ID is already bound to a different encryption key and cannot be silently replaced",
                        409,
                    ))
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Validates a device enrollment and returns the immutable record plus the
    /// conditional write needed to persist it. Callers that own a larger
    /// control mutation can commit this put atomically with related state.
    pub fn prepare_registration(
        &self,
        account_id: &AccountId,
        device_id: &str,
        public_key_hex: &str,
        now: u64,
    ) -> Result<(DeviceEncryptionKey, Option<ControlPut>), Problem> {
        validate_device_id(device_id)?;
        let public_key = decode_public_key(public_key_hex)?;
        let record_id = record_id(account_id, device_id);
        if let Some((existing, _)) = self.persistence.load_versioned::<DeviceEncryptionKey>(
            TENANT,
            RECORD_TYPE,
            &record_id,
        )? {
            if existing.account_id != *account_id || existing.device_id != device_id {
                return Err(corrupt());
            }
            if existing.public_key == public_key_hex.to_ascii_lowercase() {
                return Ok((existing, None));
            }
            return Err(problem(
                "device_encryption_key_conflict",
                "This device ID is already bound to a different encryption key and cannot be silently replaced",
                409,
            ));
        }

        let record = DeviceEncryptionKey {
            account_id: account_id.clone(),
            device_id: device_id.to_owned(),
            schema_version: 1,
            public_key: hex(&public_key),
            fingerprint: hex(&Sha256::digest(public_key)),
            registered_at: now,
        };
        let put = ControlPut {
            tenant_id: TENANT.into(),
            record_type: RECORD_TYPE.into(),
            record_id,
            expected_revision: Some(0),
            value: serde_json::to_value(&record).map_err(|_| corrupt())?,
        };
        Ok((record, Some(put)))
    }

    pub fn list_for_account(
        &self,
        account_id: &AccountId,
        active_device_ids: &BTreeSet<String>,
    ) -> Result<Vec<DeviceEncryptionKey>, Problem> {
        self.persistence
            .list_versioned_for_account::<DeviceEncryptionKey>(
                TENANT,
                RECORD_TYPE,
                account_id.as_str(),
            )?
            .into_iter()
            .filter_map(|(record_key, record, _)| {
                if record.account_id != *account_id {
                    return None;
                }
                Some((record_key, record))
            })
            .map(|(stored_id, record)| {
                let public_key = decode_public_key(&record.public_key)?;
                if stored_id != record_id(account_id, &record.device_id)
                    || DeviceId::new(record.device_id.clone()).is_err()
                    || record.schema_version != 1
                    || record.fingerprint != hex(&Sha256::digest(public_key))
                {
                    return Err(corrupt());
                }
                Ok(record)
            })
            .filter(|record| match record {
                Ok(record) => active_device_ids.contains(&record.device_id),
                Err(_) => true,
            })
            .collect()
    }
}

fn validate_device_id(value: &str) -> Result<(), Problem> {
    DeviceId::new(value)
        .map(|_| ())
        .map_err(|_| problem("device_id_invalid", "Device ID is invalid", 400))
}

fn decode_public_key(value: &str) -> Result<[u8; 32], Problem> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(problem(
            "device_encryption_key_invalid",
            "Device encryption public key must be 32-byte hexadecimal",
            400,
        ));
    }
    let mut decoded = [0_u8; 32];
    for (index, byte) in decoded.iter_mut().enumerate() {
        let start = index * 2;
        *byte =
            (hex_digit(value.as_bytes()[start])? << 4) | hex_digit(value.as_bytes()[start + 1])?;
    }
    Ok(decoded)
}

fn hex_digit(value: u8) -> Result<u8, Problem> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        b'A'..=b'F' => Ok(value - b'A' + 10),
        _ => Err(problem(
            "device_encryption_key_invalid",
            "Device encryption public key must be 32-byte hexadecimal",
            400,
        )),
    }
}

fn record_id(account_id: &AccountId, device_id: &str) -> String {
    format!(
        "device-key-{}",
        hex(&Sha256::digest(
            format!("{}:{device_id}", account_id.as_str()).as_bytes()
        ))
    )
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    output
}

fn problem(code: &'static str, message: &'static str, status: u16) -> Problem {
    Problem::new(code, message, status, "identity")
}

fn corrupt() -> Problem {
    problem(
        "device_encryption_key_corrupt",
        "Device encryption key record is corrupt",
        500,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_storage::control::MemoryControlRecordStore;

    fn service() -> DeviceKeyDirectoryService {
        DeviceKeyDirectoryService::new(ControlPersistence::new(MemoryControlRecordStore::default()))
    }

    #[test]
    fn registration_is_account_device_bound_idempotent_and_audited() {
        let service = service();
        let account = AccountId::new("acct_device_key_test").unwrap();
        let key = "ab".repeat(32);
        let registered = service
            .register(&account, "device_one", &key, "registration-idem-0001", 100)
            .unwrap();
        let other_account = AccountId::new("acct_other_device_key").unwrap();
        let other_registered = service
            .register(
                &other_account,
                "other_device",
                &"cd".repeat(32),
                "registration-idem-other-01",
                100,
            )
            .unwrap();
        assert_eq!(registered.public_key, key);
        assert_eq!(registered.fingerprint.len(), 64);
        assert_eq!(
            service
                .list_for_account(&account, &BTreeSet::from(["device_one".into()]))
                .unwrap(),
            vec![registered.clone()]
        );
        assert!(
            service
                .list_for_account(&account, &BTreeSet::new())
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            service
                .register(&account, "device_one", &key, "registration-idem-0002", 200)
                .unwrap(),
            registered
        );
        assert_eq!(
            service
                .register(
                    &account,
                    "device_one",
                    &"cd".repeat(32),
                    "registration-idem-0003",
                    300,
                )
                .unwrap_err()
                .code,
            "device_encryption_key_conflict"
        );
        assert_eq!(
            service
                .list_for_account(&other_account, &BTreeSet::from(["other_device".into()]))
                .unwrap(),
            vec![other_registered]
        );
    }

    #[test]
    fn rejects_malformed_public_key_and_device_id() {
        let service = service();
        let account = AccountId::new("acct_device_key_invalid").unwrap();
        assert_eq!(
            service
                .register(
                    &account,
                    "device_one",
                    "not-a-key",
                    "registration-idem-0001",
                    100,
                )
                .unwrap_err()
                .code,
            "device_encryption_key_invalid"
        );
        assert_eq!(
            service
                .register(
                    &account,
                    "bad/device",
                    &"ab".repeat(32),
                    "registration-idem-0001",
                    100,
                )
                .unwrap_err()
                .code,
            "device_id_invalid"
        );
    }
}
