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
