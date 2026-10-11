use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

use buglogin_domain::{errors::Problem, ids::ResourceId};
use serde::{Deserialize, Serialize};

use crate::manifest::SyncManifest;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictKind {
    ConcurrentWrite,
    StaleBase,
    VersionGap,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictStatus {
    Detected,
    AwaitingResolution,
    Resolved,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConflictResolution {
    KeepCurrent,
    AcceptIncoming,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflict {
    pub resource_id: ResourceId,
    pub kind: ConflictKind,
    pub status: ConflictStatus,
    pub current_generation: u64,
    pub incoming_generation: u64,
    pub current_checksum: String,
    pub incoming_checksum: String,
    pub resolution: Option<ConflictResolution>,
}

impl SyncConflict {
    pub fn acknowledge(mut self) -> Result<Self, Problem> {
        if self.status != ConflictStatus::Detected {
            return Err(invalid_conflict_transition());
        }
        self.status = ConflictStatus::AwaitingResolution;
        Ok(self)
    }

    pub fn resolve(mut self, resolution: ConflictResolution) -> Result<Self, Problem> {
        if self.status != ConflictStatus::AwaitingResolution {
            return Err(invalid_conflict_transition());
        }
        self.resolution = Some(resolution);
        self.status = ConflictStatus::Resolved;
        Ok(self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    Applied(SyncManifest),
    Unchanged(SyncManifest),
    Conflict(SyncConflict),
}

#[derive(Clone, Debug, Default)]
pub struct ManifestCoordinator {
    current: Option<SyncManifest>,
    attempts: BTreeMap<String, AttemptRecord>,
}

#[derive(Clone, Debug)]
struct AttemptRecord {
    manifest: SyncManifest,
    outcome: ApplyOutcome,
    resolution: Option<ConflictResolution>,
    replacement: Option<SyncManifest>,
}

impl ManifestCoordinator {
    pub fn current(&self) -> Option<&SyncManifest> {
        self.current.as_ref()
    }

    pub fn submit(
        &mut self,
        idempotency_key: &str,
        incoming: SyncManifest,
    ) -> Result<ApplyOutcome, Problem> {
        let observed_at_unix_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| Problem::new("system_clock_invalid", "System clock invalid", 500, "sync"))?
            .as_millis() as u64;
        self.submit_at(idempotency_key, incoming, observed_at_unix_ms)
    }

    pub fn submit_at(
        &mut self,
        idempotency_key: &str,
        incoming: SyncManifest,
        observed_at_unix_ms: u64,
    ) -> Result<ApplyOutcome, Problem> {
        validate_idempotency_key(idempotency_key)?;
        incoming.validate()?;

        if let Some(previous) = self.attempts.get(idempotency_key) {
            if previous.manifest == incoming {
                if !matches!(
                    &previous.outcome,
                    ApplyOutcome::Conflict(SyncConflict {
                        kind: ConflictKind::VersionGap,
                        ..
                    })
                ) {
                    return Ok(previous.outcome.clone());
                }
            } else {
                return Err(Problem::new(
                    "idempotency_conflict",
                    "Idempotency key was reused with different input",
                    409,
                    "sync",
                ));
            }
        }

        let outcome = match self.current.as_ref() {
            None if incoming.generation == 1 && incoming.parent_generation.is_none() => {
                ApplyOutcome::Applied(incoming.clone())
            }
            None => {
                return Err(Problem::new(
                    "manifest_base_missing",
                    "Manifest base is missing",
                    409,
                    "sync",
                ));
            }
            Some(current) if current == &incoming => ApplyOutcome::Unchanged(incoming.clone()),
            Some(current) if current.workspace_id != incoming.workspace_id => {
                return Err(Problem::new(
                    "manifest_workspace_mismatch",
                    "Manifest workspace mismatch",
                    409,
                    "sync",
                ));
            }
            Some(current) if current.resource_id != incoming.resource_id => {
                return Err(Problem::new(
                    "manifest_resource_mismatch",
                    "Manifest resource mismatch",
                    409,
                    "sync",
                ));
            }
            Some(current) if incoming.fencing_token < current.fencing_token => {
                return Err(Problem::new(
                    "stale_manifest_fencing_token",
                    "Stale manifest fencing token",
                    409,
                    "sync",
                ));
            }
            Some(current) if incoming.generation == current.generation => {
                ApplyOutcome::Conflict(conflict(current, &incoming, ConflictKind::ConcurrentWrite))
            }
            Some(current)
                if incoming.parent_generation == Some(current.generation)
                    && incoming.generation == current.generation + 1 =>
            {
                incoming.validate_successor(current, observed_at_unix_ms)?;
                ApplyOutcome::Applied(incoming.clone())
            }
            Some(current) if incoming.generation <= current.generation => {
                ApplyOutcome::Conflict(conflict(current, &incoming, ConflictKind::StaleBase))
            }
            Some(current) => {
                ApplyOutcome::Conflict(conflict(current, &incoming, ConflictKind::VersionGap))
            }
        };

        if let ApplyOutcome::Applied(manifest) = &outcome {
            self.current = Some(manifest.clone());
        }
        self.attempts.insert(
            idempotency_key.to_owned(),
            AttemptRecord {
                manifest: incoming,
                outcome: outcome.clone(),
                resolution: None,
                replacement: None,
            },
        );
        Ok(outcome)
    }

    pub fn resolve_at(
        &mut self,
        idempotency_key: &str,
        resolution: ConflictResolution,
        replacement: Option<SyncManifest>,
        observed_at_unix_ms: u64,
    ) -> Result<ApplyOutcome, Problem> {
        validate_idempotency_key(idempotency_key)?;
        let attempt = self.attempts.get(idempotency_key).cloned().ok_or_else(|| {
            Problem::new(
                "sync_conflict_not_found",
                "Sync conflict not found",
                404,
                "sync",
            )
        })?;
        if let Some(previous_resolution) = attempt.resolution {
            if previous_resolution != resolution || attempt.replacement != replacement {
                return Err(Problem::new(
                    "conflict_resolution_mismatch",
                    "Conflict resolution was retried with different input",
                    409,
                    "sync",
                ));
            }
            return Ok(attempt.outcome);
        }
        let conflict = match &attempt.outcome {
            ApplyOutcome::Conflict(conflict) => conflict,
            _ => {
                return Err(Problem::new(
                    "sync_conflict_not_pending",
                    "Sync conflict is not pending",
                    409,
                    "sync",
                ));
            }
        };
        let current = self.current.clone().ok_or_else(|| {
            Problem::new(
                "manifest_base_missing",
                "Manifest base is missing",
                409,
                "sync",
            )
        })?;
        if current.generation != conflict.current_generation
            || current.root_checksum != conflict.current_checksum
        {
            return Err(Problem::new(
                "stale_conflict_resolution",
                "Conflict base changed before resolution",
                409,
                "sync",
            ));
        }
        let outcome = match resolution {
            ConflictResolution::KeepCurrent => {
                if replacement.is_some() {
                    return Err(Problem::new(
                        "unexpected_resolution_manifest",
                        "Keep-current resolution cannot include a manifest",
                        400,
                        "sync",
                    ));
                }
                ApplyOutcome::Unchanged(current)
            }
            ConflictResolution::AcceptIncoming => {
                let replacement = replacement.clone().ok_or_else(|| {
                    Problem::new(
                        "resolution_manifest_required",
                        "Accept-incoming resolution requires a canonical manifest",
                        400,
                        "sync",
                    )
                })?;
                replacement.validate()?;
                if replacement.workspace_id != attempt.manifest.workspace_id
                    || replacement.resource_id != attempt.manifest.resource_id
                    || replacement.fencing_token != attempt.manifest.fencing_token
                    || replacement.root_checksum != attempt.manifest.root_checksum
                    || replacement.objects != attempt.manifest.objects
                    || replacement.tombstones != attempt.manifest.tombstones
                    || replacement.generation != current.generation + 1
                    || replacement.parent_generation != Some(current.generation)
                {
                    return Err(Problem::new(
                        "invalid_resolution_manifest",
                        "Resolution manifest is not the canonical incoming successor",
                        409,
                        "sync",
                    ));
                }
                replacement.validate_successor(&current, observed_at_unix_ms)?;
                self.current = Some(replacement.clone());
                ApplyOutcome::Applied(replacement)
            }
        };
        let record = self
            .attempts
            .get_mut(idempotency_key)
            .expect("validated attempt exists");
        record.outcome = outcome.clone();
        record.resolution = Some(resolution);
        record.replacement = replacement;
        Ok(outcome)
    }
}

fn conflict(current: &SyncManifest, incoming: &SyncManifest, kind: ConflictKind) -> SyncConflict {
    SyncConflict {
        resource_id: current.resource_id.clone(),
        kind,
        status: ConflictStatus::Detected,
        current_generation: current.generation,
        incoming_generation: incoming.generation,
        current_checksum: current.root_checksum.clone(),
        incoming_checksum: incoming.root_checksum.clone(),
        resolution: None,
    }
}

fn validate_idempotency_key(key: &str) -> Result<(), Problem> {
    if !(16..=128).contains(&key.len())
        || !key.is_ascii()
        || key.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        return Err(Problem::new(
            "invalid_idempotency_key",
            "Invalid idempotency key",
            400,
            "sync",
        ));
    }
    Ok(())
}

fn invalid_conflict_transition() -> Problem {
    Problem::new(
        "invalid_conflict_transition",
        "Invalid conflict transition",
        409,
        "sync",
    )
}
