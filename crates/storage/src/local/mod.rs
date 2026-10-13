use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use buglogin_domain::entities::{
    Extension, ExtensionGroup, Group, Profile, Proxy, RuntimeInstall, Vpn,
};
use buglogin_domain::errors::Problem;
use buglogin_domain::profile::ProfileResource;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

const SCHEMA_VERSION: u32 = 1;
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EntityKind {
    Profile,
    ProfileResource,
    Group,
    Proxy,
    Vpn,
    Extension,
    ExtensionGroup,
    RuntimeInstall,
}

impl EntityKind {
    fn directory(self) -> &'static str {
        match self {
            Self::Profile => "profiles",
            Self::ProfileResource => "profile-resources",
            Self::Group => "groups",
            Self::Proxy => "proxies",
            Self::Vpn => "vpns",
            Self::Extension => "extensions",
            Self::ExtensionGroup => "extension-groups",
            Self::RuntimeInstall => "runtime-installs",
        }
    }
}

pub trait LocalEntity: DeserializeOwned + Serialize {
    const KIND: EntityKind;

    fn entity_id(&self) -> &str;
}

impl LocalEntity for Profile {
    const KIND: EntityKind = EntityKind::Profile;

    fn entity_id(&self) -> &str {
        self.id.as_str()
    }
}

impl LocalEntity for ProfileResource {
    const KIND: EntityKind = EntityKind::ProfileResource;

    fn entity_id(&self) -> &str {
        self.profile_id.as_str()
    }
}

impl LocalEntity for Group {
    const KIND: EntityKind = EntityKind::Group;

    fn entity_id(&self) -> &str {
        self.id.as_str()
    }
}

impl LocalEntity for Proxy {
    const KIND: EntityKind = EntityKind::Proxy;

    fn entity_id(&self) -> &str {
        self.id.as_str()
    }
}

impl LocalEntity for Vpn {
    const KIND: EntityKind = EntityKind::Vpn;

    fn entity_id(&self) -> &str {
        self.id.as_str()
    }
}

impl LocalEntity for Extension {
    const KIND: EntityKind = EntityKind::Extension;

    fn entity_id(&self) -> &str {
        self.id.as_str()
    }
}

impl LocalEntity for ExtensionGroup {
    const KIND: EntityKind = EntityKind::ExtensionGroup;

    fn entity_id(&self) -> &str {
        self.id.as_str()
    }
}

impl LocalEntity for RuntimeInstall {
    const KIND: EntityKind = EntityKind::RuntimeInstall;

    fn entity_id(&self) -> &str {
        self.id.as_str()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Versioned<T> {
    pub schema_version: u32,
    pub revision: u64,
    pub value: T,
}

#[derive(Clone, Debug)]
pub struct LocalRepository {
    root: PathBuf,
}

impl LocalRepository {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, Problem> {
        let root = root.into();
        fs::create_dir_all(&root).map_err(|error| io_problem("create repository root", &error))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn get<T: LocalEntity>(&self, id: &str) -> Result<Option<Versioned<T>>, Problem> {
        validate_id(id)?;
        let revisions = self.revisions_directory(T::KIND, id);
        let Some(revision) = latest_revision(&revisions)? else {
            return Ok(None);
        };
        read_revision(&revision_path(&revisions, revision))
    }

    pub fn list<T: LocalEntity>(&self) -> Result<Vec<Versioned<T>>, Problem> {
        let directory = self.root.join(T::KIND.directory());
        if !directory.exists() {
            return Ok(Vec::new());
        }
        let mut values: Vec<Versioned<T>> = Vec::new();
        for entry in
            fs::read_dir(&directory).map_err(|error| io_problem("read entity directory", &error))?
        {
            let entry = entry.map_err(|error| io_problem("read entity entry", &error))?;
            if !entry
                .file_type()
                .map_err(|error| io_problem("read entity entry type", &error))?
                .is_dir()
            {
                continue;
            }
            let revisions = entry.path().join("revisions");
            let Some(revision) = latest_revision(&revisions)? else {
                continue;
            };
            if let Some(value) = read_revision(&revision_path(&revisions, revision))? {
                values.push(value);
            }
        }
        values.sort_by(|left, right| left.value.entity_id().cmp(right.value.entity_id()));
        Ok(values)
    }

    pub fn create<T: LocalEntity>(&self, value: T) -> Result<Versioned<T>, Problem> {
        self.write(None, value)
    }

    pub fn update<T: LocalEntity>(
        &self,
        expected_revision: u64,
        value: T,
    ) -> Result<Versioned<T>, Problem> {
        self.write(Some(expected_revision), value)
    }

    pub fn quarantine<T: LocalEntity>(
        &self,
        expected_revision: u64,
        id: &str,
    ) -> Result<PathBuf, Problem> {
        validate_id(id)?;
        let revisions = self.revisions_directory(T::KIND, id);
        let actual_revision = latest_revision(&revisions)?;
        if actual_revision != Some(expected_revision) {
            return Err(conflict_problem(
                T::KIND,
                id,
                Some(expected_revision),
                actual_revision,
            ));
        }
        let source = self.root.join(T::KIND.directory()).join(id);
        let destination = self
            .root
            .join("quarantine")
            .join(T::KIND.directory())
            .join(format!("{id}-{expected_revision}"));
        if destination.exists() {
            return Err(Problem::new(
                "storage_quarantine_exists",
                "Quarantine destination already exists",
                409,
                "storage",
            ));
        }
        let parent = destination.parent().ok_or_else(|| {
            Problem::new(
                "storage_path_invalid",
                "Storage path invalid",
                400,
                "storage",
            )
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| io_problem("create quarantine directory", &error))?;
        fs::rename(&source, &destination)
            .map_err(|error| io_problem("quarantine entity", &error))?;
        Ok(destination)
    }

    pub fn quarantined_path<T: LocalEntity>(
        &self,
        expected_revision: u64,
        id: &str,
    ) -> Result<Option<PathBuf>, Problem> {
        validate_id(id)?;
        let destination = self
            .root
            .join("quarantine")
            .join(T::KIND.directory())
            .join(format!("{id}-{expected_revision}"));
        let metadata = match fs::metadata(&destination) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io_problem("inspect quarantine entity", &error)),
        };
        if !metadata.is_dir() {
            return Err(quarantine_corrupt());
        }
        if latest_revision(&destination.join("revisions"))? != Some(expected_revision) {
            return Err(quarantine_corrupt());
        }
        Ok(Some(destination))
    }

    fn write<T: LocalEntity>(
        &self,
        expected_revision: Option<u64>,
        value: T,
    ) -> Result<Versioned<T>, Problem> {
        let entity_id = value.entity_id().to_owned();
        let id = entity_id.as_str();
        validate_id(id)?;
        let revisions = self.revisions_directory(T::KIND, id);
        fs::create_dir_all(&revisions)
            .map_err(|error| io_problem("create revision directory", &error))?;

        let actual_revision = latest_revision(&revisions)?;
        if actual_revision != expected_revision {
            return Err(conflict_problem(
                T::KIND,
                id,
                expected_revision,
                actual_revision,
            ));
        }

        let revision = actual_revision.unwrap_or(0).checked_add(1).ok_or_else(|| {
            Problem::new(
                "storage_revision_exhausted",
                "Storage revision exhausted",
                500,
                "storage",
            )
            .with_detail(format!(
                "No revision remains for {} {id}",
                T::KIND.directory()
            ))
        })?;
        let versioned = Versioned {
            schema_version: SCHEMA_VERSION,
            revision,
            value,
        };
        let temporary_path = create_temporary_file(&revisions, &versioned)?;
        let destination = revision_path(&revisions, revision);

        match fs::hard_link(&temporary_path, &destination) {
            Ok(()) => {
                let _ = fs::remove_file(&temporary_path);
                sync_directory(&revisions)?;
                Ok(versioned)
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let _ = fs::remove_file(&temporary_path);
                Err(conflict_problem(
                    T::KIND,
                    id,
                    expected_revision,
                    latest_revision(&revisions)?,
                ))
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary_path);
                Err(io_problem("publish revision", &error))
            }
        }
    }

    fn revisions_directory(&self, kind: EntityKind, id: &str) -> PathBuf {
        self.root.join(kind.directory()).join(id).join("revisions")
    }
}

fn create_temporary_file<T: Serialize>(
    directory: &Path,
    value: &Versioned<T>,
) -> Result<PathBuf, Problem> {
    loop {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(".tmp-{}-{sequence}", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => {
                let mut writer = BufWriter::new(file);
                serde_json::to_writer_pretty(&mut writer, value)
                    .map_err(|error| serialization_problem("serialize revision", &error))?;
                writer
                    .write_all(b"\n")
                    .map_err(|error| io_problem("write revision", &error))?;
                writer
                    .flush()
                    .map_err(|error| io_problem("flush revision", &error))?;
                writer
                    .get_ref()
                    .sync_all()
                    .map_err(|error| io_problem("sync revision", &error))?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_problem("create temporary revision", &error)),
        }
    }
}

fn latest_revision(directory: &Path) -> Result<Option<u64>, Problem> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_problem("read revision directory", &error)),
    };
    let mut latest = None;
    for entry in entries {
        let entry = entry.map_err(|error| io_problem("read revision entry", &error))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Some(number) = name.strip_suffix(".json") else {
            continue;
        };
        if number.len() != 20 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        let revision = number
            .parse::<u64>()
            .map_err(|error| serialization_problem("parse revision filename", &error))?;
        latest = Some(latest.map_or(revision, |current: u64| current.max(revision)));
    }
    Ok(latest)
}

fn quarantine_corrupt() -> Problem {
    Problem::new(
        "storage_quarantine_corrupt",
        "Quarantined entity does not match its expected revision",
        500,
        "storage",
    )
}

fn read_revision<T: DeserializeOwned>(path: &Path) -> Result<Option<Versioned<T>>, Problem> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_problem("open revision", &error)),
    };
    serde_json::from_reader(BufReader::new(file))
        .map(Some)
        .map_err(|error| serialization_problem("deserialize revision", &error))
}

fn revision_path(directory: &Path, revision: u64) -> PathBuf {
    directory.join(format!("{revision:020}.json"))
}

fn validate_id(id: &str) -> Result<(), Problem> {
    let valid = !id.is_empty()
        && id.len() <= 128
        && id.as_bytes()[0].is_ascii_alphanumeric()
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(
            Problem::new("invalid_storage_id", "Invalid storage ID", 400, "storage")
                .with_detail("Storage IDs must use the canonical ID character set"),
        )
    }
}

fn conflict_problem(
    kind: EntityKind,
    id: &str,
    expected: Option<u64>,
    actual: Option<u64>,
) -> Problem {
    Problem::new(
        "storage_revision_conflict",
        "Storage revision conflict",
        409,
        "storage",
    )
    .with_detail(format!(
        "{} {id} changed since revision {}",
        kind.directory(),
        expected.map_or_else(|| "none".to_owned(), |value| value.to_string())
    ))
    .with_field("entity", kind.directory())
    .with_field("id", id)
    .with_field("expectedRevision", expected)
    .with_field("actualRevision", actual)
}

fn io_problem(operation: &str, error: &std::io::Error) -> Problem {
    Problem::new(
        "storage_io_error",
        "Storage operation failed",
        500,
        "storage",
    )
    .with_detail(format!("Failed to {operation}: {error}"))
    .retryable(true)
}

fn serialization_problem(operation: &str, error: &dyn std::fmt::Display) -> Problem {
    Problem::new(
        "storage_serialization_error",
        "Storage data is invalid",
        500,
        "storage",
    )
    .with_detail(format!("Failed to {operation}: {error}"))
}

#[cfg(unix)]
fn sync_directory(directory: &Path) -> Result<(), Problem> {
    File::open(directory)
        .and_then(|file| file.sync_all())
        .map_err(|error| io_problem("sync revision directory", &error))
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Path) -> Result<(), Problem> {
    Ok(())
}
