//! Chromium fingerprint engine integration without Wayfern provider services.
//! This does not change the production runtime approval/catalog policy.

use std::{
    fs,
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

use buglogin_domain::{
    entities::{FingerprintOperatingSystem, ProfileFingerprintTarget, Runtime},
    errors::Problem,
    ids::ProfileId,
};
use buglogin_storage::paths::confined_path;
use fs2::FileExt;
use serde::{Deserialize, Serialize};

use super::{
    LaunchSpecification, RuntimeAdapter, RuntimeBinary, RuntimeCapability, ValidatedRuntimeBinary,
};

const IDENTITY_FILE: &str = "BugLogin/independent-chromium.json";
const MAX_IDENTITY_BYTES: u64 = 16 * 1024;
const CAPABILITIES: &[RuntimeCapability] = &[
    RuntimeCapability::ProfileIsolation,
    RuntimeCapability::Proxy,
    RuntimeCapability::Cookies,
    RuntimeCapability::Extensions,
    RuntimeCapability::Fingerprint,
    RuntimeCapability::Cdp,
];

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChromiumIdentity {
    schema_version: u8,
    engine: String,
    engine_version: String,
    seed: u32,
    target: ProfileFingerprintTarget,
}

impl ChromiumIdentity {
    pub fn generate(
        engine_version: String,
        target: ProfileFingerprintTarget,
    ) -> Result<Self, Problem> {
        let mut bytes = [0_u8; 4];
        let seed = loop {
            getrandom::fill(&mut bytes).map_err(|_| {
                problem(
                    "fingerprint_entropy_unavailable",
                    "Fingerprint randomness is unavailable",
                    503,
                )
            })?;
            let value = u32::from_le_bytes(bytes);
            if value != 0 {
                break value;
            }
        };
        let identity = Self {
            schema_version: 1,
            engine: "fingerprint-chromium".into(),
            engine_version,
            seed,
            target,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    pub fn load(profile_path: &Path) -> Result<Option<Self>, Problem> {
        validate_profile_directory(profile_path)?;
        let path = confined_path(profile_path, Path::new(IDENTITY_FILE))?;
        if fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_IDENTITY_BYTES) {
            return Err(problem(
                "chromium_identity_invalid",
                "Stored Chromium identity exceeds its size limit",
                409,
            ));
        }
        match fs::read(path) {
            Ok(bytes) => {
                let identity: Self = serde_json::from_slice(&bytes).map_err(|_| {
                    problem(
                        "chromium_identity_invalid",
                        "Stored Chromium identity is invalid",
                        409,
                    )
                })?;
                identity.validate()?;
                Ok(Some(identity))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(problem(
                "chromium_identity_unavailable",
                "Stored Chromium identity cannot be read",
                503,
            )),
        }
    }

    fn validate(&self) -> Result<(), Problem> {
        if self.schema_version != 1
            || self.engine != "fingerprint-chromium"
            || self.seed == 0
            || self.engine_version.split('.').count() != 4
            || self
                .engine_version
                .split('.')
                .any(|part| part.is_empty() || part.parse::<u32>().is_err())
        {
            return Err(problem(
                "chromium_identity_invalid",
                "Chromium identity is invalid",
                409,
            ));
        }
        // Official fingerprint-chromium 150 crashes on canvas readback. Native
        // probes reproduced upstream issues #94/#95; never mask this by
        // disabling fingerprint surfaces or silently choosing an older engine.
        if self.engine_version.split('.').next() == Some("150") {
            return Err(problem(
                "chromium_engine_regression",
                "This Chromium engine version has a verified canvas renderer crash",
                409,
            ));
        }
        let native = if cfg!(target_os = "windows") {
            FingerprintOperatingSystem::Windows
        } else if cfg!(target_os = "macos") {
            FingerprintOperatingSystem::Macos
        } else {
            FingerprintOperatingSystem::Linux
        };
        if self.target.operating_system != native || self.target.operating_system_version.is_some()
        {
            return Err(problem(
                "chromium_fingerprint_target_unsupported",
                "This Chromium adapter currently supports the native operating system only",
                409,
            ));
        }
        Ok(())
    }

    fn ensure_persisted(&self, profile_path: &Path) -> Result<(), Problem> {
        let destination = confined_path(profile_path, Path::new(IDENTITY_FILE))?;
        let parent = destination.parent().expect("identity has a parent");
        fs::create_dir_all(parent).map_err(storage_error)?;
        let lock_path = confined_path(
            profile_path,
            Path::new("BugLogin/.independent-chromium.lock"),
        )?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(storage_error)?;
        lock.lock_exclusive().map_err(storage_error)?;
        if let Some(stored) = Self::load(profile_path)? {
            if stored != *self {
                return Err(problem(
                    "chromium_identity_conflict",
                    "The stored profile identity must not change during launch",
                    409,
                ));
            }
            return Ok(());
        }
        if confined_path(profile_path, Path::new("BugLogin/fingerprint.json"))?.exists()
            || confined_path(profile_path, Path::new("Local State"))?.exists()
        {
            return Err(problem(
                "chromium_profile_engine_conflict",
                "This browser profile requires an explicit identity migration before using this adapter",
                409,
            ));
        }
        let temporary = confined_path(
            profile_path,
            Path::new("BugLogin/.independent-chromium.json.tmp"),
        )?;
        let encoded = serde_json::to_vec(self).map_err(|_| {
            problem(
                "chromium_identity_invalid",
                "Chromium identity cannot be encoded",
                500,
            )
        })?;
        let result = (|| {
            let mut output = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary)?;
            output.write_all(&encoded)?;
            output.sync_all()?;
            fs::rename(&temporary, &destination)
        })();
        if let Err(error) = result {
            let _ = fs::remove_file(&temporary);
            return Err(storage_error(error));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct IndependentChromiumAdapter {
    identity: ChromiumIdentity,
}

impl IndependentChromiumAdapter {
    pub fn new(identity: ChromiumIdentity) -> Result<Self, Problem> {
        identity.validate()?;
        Ok(Self { identity })
    }
}

impl RuntimeAdapter for IndependentChromiumAdapter {
    fn runtime(&self) -> Runtime {
        Runtime::Bugium
    }
    fn capabilities(&self) -> &'static [RuntimeCapability] {
        CAPABILITIES
    }

    fn validate_binary(&self, binary: &RuntimeBinary) -> Result<ValidatedRuntimeBinary, Problem> {
        if binary.version != self.identity.engine_version {
            return Err(problem(
                "chromium_identity_version_mismatch",
                "The browser version does not match this profile identity",
                409,
            ));
        }
        if binary.expected_checksum.len() != 64
            || !binary
                .expected_checksum
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(problem(
                "runtime_checksum_invalid",
                "Runtime checksum invalid",
                400,
            ));
        }
        let metadata = fs::symlink_metadata(&binary.path)
            .map_err(|_| problem("runtime_binary_not_found", "Runtime binary not found", 400))?;
        if !metadata.file_type().is_file() {
            return Err(problem(
                "runtime_binary_invalid",
                "Runtime binary is not a regular file",
                400,
            ));
        }
        let file = fs::File::open(&binary.path).map_err(|_| {
            problem(
                "runtime_binary_unreadable",
                "Runtime binary is unreadable",
                400,
            )
        })?;
        let (checksum, size) = buglogin_storage::blob::checksum_reader(file).map_err(|_| {
            problem(
                "runtime_binary_unreadable",
                "Runtime binary is unreadable",
                400,
            )
        })?;
        if checksum != binary.expected_checksum.to_ascii_lowercase() || size != metadata.len() {
            return Err(problem(
                "runtime_checksum_mismatch",
                "Runtime checksum mismatch",
                400,
            ));
        }
        Ok(ValidatedRuntimeBinary {
            path: binary.path.clone(),
            version: binary.version.clone(),
            checksum,
            size_bytes: size,
        })
    }

    fn profile_path(
        &self,
        profiles_root: &Path,
        profile_id: &ProfileId,
    ) -> Result<PathBuf, Problem> {
        confined_path(profiles_root, Path::new(profile_id.as_str()))
    }

    fn launch_specification(
        &self,
        binary: &ValidatedRuntimeBinary,
        profile_path: &Path,
        extra_args: &[String],
    ) -> Result<LaunchSpecification, Problem> {
        self.identity.validate()?;
        if binary.version != self.identity.engine_version {
            return Err(problem(
                "chromium_identity_version_mismatch",
                "The browser version does not match this profile identity",
                409,
            ));
        }
        validate_profile_directory(profile_path)?;
        if extra_args
            .iter()
            .any(|argument| protected_argument(argument))
        {
            return Err(problem(
                "chromium_identity_override_forbidden",
                "Runtime arguments cannot override the profile identity or data directory",
                400,
            ));
        }
        self.identity.ensure_persisted(profile_path)?;
        let os = match self.identity.target.operating_system {
            FingerprintOperatingSystem::Windows => "windows",
            FingerprintOperatingSystem::Macos => "macos",
            FingerprintOperatingSystem::Linux => "linux",
            _ => unreachable!("identity target validated"),
        };
        let mut args = vec![
            format!("--user-data-dir={}", native_path_argument(profile_path)),
            format!("--fingerprint={}", self.identity.seed),
            format!("--fingerprint-platform={os}"),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-background-mode".into(),
        ];
        args.extend(extra_args.iter().cloned());
        Ok(LaunchSpecification {
            runtime: self.runtime(),
            version: binary.version.clone(),
            executable: binary.path.clone(),
            profile_path: profile_path.to_owned(),
            args,
        })
    }
}

fn protected_argument(argument: &str) -> bool {
    let name = argument
        .split_once('=')
        .map_or(argument, |(name, _)| name)
        .to_ascii_lowercase();
    name == "--fingerprint"
        || name.starts_with("--fingerprint-")
        || matches!(
            name.as_str(),
            "--user-data-dir"
                | "--profile-directory"
                | "--user-agent"
                | "--disable-spoofing"
                | "--incognito"
                | "--guest"
        )
}

fn validate_profile_directory(profile_path: &Path) -> Result<(), Problem> {
    if !profile_path.is_absolute()
        || !fs::symlink_metadata(profile_path)
            .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        return Err(problem(
            "runtime_profile_path_invalid",
            "An existing isolated profile directory is required",
            400,
        ));
    }
    Ok(())
}

fn native_path_argument(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return format!(r"\\{unc}");
    }
    if let Some(drive) = text.strip_prefix(r"\\?\") {
        let bytes = drive.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/')
        {
            return drive.to_owned();
        }
    }
    text.into_owned()
}

fn storage_error(_: std::io::Error) -> Problem {
    problem(
        "chromium_identity_storage_failed",
        "The profile identity could not be stored",
        503,
    )
}

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "independent-chromium")
}
