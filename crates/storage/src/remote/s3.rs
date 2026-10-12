use std::fmt;
use std::{env, sync::Mutex};

use buglogin_domain::errors::Problem;
use http::{HeaderMap, HeaderName, HeaderValue};
use s3::bucket::Bucket;
use s3::creds::Credentials;
use s3::region::Region;

use super::{
    ConcurrentRemoteChunkStore, RemoteChunk, RemoteChunkStore, checksum_problem,
    validate_object_key,
};
use crate::blob::checksum;

const CHECKSUM_METADATA: &str = "sha256";

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
        Self::new(
            required_env("BUGLOGIN_S3_ENDPOINT")?,
            required_env("BUGLOGIN_S3_BUCKET")?,
            env::var("BUGLOGIN_S3_REGION").unwrap_or_else(|_| "us-east-1".to_owned()),
            required_env("BUGLOGIN_TENANT_ID")?,
            env::var("BUGLOGIN_S3_PREFIX").unwrap_or_else(|_| "buglogin/v1".to_owned()),
            required_env("BUGLOGIN_S3_ACCESS_KEY")?,
            required_env("BUGLOGIN_S3_SECRET_KEY")?,
            env::var("BUGLOGIN_S3_SESSION_TOKEN").ok(),
            env::var("BUGLOGIN_S3_PATH_STYLE")
                .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true")),
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
        if !self.endpoint.starts_with("https://")
            || self.endpoint.contains(['\r', '\n'])
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
}

impl fmt::Debug for S3Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("S3Config")
            .field("endpoint", &self.endpoint)
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

pub struct S3RemoteChunkStore {
    bucket: Box<Bucket>,
    config: S3Config,
    object_locks: [Mutex<()>; 128],
}

impl S3RemoteChunkStore {
    pub fn new(config: S3Config) -> Result<Self, Problem> {
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
        Ok(Self {
            bucket,
            config,
            object_locks: std::array::from_fn(|_| Mutex::new(())),
        })
    }
}

impl S3RemoteChunkStore {
    fn store_readiness(&self) -> Result<(), Problem> {
        let key = self.config.namespaced_key("buglogin-readiness-probe")?;
        match self.bucket.head_object(&key) {
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
        match self.bucket.head_object(&key) {
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
            .bucket
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
        let response = self.bucket.get_object(&key).map_err(|_| unavailable())?;
        if response.status_code() == 404 {
            return Err(Problem::new(
                "remote_chunk_not_found",
                "Remote chunk not found",
                404,
                "storage",
            ));
        }
        if !(200..=299).contains(&response.status_code()) {
            return Err(unavailable());
        }
        let bytes = response.bytes().to_vec();
        if checksum(&bytes) != expected_checksum {
            return Err(checksum_problem());
        }
        Ok(RemoteChunk {
            bytes,
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
        let response = self.bucket.delete_object(&key).map_err(|_| unavailable())?;
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
        let debug = format!("{:?}", config());
        assert!(!debug.contains("access-canary"));
        assert!(!debug.contains("secret-canary"));
    }
}
