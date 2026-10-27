use std::{env, path::PathBuf};

use buglogin_domain::errors::Problem;
#[cfg(any(target_os = "macos", test))]
use sha2::{Digest, Sha256};

const INSTANCE_ENV: &str = "BUGLOGIN_INSTANCE_ID";
const MAX_INSTANCE_ID_LEN: usize = 32;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct AppInstance {
    id: Option<String>,
}

impl AppInstance {
    pub(crate) fn from_environment() -> Result<Self, Problem> {
        match env::var(INSTANCE_ENV) {
            Ok(value) => Self::from_id(Some(&value)),
            Err(env::VarError::NotPresent) => Ok(Self::default()),
            Err(env::VarError::NotUnicode(_)) => Err(invalid_instance_id()),
        }
    }

    pub(crate) fn from_id(id: Option<&str>) -> Result<Self, Problem> {
        let Some(id) = id else {
            return Ok(Self::default());
        };
        if id.is_empty()
            || id.len() > MAX_INSTANCE_ID_LEN
            || !id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"-_".contains(&byte)
            })
            || !id.as_bytes()[0].is_ascii_alphanumeric()
        {
            return Err(invalid_instance_id());
        }
        Ok(Self {
            id: Some(id.to_owned()),
        })
    }

    pub(crate) fn is_isolated(&self) -> bool {
        self.id.is_some()
    }

    pub(crate) fn uses_single_instance_guard(&self) -> bool {
        !self.is_isolated()
    }

    pub(crate) fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    pub(crate) fn namespace_path(&self, base: PathBuf) -> PathBuf {
        match &self.id {
            Some(id) => base.join("instances").join(id),
            None => base,
        }
    }

    pub(crate) fn app_data_dir(base: PathBuf) -> PathBuf {
        let instance = Self::from_environment()
            .unwrap_or_else(|_| panic!("{INSTANCE_ENV} must be validated before app startup"));
        instance.namespace_path(base)
    }

    pub(crate) fn device_identity_namespace(&self, base: &str) -> String {
        self.vault_namespace(base)
    }

    pub(crate) fn vault_namespace(&self, base: &str) -> String {
        match &self.id {
            Some(id) => format!("{base}-instance-{id}"),
            None => base.to_owned(),
        }
    }

    pub(crate) fn webview_relative_data_directory(&self) -> Option<PathBuf> {
        self.id.as_ref().map(|id| {
            PathBuf::from("BugLogin")
                .join("instances")
                .join(id)
                .join("webview")
        })
    }

    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn webview_data_store_identifier(&self) -> Option<[u8; 16]> {
        self.id.as_ref().map(|id| {
            let digest = Sha256::digest(format!("buglogin-webview-instance-v1:{id}").as_bytes());
            let mut identifier = [0; 16];
            identifier.copy_from_slice(&digest[..16]);
            identifier
        })
    }
}

fn invalid_instance_id() -> Problem {
    Problem::new(
        "app_instance_id_invalid",
        "BUGLOGIN_INSTANCE_ID must be 1 to 32 lowercase letters, digits, hyphens, or underscores and start with a letter or digit",
        400,
        "desktop-bootstrap",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_instance_preserves_existing_paths_and_identity_namespace() {
        let instance = AppInstance::from_id(None).unwrap();
        let base = PathBuf::from("data");

        assert!(!instance.is_isolated());
        assert!(instance.uses_single_instance_guard());
        assert_eq!(instance.namespace_path(base.clone()), base);
        assert_eq!(
            instance.device_identity_namespace("buglogin-device-identity"),
            "buglogin-device-identity"
        );
        assert_eq!(instance.vault_namespace("buglogin-sync"), "buglogin-sync");
        assert_eq!(instance.webview_relative_data_directory(), None);
        assert_eq!(instance.webview_data_store_identifier(), None);
    }

    #[test]
    fn instance_namespaces_are_distinct_and_safe_for_paths_and_vault_keys() {
        let first = AppInstance::from_id(Some("qa-a")).unwrap();
        let second = AppInstance::from_id(Some("qa-b")).unwrap();

        assert!(!first.uses_single_instance_guard());
        assert!(!second.uses_single_instance_guard());
        assert_ne!(
            first.namespace_path(PathBuf::from("data")),
            second.namespace_path(PathBuf::from("data"))
        );
        assert_ne!(
            first.device_identity_namespace("buglogin-device-identity"),
            second.device_identity_namespace("buglogin-device-identity")
        );
        assert_ne!(
            first.vault_namespace("buglogin-sync"),
            second.vault_namespace("buglogin-sync")
        );
        assert_ne!(
            first.webview_relative_data_directory(),
            second.webview_relative_data_directory()
        );
        assert_ne!(
            first.webview_data_store_identifier(),
            second.webview_data_store_identifier()
        );
    }

    #[test]
    fn invalid_instance_ids_cannot_escape_the_namespace() {
        for id in ["", ".", "..", "qa/a", "qa\\a", "QA-A", "has space"] {
            assert_eq!(
                AppInstance::from_id(Some(id)).unwrap_err().code,
                "app_instance_id_invalid"
            );
        }
        assert_eq!(
            AppInstance::from_id(Some(&"a".repeat(MAX_INSTANCE_ID_LEN + 1)))
                .unwrap_err()
                .code,
            "app_instance_id_invalid"
        );
    }
}
