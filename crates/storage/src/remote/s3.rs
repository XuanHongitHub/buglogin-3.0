use std::fmt;
use std::{env, io, sync::Mutex};

use buglogin_domain::errors::Problem;
use http::{HeaderMap, HeaderName, HeaderValue};
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;
use zeroize::{Zeroize, Zeroizing};

use super::{
    ConcurrentRemoteChunkStore, MAX_REMOTE_OBJECT_BYTES, RemoteChunk, RemoteChunkStore,
    checksum_problem, validate_object_key, validate_remote_object_size,
};
use crate::blob::checksum;

const CHECKSUM_METADATA: &str = "sha256";
const S3_METADATA_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const S3_TRANSFER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

struct BoundedRemoteObjectWriter {
    bytes: Vec<u8>,
    exceeded_limit: bool,
    maximum_bytes: usize,
}

impl BoundedRemoteObjectWriter {
    fn new(maximum_bytes: usize) -> Self {
        Self {
            bytes: Vec::new(),
            exceeded_limit: false,
            maximum_bytes,
        }
    }
}

impl io::Write for BoundedRemoteObjectWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() > self.maximum_bytes.saturating_sub(self.bytes.len()) {
            self.exceeded_limit = true;
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "remote object exceeds configured size limit",
            ));
        }
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Clone)]
pub struct S3Config {
    endpoint: String,
    bucket: String,
    region: String,
    tenant_id: String,
    prefix: String,
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
    path_style: bool,
}

impl S3Config {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        region: impl Into<String>,
        tenant_id: impl Into<String>,
        prefix: impl Into<String>,
        access_key: impl Into<String>,
        secret_key: impl Into<String>,
        session_token: Option<String>,
        path_style: bool,
    ) -> Result<Self, Problem> {
        let config = Self {
            endpoint: endpoint.into(),
            bucket: bucket.into(),
            region: region.into(),
            tenant_id: tenant_id.into(),
            prefix: prefix.into().trim_matches('/').to_owned(),
            access_key: access_key.into(),
            secret_key: secret_key.into(),
            session_token,
            path_style,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn from_env() -> Result<Self, Problem> {
        let endpoint = required_env("BUGLOGIN_S3_ENDPOINT")?;
        let bucket = required_env("BUGLOGIN_S3_BUCKET")?;
        let region = env::var("BUGLOGIN_S3_REGION").unwrap_or_else(|_| "us-east-1".to_owned());
        let tenant_id = required_env("BUGLOGIN_TENANT_ID")?;
        let prefix = env::var("BUGLOGIN_S3_PREFIX").unwrap_or_else(|_| "buglogin/v1".to_owned());
        let access_key = Zeroizing::new(required_env("BUGLOGIN_S3_ACCESS_KEY")?);
        let secret_key = Zeroizing::new(required_env("BUGLOGIN_S3_SECRET_KEY")?);
        let session_token = env::var("BUGLOGIN_S3_SESSION_TOKEN")
            .ok()
            .map(Zeroizing::new);
        let path_style = env::var("BUGLOGIN_S3_PATH_STYLE")
            .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"));
        Self::new(
            endpoint,
            bucket,
            region,
            tenant_id,
            prefix,
            access_key.as_str().to_owned(),
            secret_key.as_str().to_owned(),
            session_token
                .as_ref()
                .map(|token| token.as_str().to_owned()),
            path_style,
        )
    }

    pub fn namespaced_key(&self, object_key: &str) -> Result<String, Problem> {
        validate_object_key(object_key)?;
        Ok(format!(
            "{}/tenants/{}/{}",
            self.prefix, self.tenant_id, object_key
        ))
    }

    fn validate(&self) -> Result<(), Problem> {
        let endpoint = url::Url::parse(&self.endpoint);
        if self.endpoint.contains(['\r', '\n'])
            || endpoint.is_err()
            || endpoint.as_ref().is_ok_and(|url| {
                url.scheme() != "https"
                    || url.host_str().is_none_or(str::is_empty)
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                    || endpoint_authority_has_userinfo(&self.endpoint)
            })
            || self.bucket.is_empty()
            || self.region.is_empty()
            || self.access_key.is_empty()
            || self.secret_key.is_empty()
            || !valid_segment(&self.tenant_id)
            || self.prefix.is_empty()
            || self.prefix.split('/').any(|part| !valid_segment(part))
        {
            return Err(Problem::new(
                "invalid_s3_config",
                "S3 configuration is invalid or TLS is missing",
                400,
                "storage",
            ));
        }
        Ok(())
    }

    fn zeroize_credentials(&mut self) {
        self.access_key.zeroize();
        self.secret_key.zeroize();
        if let Some(session_token) = &mut self.session_token {
            session_token.zeroize();
        }
    }
}

impl Drop for S3Config {
    fn drop(&mut self) {
        self.zeroize_credentials();
    }
}

impl fmt::Debug for S3Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Config")
            .field("endpoint", &"<redacted>")
            .field("bucket", &self.bucket)
            .field("region", &self.region)
            .field("tenant_id", &self.tenant_id)
            .field("prefix", &self.prefix)
            .field("access_key", &"<redacted>")
            .field("secret_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .field("path_style", &self.path_style)
            .finish()
    }
}

fn endpoint_authority_has_userinfo(endpoint: &str) -> bool {
    endpoint
        .split_once("://")
        .and_then(|(_, remainder)| remainder.split(['/', '?', '#']).next())
        .is_some_and(|authority| authority.contains('@'))
}

pub struct S3RemoteChunkStore {
    metadata_bucket: Box<Bucket>,
    transfer_bucket: Box<Bucket>,
    config: S3Config,
    object_locks: [Mutex<()>; 128],
}

impl S3RemoteChunkStore {
    pub fn new(mut config: S3Config) -> Result<Self, Problem> {
        let region = Region::Custom {
            region: config.region.clone(),
            endpoint: config.endpoint.clone(),
        };
        let credentials = Credentials::new(
            Some(&config.access_key),
            Some(&config.secret_key),
            config.session_token.as_deref(),
            None,
            None,
        )
        .map_err(|_| unavailable())?;
        let bucket = Bucket::new(&config.bucket, region, credentials).map_err(|_| unavailable())?;
        let bucket = if config.path_style {
            bucket.with_path_style()
        } else {
            bucket
        };
        let metadata_bucket = bucket
            .with_request_timeout(S3_METADATA_TIMEOUT)
            .map_err(|_| unavailable())?;
        let transfer_bucket = bucket
            .with_request_timeout(S3_TRANSFER_TIMEOUT)
            .map_err(|_| unavailable())?;
        config.zeroize_credentials();
        Ok(Self {
            metadata_bucket,
            transfer_bucket,
            config,
            object_locks: std::array::from_fn(|_| Mutex::new(())),
        })
    }
}

impl S3RemoteChunkStore {
    fn store_readiness(&self) -> Result<(), Problem> {
        let key = self.config.namespaced_key("buglogin-readiness-probe")?;
        match self.metadata_bucket.head_object(&key) {
            Ok((_, 200..=299 | 404)) => Ok(()),
            Ok(_) | Err(_) => Err(unavailable()),
        }
    }

    fn store_checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        let _lock = self.lock_object(object_key)?;
        self.read_checksum(object_key)
    }

    fn read_checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        let key = self.config.namespaced_key(object_key)?;
        match self.metadata_bucket.head_object(&key) {
            Ok((_, 404)) => Ok(None),
            Ok((head, 200..=299)) => head
                .metadata
                .and_then(|metadata| metadata.get(CHECKSUM_METADATA).cloned())
                .map(Some)
                .ok_or_else(checksum_problem),
            Ok(_) | Err(_) => Err(unavailable()),
        }
    }

    fn store_put(
        &self,
        object_key: &str,
        bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        let _lock = self.lock_object(object_key)?;
        let key = self.config.namespaced_key(object_key)?;
        validate_remote_object_size(bytes.len() as u64)?;
        if checksum(bytes) != expected_checksum {
            return Err(checksum_problem());
        }
        match self.read_checksum(object_key)? {
            Some(existing) if existing == expected_checksum => return Ok(()),
            Some(_) => {
                return Err(Problem::new(
                    "remote_chunk_conflict",
                    "Remote chunk conflict",
                    409,
                    "storage",
                ));
            }
            None => {}
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-amz-meta-sha256"),
            HeaderValue::from_str(expected_checksum).map_err(|_| checksum_problem())?,
        );
        headers.insert(
            HeaderName::from_static("if-none-match"),
            HeaderValue::from_static("*"),
        );
        let response = self
            .transfer_bucket
            .put_object_with_content_type_and_headers(
                &key,
                bytes,
                "application/octet-stream",
                Some(headers),
            )
            .map_err(|_| unavailable())?;
        if matches!(response.status_code(), 409 | 412) {
            return match self.read_checksum(object_key)? {
                Some(stored) if stored == expected_checksum => Ok(()),
                Some(_) => Err(Problem::new(
                    "remote_chunk_conflict",
                    "Remote chunk conflict",
                    409,
                    "storage",
                )),
                None => Err(unavailable()),
            };
        }
        if !(200..=299).contains(&response.status_code()) {
            return Err(unavailable());
        }
        match self.read_checksum(object_key)? {
            Some(stored) if stored == expected_checksum => Ok(()),
            _ => Err(checksum_problem()),
        }
    }

    fn store_get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
        let _lock = self.lock_object(object_key)?;
        let key = self.config.namespaced_key(object_key)?;
        let mut body = BoundedRemoteObjectWriter::new(MAX_REMOTE_OBJECT_BYTES as usize);
        let status = match self.transfer_bucket.get_object_to_writer(&key, &mut body) {
            Ok(status) => status,
            Err(_) if body.exceeded_limit => {
                return Err(Problem::new(
                    "remote_chunk_too_large",
                    "Remote object exceeds the configured size limit",
                    502,
                    "storage",
                ));
            }
            Err(_) => return Err(unavailable()),
        };
        if status == 404 {
            return Err(Problem::new(
                "remote_chunk_not_found",
                "Remote chunk not found",
                404,
                "storage",
            ));
        }
        if !(200..=299).contains(&status) {
            return Err(unavailable());
        }
        if checksum(&body.bytes) != expected_checksum {
            return Err(checksum_problem());
        }
        Ok(RemoteChunk {
            bytes: body.bytes,
            checksum: expected_checksum.to_owned(),
        })
    }

    fn store_delete_if_checksum(
        &self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<bool, Problem> {
        let _lock = self.lock_object(object_key)?;
        let key = self.config.namespaced_key(object_key)?;
        let Some(existing) = self.read_checksum(object_key)? else {
            return Ok(false);
        };
        if existing != expected_checksum {
            return Err(Problem::new(
                "remote_chunk_conflict",
                "Remote chunk conflict",
                409,
                "storage",
            ));
        }
        let response = self
            .metadata_bucket
            .delete_object(&key)
            .map_err(|_| unavailable())?;
        if response.status_code() == 404 {
            return Ok(false);
        }
        if !(200..=299).contains(&response.status_code()) {
            return Err(unavailable());
        }
        Ok(true)
    }

    fn lock_object(&self, object_key: &str) -> Result<std::sync::MutexGuard<'_, ()>, Problem> {
        let index = object_lock_index(object_key);
        self.object_locks[index].lock().map_err(|_| unavailable())
    }
}

fn object_lock_index(object_key: &str) -> usize {
    let hash = object_key
        .bytes()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    (hash as usize) % 128
}

impl RemoteChunkStore for S3RemoteChunkStore {
    fn readiness(&self) -> Result<(), Problem> {
        self.store_readiness()
    }

    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        self.store_checksum(object_key)
    }

    fn put(
        &mut self,
        object_key: &str,
        bytes: &[u8],
        expected_checksum: &str,
    ) -> Result<(), Problem> {
        self.store_put(object_key, bytes, expected_checksum)
    }

    fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
        self.store_get(object_key, expected_checksum)
    }

    fn delete_if_checksum(
        &mut self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<bool, Problem> {
        self.store_delete_if_checksum(object_key, expected_checksum)
    }
}

impl ConcurrentRemoteChunkStore for S3RemoteChunkStore {
    fn readiness(&self) -> Result<(), Problem> {
        self.store_readiness()
    }

    fn checksum(&self, object_key: &str) -> Result<Option<String>, Problem> {
        self.store_checksum(object_key)
    }

    fn put(&self, object_key: &str, bytes: &[u8], expected_checksum: &str) -> Result<(), Problem> {
        self.store_put(object_key, bytes, expected_checksum)
    }

    fn get(&self, object_key: &str, expected_checksum: &str) -> Result<RemoteChunk, Problem> {
        self.store_get(object_key, expected_checksum)
    }

    fn delete_if_checksum(
        &self,
        object_key: &str,
        expected_checksum: &str,
    ) -> Result<bool, Problem> {
        self.store_delete_if_checksum(object_key, expected_checksum)
    }
}

fn required_env(name: &str) -> Result<String, Problem> {
    env::var(name).map_err(|_| {
        Problem::new(
            "missing_s3_config",
            "S3 configuration is missing",
            500,
            "storage",
        )
    })
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        && value != "."
        && value != ".."
}

fn unavailable() -> Problem {
    Problem::new(
        "remote_chunk_store_unavailable",
        "Remote chunk store is unavailable",
        503,
        "storage",
    )
    .retryable(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn bounded_remote_object_writer_rejects_over_limit_without_growing_buffer() {
        let mut writer = BoundedRemoteObjectWriter::new(1);
        let result = writer.write_all(&[1, 2]);
        assert!(result.is_err());
        assert!(writer.exceeded_limit);
        assert!(writer.bytes.is_empty());
    }

    fn config() -> S3Config {
        S3Config::new(
            "https://objects.example.test",
            "chunks",
            "us-east-1",
            "tenant-a",
            "buglogin/v1",
            "access-canary",
            "secret-canary",
            None,
            true,
        )
        .unwrap()
    }

    #[test]
    fn tenant_namespace_is_mandatory() {
        assert_eq!(
            config().namespaced_key("workspace/profile/chunk").unwrap(),
            "buglogin/v1/tenants/tenant-a/workspace/profile/chunk"
        );
    }

    #[test]
    fn config_requires_https_and_redacts_credentials() {
        assert!(
            S3Config::new("http://localhost", "b", "r", "t", "p", "a", "s", None, true).is_err()
        );
        assert!(
            S3Config::new(
                "https://user:pass@objects.example.test",
                "b",
                "r",
                "t",
                "p",
                "a",
                "s",
                None,
                true,
            )
            .is_err()
        );
        assert!(
            S3Config::new(
                "https://@objects.example.test",
                "b",
                "r",
                "t",
                "p",
                "a",
                "s",
                None,
                true,
            )
            .is_err()
        );
        assert!(
            S3Config::new(
                "https://objects.example.test?token=secret",
                "b",
                "r",
                "t",
                "p",
                "a",
                "s",
                None,
                true,
            )
            .is_err()
        );
        assert!(
            S3Config::new(
                "https://objects.example.test/#fragment",
                "b",
                "r",
                "t",
                "p",
                "a",
                "s",
                None,
                true,
            )
            .is_err()
        );
        assert!(S3Config::new("https://", "b", "r", "t", "p", "a", "s", None, true).is_err());
        assert!(
            S3Config::new(
                "https://objects.example.test:9443/s3-prefix",
                "b",
                "r",
                "t",
                "p",
                "a",
                "s",
                None,
                true,
            )
            .is_ok()
        );
        let debug = format!("{:?}", config());
        assert!(!debug.contains("objects.example.test"));
        assert!(!debug.contains("access-canary"));
        assert!(!debug.contains("secret-canary"));
    }

    #[test]
    fn s3_buckets_use_separate_request_deadlines_and_config_zeroizes_credentials() {
        let mut configured = S3Config::new(
            "https://objects.example.test",
            "chunks",
            "us-east-1",
            "tenant-a",
            "buglogin/v1",
            "access-canary",
            "secret-canary",
            Some("session-canary".to_owned()),
            true,
        )
        .unwrap();

        configured.zeroize_credentials();

        assert!(configured.access_key.is_empty());
        assert!(configured.secret_key.is_empty());
        assert_eq!(configured.session_token.as_deref(), Some(""));

        let store = S3RemoteChunkStore::new(config()).unwrap();
        assert_eq!(
            store.metadata_bucket.request_timeout(),
            Some(S3_METADATA_TIMEOUT)
        );
        assert_eq!(
            store.transfer_bucket.request_timeout(),
            Some(S3_TRANSFER_TIMEOUT)
        );
        assert!(store.config.access_key.is_empty());
        assert!(store.config.secret_key.is_empty());
        assert!(store.config.session_token.is_none());
    }
}
