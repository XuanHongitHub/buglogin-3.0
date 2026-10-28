//! Independent integration with Fury's BSD-licensed Chromium core protocol.
//! No Fury agent/server code or provider authentication is used.

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
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    process::{
        ApprovedExecutable, ManagedProcess, ProcessController, ProcessIdentity, ProcessStatus,
        SystemProcessController,
    },
    runtime::{
        LaunchSpecification, RuntimeAdapter, RuntimeBinary, RuntimeCapability,
        ValidatedRuntimeBinary,
    },
};

use super::{CAPABILITIES, native_path_argument, protected_argument, validate_profile_directory};

const IDENTITY: &str = "BugLogin/fury-identity.json";
const CONFIG: &str = "BugLogin/fury-fingerprint.json";
const MAX_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FuryIdentity {
    schema_version: u8,
    engine: String,
    engine_version: String,
    engine_checksum: String,
    target: ProfileFingerprintTarget,
    seed: u64,
    fingerprint: Value,
}

impl FuryIdentity {
    /// `fingerprint` is a coherent measured/captured device configuration.
    /// The adapter never manufactures unrelated hardware values independently.
    pub fn generate(
        engine_version: String,
        engine_checksum: String,
        target: ProfileFingerprintTarget,
        mut fingerprint: Value,
    ) -> Result<Self, Problem> {
        let object = fingerprint.as_object_mut().ok_or_else(invalid_identity)?;
        let mut seed_bytes = [0_u8; 8];
        getrandom::fill(&mut seed_bytes).map_err(|_| {
            problem(
                "fingerprint_entropy_unavailable",
                "Fingerprint randomness is unavailable",
                503,
            )
        })?;
        let seed = u64::from_le_bytes(seed_bytes);
        object.insert("schema_version".into(), Value::from(1));
        object.insert(
            "noise".into(),
            serde_json::json!({
                "canvasSeed": derive_seed(seed, b"canvas"),
                "audioSeed": derive_seed(seed, b"audio"),
                "clientRectsSeed": derive_seed(seed, b"client-rects"),
            }),
        );
        let identity = Self {
            schema_version: 3,
            engine: "fury-chromium".into(),
            engine_version,
            engine_checksum: engine_checksum.to_ascii_lowercase(),
            target,
            seed,
            fingerprint,
        };
        identity.validate()?;
        Ok(identity)
    }

    pub fn load(profile: &Path) -> Result<Option<Self>, Problem> {
        validate_profile_directory(profile)?;
        let path = confined_path(profile, Path::new(IDENTITY))?;
        match fs::metadata(&path) {
            Ok(metadata) if metadata.len() > MAX_BYTES => return Err(invalid_identity()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(storage_error()),
        }
        let bytes = fs::read(path).map_err(|_| storage_error())?;
        let identity: Self = serde_json::from_slice(&bytes).map_err(|_| invalid_identity())?;
        identity.validate()?;
        Ok(Some(identity))
    }

    fn validate(&self) -> Result<(), Problem> {
        if self.schema_version != 3
            || self.engine != "fury-chromium"
            || !is_sha256(&self.engine_checksum)
            || self.engine_version.split('.').count() != 4
            || self
                .engine_version
                .split('.')
                .any(|part| part.parse::<u32>().is_err())
        {
            return Err(invalid_identity());
        }
        let native = if cfg!(target_os = "windows") {
            FingerprintOperatingSystem::Windows
        } else if cfg!(target_os = "macos") {
            FingerprintOperatingSystem::Macos
        } else {
            return Err(problem(
                "fury_platform_unsupported",
                "This independent core supports Windows and macOS",
                409,
            ));
        };
        if self.target.operating_system != native || self.target.operating_system_version.is_some()
        {
            return Err(problem(
                "fury_target_unsupported",
                "Use a measured device configuration for the native operating system",
                409,
            ));
        }
        let config = &self.fingerprint;
        if config["schema_version"] != 1
            || serde_json::to_vec(config)
                .map_err(|_| invalid_identity())?
                .len() as u64
                > MAX_BYTES / 2
        {
            return Err(invalid_identity());
        }
        let platform = if cfg!(target_os = "windows") {
            "Win32"
        } else {
            "MacIntel"
        };
        let ch_platform = if cfg!(target_os = "windows") {
            "Windows"
        } else {
            "macOS"
        };
        let major = self
            .engine_version
            .split('.')
            .next()
            .expect("validated version");
        let ua = config["navigator"]["userAgent"]
            .as_str()
            .ok_or_else(invalid_identity)?;
        if config["navigator"]["platform"] != platform
            || config["clientHints"]["platform"] != ch_platform
            || !ua.contains(&format!("Chrome/{major}."))
            || config["clientHints"]["fullVersion"] != self.engine_version
        {
            return Err(invalid_identity());
        }
        let cores = config["navigator"]["hardwareConcurrency"]
            .as_u64()
            .unwrap_or(0);
        let memory = config["navigator"]["deviceMemory"].as_u64().unwrap_or(0);
        if !(2..=128).contains(&cores) || ![1, 2, 4, 8].contains(&memory) {
            return Err(invalid_identity());
        }
        for field in ["width", "height", "availWidth", "availHeight"] {
            let value = config["screen"][field].as_u64().unwrap_or(0);
            if !(320..=16384).contains(&value) {
                return Err(invalid_identity());
            }
        }
        if config["screen"]["availWidth"].as_u64() > config["screen"]["width"].as_u64()
            || config["screen"]["availHeight"].as_u64() > config["screen"]["height"].as_u64()
        {
            return Err(invalid_identity());
        }
        for (field, domain) in [
            ("canvasSeed", b"canvas".as_slice()),
            ("audioSeed", b"audio".as_slice()),
            ("clientRectsSeed", b"client-rects".as_slice()),
        ] {
            if config["noise"][field].as_i64() != Some(i64::from(derive_seed(self.seed, domain))) {
                return Err(invalid_identity());
            }
        }
        for path in CORE_CONFIG_REQUIRED {
            if lookup(config, path).is_none() {
                return Err(problem(
                    "fury_core_config_incomplete",
                    "The fingerprint config does not satisfy the pinned browser core contract",
                    400,
                ));
            }
        }
        if config["clientHints"]["fullVersion"] != self.engine_version
            || config["clientHints"]["platform"] != ch_platform
            || config["clientHints"]["formFactors"]
                .as_array()
                .is_none_or(Vec::is_empty)
            || config["gpu"]["webglParams"]["UNMASKED_VENDOR_WEBGL"]
                .as_str()
                .is_none_or(str::is_empty)
            || config["gpu"]["webglParams"]["UNMASKED_RENDERER_WEBGL"]
                .as_str()
                .is_none_or(str::is_empty)
            || config["screen"]["availWidth"].as_u64() > config["screen"]["width"].as_u64()
            || config["screen"]["availHeight"].as_u64() > config["screen"]["height"].as_u64()
        {
            return Err(invalid_identity());
        }
        let webgl = config["gpu"]["webglParams"]
            .as_object()
            .ok_or_else(invalid_identity)?;
        let extensions = config["gpu"]["webglExtensions"]
            .as_array()
            .ok_or_else(invalid_identity)?;
        let fonts = config["fonts"].as_array().ok_or_else(invalid_identity)?;
        config["fontsHidden"]
            .as_array()
            .ok_or_else(invalid_identity)?;
        let languages = config["navigator"]["languages"]
            .as_array()
            .ok_or_else(invalid_identity)?;
        let renderer = webgl["UNMASKED_RENDERER_WEBGL"]
            .as_str()
            .ok_or_else(invalid_identity)?;
        if webgl.len() < 4
            || extensions.is_empty()
            || fonts.is_empty()
            || languages.is_empty()
            || !config["audio"]["sampleRate"]
                .as_u64()
                .is_some_and(|rate| [44_100, 48_000].contains(&rate))
            || !config["screen"]["devicePixelRatio"]
                .as_f64()
                .is_some_and(|dpr| {
                    if self.target.operating_system == FingerprintOperatingSystem::Macos {
                        [1.0, 2.0].contains(&dpr)
                    } else {
                        [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0].contains(&dpr)
                    }
                })
            || (self.target.operating_system == FingerprintOperatingSystem::Macos
                && (config["screen"]["scrollbarWidth"] != 0
                    || config["navigator"]["maxTouchPoints"] != 0
                    || renderer.contains("Direct3D")
                    || renderer.contains("D3D11")))
            || (self.target.operating_system == FingerprintOperatingSystem::Windows
                && (config["screen"]["scrollbarWidth"].as_u64().unwrap_or(0) == 0
                    || renderer.contains("Metal")))
            || !matches!(
                config["webrtc"]["ipHandlingPolicy"].as_str(),
                Some("default")
                    | Some("default_public_interface_only")
                    | Some("default_public_and_private_interfaces")
                    | Some("disable_non_proxied_udp")
            )
        {
            return Err(invalid_identity());
        }
        Ok(())
    }

    fn verify_executable(&self, path: &Path, version: &str) -> Result<(), Problem> {
        if version != self.engine_version {
            return Err(problem(
                "fury_version_mismatch",
                "The browser version differs from the stored identity",
                409,
            ));
        }
        let metadata = fs::symlink_metadata(path).map_err(|_| storage_error())?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(problem(
                "runtime_binary_invalid",
                "Runtime binary is not a regular file",
                400,
            ));
        }
        let file = fs::File::open(path).map_err(|_| storage_error())?;
        let (checksum, _) =
            buglogin_storage::blob::checksum_reader(file).map_err(|_| storage_error())?;
        if checksum != self.engine_checksum {
            return Err(problem(
                "fury_checksum_mismatch",
                "The browser artifact differs from the artifact bound to this profile identity",
                409,
            ));
        }
        Ok(())
    }

    fn persist(&self, profile: &Path) -> Result<PathBuf, Problem> {
        validate_profile_directory(profile)?;
        if Self::load(profile)?.is_none() {
            reject_foreign_profile(profile)?;
        }
        let destination = confined_path(profile, Path::new(IDENTITY))?;
        fs::create_dir_all(destination.parent().expect("identity parent"))
            .map_err(|_| storage_error())?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(confined_path(
                profile,
                Path::new("BugLogin/.fury-identity.lock"),
            )?)
            .map_err(|_| storage_error())?;
        lock.lock_exclusive().map_err(|_| storage_error())?;
        if let Some(existing) = Self::load(profile)? {
            if existing != *self {
                return Err(problem(
                    "fury_identity_conflict",
                    "The stored independent profile identity must not change during launch",
                    409,
                ));
            }
        } else {
            reject_foreign_profile(profile)?;
            write_new(
                &destination,
                &serde_json::to_vec(self).map_err(|_| invalid_identity())?,
            )?;
        }
        let config = confined_path(profile, Path::new(CONFIG))?;
        let bytes = serde_json::to_vec(&self.fingerprint).map_err(|_| invalid_identity())?;
        match fs::read(&config) {
            Ok(existing) if existing == bytes => {}
            Ok(_) => {
                return Err(problem(
                    "fury_config_conflict",
                    "The native fingerprint configuration differs from the profile identity",
                    409,
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                write_new(&config, &bytes)?
            }
            Err(_) => return Err(storage_error()),
        }
        Ok(config)
    }
}

// Keep this interoperability check aligned with the pinned Fury core 0.4.16
// contract. It intentionally rejects absent values instead of letting the core
// fall back to the host fingerprint. These are protocol facts, not copied code.
const CORE_CONFIG_REQUIRED: &[&str] = &[
    "audio.sampleRate",
    "audio.baseLatency",
    "audio.outputLatency",
    "automation.hideTraces",
    "battery.charging",
    "battery.chargingTime",
    "battery.dischargingTime",
    "battery.level",
    "clientHints.formFactors",
    "clientHints.mobile",
    "clientHints.wow64",
    "engine.jsHeapSizeLimit",
    "fonts",
    "fontsHidden",
    "gpu.webglExtensions",
    "gpu.webglParams",
    "locale.locale",
    "locale.timezone",
    "navigator.deviceMemory",
    "navigator.hardwareConcurrency",
    "navigator.languages",
    "navigator.maxTouchPoints",
    "navigator.platform",
    "navigator.userAgent",
    "clientHints.platformVersion",
    "clientHints.architecture",
    "clientHints.bitness",
    "clientHints.fullVersion",
    "clientHints.brands",
    "clientHints.fullVersionList",
    "clientHints.model",
    "noise.audioSeed",
    "noise.canvasSeed",
    "noise.clientRectsSeed",
    "permissions.notifications",
    "screen.availLeft",
    "screen.availTop",
    "screen.chromeHeightDelta",
    "screen.chromeWidthDelta",
    "screen.colorDepth",
    "screen.devicePixelRatio",
    "screen.height",
    "screen.scrollbarWidth",
    "screen.width",
    "webrtc.ipHandlingPolicy",
];

fn lookup<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.')
        .try_fold(value, |node, part| node.get(part))
        .filter(|value| !value.is_null())
}

fn derive_seed(seed: u64, domain: &[u8]) -> i32 {
    let mut hasher = Sha256::new();
    hasher.update(b"buglogin-fury-profile-seed-v1\0");
    hasher.update(seed.to_le_bytes());
    hasher.update((domain.len() as u32).to_le_bytes());
    hasher.update(domain);
    i32::from_le_bytes(hasher.finalize()[..4].try_into().expect("digest prefix"))
}

fn reject_foreign_profile(profile: &Path) -> Result<(), Problem> {
    for marker in [
        "Local State",
        "BugLogin/fingerprint.json",
        "BugLogin/independent-chromium.json",
    ] {
        if confined_path(profile, Path::new(marker))?.exists() {
            return Err(problem(
                "fury_profile_engine_conflict",
                "This profile already contains data owned by another browser runtime",
                409,
            ));
        }
    }
    Ok(())
}

fn write_new(destination: &Path, bytes: &[u8]) -> Result<(), Problem> {
    let temporary = destination.with_extension("json.tmp");
    let result = (|| {
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        output.write_all(bytes)?;
        output.sync_all()?;
        fs::rename(&temporary, destination)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
        return Err(storage_error());
    }
    Ok(())
}

#[derive(Clone, Debug)]
pub struct FuryCoreRuntimeAdapter {
    identity: FuryIdentity,
}

impl FuryCoreRuntimeAdapter {
    pub fn new(identity: FuryIdentity) -> Result<Self, Problem> {
        identity.validate()?;
        Ok(Self { identity })
    }
}

impl RuntimeAdapter for FuryCoreRuntimeAdapter {
    fn runtime(&self) -> Runtime {
        Runtime::Bugium
    }
    fn capabilities(&self) -> &'static [RuntimeCapability] {
        CAPABILITIES
    }
    fn profile_path(&self, root: &Path, id: &ProfileId) -> Result<PathBuf, Problem> {
        confined_path(root, Path::new(id.as_str()))
    }
    fn validate_binary(&self, binary: &RuntimeBinary) -> Result<ValidatedRuntimeBinary, Problem> {
        if binary.expected_checksum.to_ascii_lowercase() != self.identity.engine_checksum {
            return Err(problem(
                "fury_checksum_mismatch",
                "The approved browser checksum differs from the artifact bound to this profile identity",
                409,
            ));
        }
        self.identity
            .verify_executable(&binary.path, &binary.version)?;
        let metadata = fs::metadata(&binary.path).map_err(|_| storage_error())?;
        let size_bytes = metadata.len();
        if binary.expected_checksum.len() != 64 {
            return Err(problem(
                "runtime_checksum_mismatch",
                "Runtime checksum mismatch",
                400,
            ));
        }
        Ok(ValidatedRuntimeBinary {
            path: binary.path.clone(),
            version: binary.version.clone(),
            checksum: self.identity.engine_checksum.clone(),
            size_bytes,
        })
    }
    fn launch_specification(
        &self,
        binary: &ValidatedRuntimeBinary,
        profile: &Path,
        extra: &[String],
    ) -> Result<LaunchSpecification, Problem> {
        self.identity.validate()?;
        if binary.checksum != self.identity.engine_checksum {
            return Err(problem(
                "fury_checksum_mismatch",
                "The browser artifact differs from the artifact bound to this profile identity",
                409,
            ));
        }
        if extra
            .iter()
            .any(|argument| protected_argument(argument) || fury_switch(argument))
        {
            return Err(problem(
                "fury_identity_override_forbidden",
                "Arguments cannot override the profile identity or native carrier",
                400,
            ));
        }
        self.identity.persist(profile)?;
        let mut args = vec![
            format!("--user-data-dir={}", native_path_argument(profile)),
            "--no-first-run".into(),
            "--no-default-browser-check".into(),
            "--disable-background-mode".into(),
        ];
        args.extend(extra.iter().cloned());
        Ok(LaunchSpecification {
            runtime: Runtime::Bugium,
            version: binary.version.clone(),
            executable: binary.path.clone(),
            profile_path: profile.to_owned(),
            args,
        })
    }
}

fn fury_switch(argument: &str) -> bool {
    argument
        .trim()
        .trim_start_matches(['-', '/'])
        .to_ascii_lowercase()
        .starts_with("fury-")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Process controller for the standalone Fury Chromium core. It does not use
/// Fury's desktop app, agent, team server, or authentication service.
pub struct FuryCoreProcessController {
    inner: SystemProcessController,
}

impl FuryCoreProcessController {
    pub fn new(approved: impl IntoIterator<Item = ApprovedExecutable>) -> Self {
        Self {
            inner: SystemProcessController::new(approved),
        }
    }
}

impl ProcessController for FuryCoreProcessController {
    fn launch(
        &self,
        specification: &LaunchSpecification,
    ) -> Result<Box<dyn ManagedProcess>, Problem> {
        let identity =
            FuryIdentity::load(&specification.profile_path)?.ok_or_else(invalid_identity)?;
        if specification.runtime != Runtime::Bugium
            || specification.version != identity.engine_version
            || specification
                .args
                .iter()
                .any(|argument| fury_switch(argument))
        {
            return Err(invalid_identity());
        }
        identity.verify_executable(&specification.executable, &specification.version)?;
        let expected = format!(
            "--user-data-dir={}",
            native_path_argument(&specification.profile_path)
        );
        if specification
            .args
            .iter()
            .filter(|argument| protected_argument(argument))
            .collect::<Vec<_>>()
            != vec![&expected]
        {
            return Err(invalid_identity());
        }
        let config = identity.persist(&specification.profile_path)?;
        self.inner.launch_with_fingerprint(specification, &config)
    }
    fn inspect(&self, identity: &ProcessIdentity) -> ProcessStatus {
        self.inner.inspect(identity)
    }
    fn terminate(&self, identity: &ProcessIdentity) -> Result<(), Problem> {
        self.inner.terminate(identity)
    }
    fn approve_executable(&self, path: &Path) -> Result<(), Problem> {
        self.inner.approve_executable(path)
    }
}

fn invalid_identity() -> Problem {
    problem(
        "fury_identity_invalid",
        "The independent browser identity is invalid or inconsistent",
        409,
    )
}
fn storage_error() -> Problem {
    problem(
        "fury_identity_storage_failed",
        "The independent browser identity could not be read or stored",
        503,
    )
}
fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "independent-chromium")
}
