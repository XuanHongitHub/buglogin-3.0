use std::collections::VecDeque;

use super::operations::reject_secrets;
use buglogin_domain::{
    errors::Problem,
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const CAPABILITY: &str = "events.read";
const DEFAULT_CAPACITY: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewEvent {
    pub event_id: String,
    pub event_version: String,
    pub name: String,
    pub occurred_at: String,
    pub correlation_id: String,
    pub causation_id: String,
    pub actor_id: String,
    pub scope: ResourceScope,
    pub resource_type: String,
    pub resource_id: String,
    pub resource_revision: u64,
    pub data: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub sequence: u64,
    #[serde(flatten)]
    pub event: NewEvent,
}

#[derive(Clone, Debug)]
pub struct EventService {
    capacity: usize,
    next_sequence: u64,
    events: VecDeque<Event>,
}

impl Default for EventService {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY).expect("default event capacity is non-zero")
    }
}

impl EventService {
    pub fn with_capacity(capacity: usize) -> Result<Self, Problem> {
        if capacity == 0 {
            return Err(problem(
                "invalid_event_capacity",
                "Event capacity must be greater than zero",
                400,
            ));
        }
        Ok(Self {
            capacity,
            next_sequence: 1,
            events: VecDeque::new(),
        })
    }

    pub fn publish(&mut self, event: NewEvent) -> Result<&Event, Problem> {
        validate_event(&event)?;
        if self
            .events
            .iter()
            .any(|stored| stored.event.event_id == event.event_id)
        {
            return Err(problem("event_exists", "Event already exists", 409));
        }
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.checked_add(1).ok_or_else(|| {
            problem(
                "event_sequence_exhausted",
                "Event sequence is exhausted",
                503,
            )
        })?;
        if self.events.len() == self.capacity {
            self.events.pop_front();
        }
        self.events.push_back(Event { sequence, event });
        Ok(self.events.back().expect("published event exists"))
    }

    pub fn read_after<'a>(
        &'a self,
        principal: &Principal,
        grants: &[Grant],
        scope: &ResourceScope,
        after_sequence: Option<u64>,
        limit: usize,
    ) -> Result<Vec<&'a Event>, Problem> {
        authorize(principal, grants, scope)?;
        if limit == 0 || limit > self.capacity {
            return Err(problem(
                "invalid_event_limit",
                "Event limit is invalid",
                400,
            ));
        }
        if let (Some(after), Some(oldest)) = (after_sequence, self.events.front())
            && after.saturating_add(1) < oldest.sequence
        {
            return Err(problem(
                "event_cursor_expired",
                "Event cursor is no longer retained",
                409,
            )
            .with_field("oldestSequence", oldest.sequence));
        }
        Ok(self
            .events
            .iter()
            .filter(|event| after_sequence.is_none_or(|after| event.sequence > after))
            .filter(|event| scope.contains(&event.event.scope))
            .take(limit)
            .collect())
    }
    pub fn len(&self) -> usize {
        self.events.len()
    }
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

fn validate_event(event: &NewEvent) -> Result<(), Problem> {
    for (value, field) in [
        (&event.event_id, "event_id"),
        (&event.event_version, "event_version"),
        (&event.name, "name"),
        (&event.correlation_id, "correlation_id"),
        (&event.causation_id, "causation_id"),
        (&event.actor_id, "actor_id"),
        (&event.resource_type, "resource_type"),
        (&event.resource_id, "resource_id"),
    ] {
        validate_identifier(value, field)?;
    }
    if event.occurred_at.trim().is_empty() || event.occurred_at.len() > 64 {
        return Err(problem(
            "invalid_event_timestamp",
            "Event timestamp is invalid",
            400,
        ));
    }
    if event.resource_revision == 0 {
        return Err(problem(
            "invalid_resource_revision",
            "Resource revision must be greater than zero",
            400,
        ));
    }
    reject_secrets(&event.data)
}

fn validate_identifier(value: &str, field: &str) -> Result<(), Problem> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'-'))
    {
        return Err(problem(
            "invalid_event_identifier",
            "Event identifier is invalid",
            400,
        )
        .with_field("field", field));
    }
    Ok(())
}

fn authorize(
    principal: &Principal,
    grants: &[Grant],
    scope: &ResourceScope,
) -> Result<(), Problem> {
    let capability = Capability::new(CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    if is_allowed(principal, &capability, Action::Read, scope, grants) {
        Ok(())
    } else {
        Err(problem("events_forbidden", "Event access denied", 403))
    }
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "events")
}
