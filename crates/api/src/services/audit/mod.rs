use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use buglogin_domain::{
    audit::{AuditEvent, SupportAccess},
    errors::Problem,
    ids::AccountId,
    permissions::{Action, Capability, Grant, Principal, ResourceScope, is_allowed},
};

const CAPABILITY: &str = "CAP-AUD-001";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct AuditService {
    events: Vec<AuditEvent>,
    support_access: BTreeMap<String, SupportAccess>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEventPage {
    pub items: Vec<AuditEvent>,
    pub next_cursor: Option<String>,
}

impl AuditService {
    pub fn append(&mut self, event: AuditEvent) -> Result<(), Problem> {
        if self
            .events
            .iter()
            .any(|item| item.event_id == event.event_id)
        {
            return Err(problem(
                "audit_event_exists",
                "Audit event already exists",
                409,
            ));
        }
        self.events.push(event);
        Ok(())
    }

    pub fn read<'a>(
        &'a self,
        principal: &Principal,
        grants: &[Grant],
        scope: &ResourceScope,
    ) -> Result<Vec<&'a AuditEvent>, Problem> {
        authorize(principal, grants, scope, Action::Read)?;
        Ok(self.events_in(scope))
    }

    pub fn read_page(
        &self,
        principal: &Principal,
        grants: &[Grant],
        scope: &ResourceScope,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<AuditEventPage, Problem> {
        authorize(principal, grants, scope, Action::Read)?;
        if !(1..=100).contains(&limit) {
            return Err(problem(
                "invalid_page_limit",
                "Audit page limit must be between 1 and 100",
                400,
            ));
        }
        let cursor = cursor.map(parse_cursor).transpose()?;
        let mut events = self.events_in(scope);
        events.sort_by(|left, right| {
            right
                .occurred_at
                .cmp(&left.occurred_at)
                .then_with(|| right.event_id.cmp(&left.event_id))
        });
        if let Some((occurred_at, event_id)) = cursor {
            events.retain(|event| {
                (event.occurred_at, event.event_id.as_str()) < (occurred_at, event_id.as_str())
            });
        }
        let has_more = events.len() > limit;
        events.truncate(limit);
        let next_cursor = has_more
            .then(|| {
                events
                    .last()
                    .map(|event| format!("{}:{}", event.occurred_at, event.event_id))
            })
            .flatten();
        Ok(AuditEventPage {
            items: events.into_iter().cloned().collect(),
            next_cursor,
        })
    }

    pub fn issue_support_access(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        access: SupportAccess,
    ) -> Result<(), Problem> {
        authorize(principal, grants, &access.scope, Action::Manage)?;
        if principal.account_id.as_ref() != Some(&access.issued_by) {
            return Err(problem(
                "support_issuer_mismatch",
                "Support issuer does not match authenticated actor",
                403,
            ));
        }
        if self.support_access.contains_key(&access.access_id) {
            return Err(problem(
                "support_access_exists",
                "Support access already exists",
                409,
            ));
        }
        self.support_access.insert(access.access_id.clone(), access);
        Ok(())
    }

    pub fn read_with_support<'a>(
        &'a self,
        actor_id: &AccountId,
        access_id: &str,
        scope: &ResourceScope,
        now: u64,
    ) -> Result<Vec<&'a AuditEvent>, Problem> {
        let access = self
            .support_access
            .get(access_id)
            .ok_or_else(|| problem("support_access_not_found", "Support access not found", 404))?;
        if !access.allows(actor_id, scope, now) {
            return Err(problem(
                "support_access_denied",
                "Support access denied",
                403,
            ));
        }
        Ok(self.events_in(scope))
    }

    pub fn revoke_support_access(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        access_id: &str,
        now: u64,
    ) -> Result<(), Problem> {
        let scope = self
            .support_access
            .get(access_id)
            .ok_or_else(|| problem("support_access_not_found", "Support access not found", 404))?
            .scope
            .clone();
        authorize(principal, grants, &scope, Action::Manage)?;
        self.support_access
            .get_mut(access_id)
            .expect("support access checked above")
            .revoke(now);
        Ok(())
    }

    fn events_in(&self, scope: &ResourceScope) -> Vec<&AuditEvent> {
        self.events
            .iter()
            .filter(|event| scope.contains(&event.resource.scope))
            .collect()
    }
}

fn parse_cursor(value: &str) -> Result<(u64, String), Problem> {
    if value.chars().count() > 280 {
        return Err(problem(
            "invalid_page_cursor",
            "Invalid audit page cursor",
            400,
        ));
    }
    let (timestamp, event_id) = value
        .split_once(':')
        .ok_or_else(|| problem("invalid_page_cursor", "Invalid audit page cursor", 400))?;
    let occurred_at = timestamp
        .parse::<u64>()
        .map_err(|_| problem("invalid_page_cursor", "Invalid audit page cursor", 400))?;
    if event_id.trim().is_empty() || event_id.chars().count() > 256 {
        return Err(problem(
            "invalid_page_cursor",
            "Invalid audit page cursor",
            400,
        ));
    }
    Ok((occurred_at, event_id.to_owned()))
}

fn authorize(
    principal: &Principal,
    grants: &[Grant],
    scope: &ResourceScope,
    action: Action,
) -> Result<(), Problem> {
    let capability = Capability::new(CAPABILITY)
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    if is_allowed(principal, &capability, action, scope, grants) {
        Ok(())
    } else {
        Err(problem("audit_forbidden", "Audit access denied", 403))
    }
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "audit")
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_domain::{
        audit::{AuditOutcome, AuditResource},
        ids::{GrantId, WorkspaceId},
        permissions::{Effect, Role},
    };

    fn fixture() -> (AuditService, Principal, Vec<Grant>, ResourceScope) {
        let account_id = AccountId::from_str("acct_audit_page").unwrap();
        let workspace_id = WorkspaceId::from_str("ws_audit_page").unwrap();
        let scope = ResourceScope::workspace(workspace_id.clone());
        let principal = Principal {
            account_id: Some(account_id.clone()),
            workspace_ids: [workspace_id.clone()].into_iter().collect(),
            roles: [Role::WorkspaceMember].into_iter().collect(),
            ..Principal::default()
        };
        let grants = vec![Grant {
            id: GrantId::from_str("grant_audit_page_read").unwrap(),
            account_id: Some(account_id.clone()),
            member_id: None,
            capability: Capability::new(CAPABILITY).unwrap(),
            action: Action::Read,
            scope: scope.clone(),
            effect: Effect::Allow,
        }];
        let mut audit = AuditService::default();
        for (event_id, occurred_at) in [("event_a", 10), ("event_c", 12), ("event_b", 11)] {
            audit
                .append(
                    AuditEvent::new(
                        event_id,
                        occurred_at,
                        account_id.clone(),
                        AuditResource {
                            resource_type: "workspace".into(),
                            resource_id: workspace_id.as_str().into(),
                            scope: scope.clone(),
                        },
                        "workspace.update",
                        format!("cor_{event_id}"),
                        AuditOutcome::Succeeded,
                        BTreeMap::new(),
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        (audit, principal, grants, scope)
    }

    #[test]
    fn audit_pages_are_newest_first_bounded_and_cursor_stable() {
        let (audit, principal, grants, scope) = fixture();
        let first = audit
            .read_page(&principal, &grants, &scope, None, 2)
            .unwrap();
        assert_eq!(
            first
                .items
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            ["event_c", "event_b"]
        );
        let second = audit
            .read_page(&principal, &grants, &scope, first.next_cursor.as_deref(), 2)
            .unwrap();
        assert_eq!(
            second
                .items
                .iter()
                .map(|event| event.event_id.as_str())
                .collect::<Vec<_>>(),
            ["event_a"]
        );
        assert_eq!(second.next_cursor, None);
    }

    #[test]
    fn audit_page_rejects_invalid_bounds_and_cursor() {
        let (audit, principal, grants, scope) = fixture();
        assert_eq!(
            audit
                .read_page(&principal, &grants, &scope, None, 0)
                .unwrap_err()
                .code,
            "invalid_page_limit"
        );
        assert_eq!(
            audit
                .read_page(&principal, &grants, &scope, Some("not-a-cursor"), 10)
                .unwrap_err()
                .code,
            "invalid_page_cursor"
        );
    }
}
