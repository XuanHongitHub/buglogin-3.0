use std::collections::BTreeMap;

use buglogin_domain::{
    errors::Problem,
    ids::{AccountId, OrganizationId},
    organization::{OrganizationMember, OrganizationMemberStatus},
};
use serde::{Deserialize, Serialize};

/// Durable organization-level parent memberships. Workspace roles and grants stay
/// in their existing workspace records; this service only gates that access.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct OrganizationMemberService {
    #[serde(default)]
    members: BTreeMap<String, OrganizationMember>,
}

impl OrganizationMemberService {
    pub(crate) fn remove_owned_organization_memberships(
        &mut self,
        organization_id: &OrganizationId,
        owner_id: &AccountId,
    ) -> Result<(), Problem> {
        if self.members.values().any(|member| &member.organization_id == organization_id && &member.account_id != owner_id) {
            return Err(problem("organization_has_members", "Remove organization members before deleting the organization", 409));
        }
        self.members.retain(|_, member| &member.organization_id != organization_id);
        Ok(())
    }

    pub fn get(
        &self,
        organization_id: &OrganizationId,
        account_id: &AccountId,
    ) -> Option<&OrganizationMember> {
        self.members.values().find(|member| {
            &member.organization_id == organization_id && &member.account_id == account_id
        })
    }

    pub fn backfill_active(
        &mut self,
        memberships: impl IntoIterator<Item = (OrganizationId, AccountId)>,
    ) -> usize {
        let mut inserted = 0;
        for (organization_id, account_id) in memberships {
            if self.get(&organization_id, &account_id).is_some() {
                continue;
            }
            let member = OrganizationMember::active(organization_id, account_id);
            self.members.insert(member.id.as_str().to_owned(), member);
            inserted += 1;
        }
        inserted
    }

    pub fn list_for_organization(
        &self,
        organization_id: &OrganizationId,
    ) -> Vec<OrganizationMember> {
        self.members
            .values()
            .filter(|member| &member.organization_id == organization_id)
            .cloned()
            .collect()
    }

    pub fn create_active(
        &mut self,
        organization_id: OrganizationId,
        account_id: AccountId,
    ) -> Result<OrganizationMember, Problem> {
        if self.get(&organization_id, &account_id).is_some() {
            return Err(problem(
                "organization_member_exists",
                "Account is already a member of this organization",
                409,
            ));
        }
        let member = OrganizationMember::active(organization_id, account_id);
        self.members
            .insert(member.id.as_str().to_owned(), member.clone());
        Ok(member)
    }

    pub fn update_status(
        &mut self,
        organization_id: &OrganizationId,
        account_id: &AccountId,
        expected_revision: u64,
        status: OrganizationMemberStatus,
    ) -> Result<OrganizationMember, Problem> {
        let current = self
            .get(organization_id, account_id)
            .cloned()
            .ok_or_else(|| {
                problem(
                    "organization_member_not_found",
                    "Organization member not found",
                    404,
                )
            })?;
        let updated = current
            .transition_status(expected_revision, status)
            .map_err(|error| match error {
                buglogin_domain::organization::OrganizationMemberTransitionError::RevisionConflict => {
                    problem(
                        "revision_conflict",
                        "Organization member revision conflict",
                        409,
                    )
                }
                buglogin_domain::organization::OrganizationMemberTransitionError::RevisionExhausted => {
                    problem("revision_exhausted", "Revision exhausted", 409)
                }
            })?;
        self.members
            .insert(updated.id.as_str().to_owned(), updated.clone());
        Ok(updated)
    }

    pub fn suspended_organizations_for(&self, account_id: &AccountId) -> Vec<OrganizationId> {
        self.members
            .values()
            .filter(|member| {
                &member.account_id == account_id
                    && member.status == OrganizationMemberStatus::Suspended
            })
            .map(|member| member.organization_id.clone())
            .collect()
    }
}

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "organization-member")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_backfill_is_idempotent_and_preserves_suspension() {
        let organization = OrganizationId::new("org_backfill").unwrap();
        let account = AccountId::new("acct_backfill").unwrap();
        let mut members = OrganizationMemberService::default();

        assert_eq!(
            members.backfill_active([(organization.clone(), account.clone())]),
            1
        );
        let active = members.get(&organization, &account).unwrap().clone();
        let suspended = active
            .transition_status(1, OrganizationMemberStatus::Suspended)
            .unwrap();
        members
            .members
            .insert(suspended.id.as_str().into(), suspended);

        assert_eq!(
            members.backfill_active([(organization.clone(), account.clone())]),
            0
        );
        assert_eq!(members.get(&organization, &account).unwrap().revision, 2);
        assert_eq!(
            members.suspended_organizations_for(&account),
            vec![organization]
        );
    }

    #[test]
    fn create_and_status_update_are_revisioned_and_duplicate_safe() {
        let organization = OrganizationId::new("org_mutation").unwrap();
        let account = AccountId::new("acct_mutation").unwrap();
        let mut members = OrganizationMemberService::default();
        let created = members
            .create_active(organization.clone(), account.clone())
            .unwrap();
        assert_eq!(created.status, OrganizationMemberStatus::Active);
        assert_eq!(created.revision, 1);
        assert_eq!(
            members
                .create_active(organization.clone(), account.clone())
                .unwrap_err()
                .code,
            "organization_member_exists"
        );

        let suspended = members
            .update_status(
                &organization,
                &account,
                1,
                OrganizationMemberStatus::Suspended,
            )
            .unwrap();
        assert_eq!(suspended.status, OrganizationMemberStatus::Suspended);
        assert_eq!(suspended.revision, 2);
        assert_eq!(
            members
                .update_status(&organization, &account, 1, OrganizationMemberStatus::Active,)
                .unwrap_err()
                .code,
            "revision_conflict"
        );
    }
}
