use serde::{Deserialize, Serialize};

use std::collections::{BTreeMap, BTreeSet, Bound};

use buglogin_domain::{
    errors::Problem,
    grant::ResourceGrant,
    ids::{GrantId, MemberId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, is_allowed},
    resource::{Resource, ResourceKind, ResourceState},
};

const RESOURCE_CAPABILITY: &str = "CAP-RES-001";
const SHARING_CAPABILITY: &str = "CAP-SHARE-001";

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct ResourceService {
    resources: BTreeMap<ResourceId, Resource>,
    grants: BTreeMap<GrantId, ResourceGrant>,
}

impl ResourceService {
    pub(crate) fn reconcile_managed_profile_registration(
        &mut self,
        resource: Resource,
    ) -> Result<(), Problem> {
        if let Some(existing) = self.resources.get(&resource.id) {
            if existing.workspace_id == resource.workspace_id
                && existing.kind == ResourceKind::Profile
                && existing.state == ResourceState::Active
            {
                return Ok(());
            }
            return Err(problem(
                "resource_registration_conflict",
                "Managed profile resource registration conflicts with existing resource state",
                409,
            ));
        }
        self.resources.insert(resource.id.clone(), resource);
        Ok(())
    }

    pub(crate) fn purge_resource(&mut self, resource_id: &ResourceId) {
        self.grants
            .retain(|_, grant| &grant.resource_id != resource_id);
        self.resources.remove(resource_id);
    }

    pub fn register(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        resource: Resource,
    ) -> Result<(), Problem> {
        authorize_resource(
            principal,
            grants,
            &resource.workspace_id,
            None,
            Action::Create,
        )?;
        if self.resources.contains_key(&resource.id) {
            return Err(problem("resource_exists", "Resource exists", 409));
        }
        self.resources.insert(resource.id.clone(), resource);
        Ok(())
    }

    pub fn get(
        &self,
        principal: &Principal,
        grants: &[Grant],
        id: &ResourceId,
        workspace: &WorkspaceId,
    ) -> Result<Resource, Problem> {
        authorize_resource(principal, grants, workspace, Some(id), Action::Read)?;
        let resource = self
            .resources
            .get(id)
            .ok_or_else(|| problem("resource_not_found", "Resource not found", 404))?;
        if &resource.workspace_id != workspace {
            return Err(problem("resource_forbidden", "Resource access denied", 403));
        }
        Ok(resource.clone())
    }

    pub fn list(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace: &WorkspaceId,
    ) -> Result<Vec<Resource>, Problem> {
        authorize_resource(principal, grants, workspace, None, Action::Read)?;
        let items = self
            .resources
            .values()
            .filter(|resource| &resource.workspace_id == workspace)
            .cloned()
            .collect();
        Ok(items)
    }

    pub fn list_page(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace: &WorkspaceId,
        after: Option<&ResourceId>,
        limit: usize,
    ) -> Result<(Vec<Resource>, Option<ResourceId>), Problem> {
        self.list_page_filtered(principal, grants, workspace, after, limit, None, None, None)
    }

    pub fn list_page_filtered(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace: &WorkspaceId,
        after: Option<&ResourceId>,
        limit: usize,
        search: Option<&str>,
        kind: Option<&ResourceKind>,
        state: Option<&ResourceState>,
    ) -> Result<(Vec<Resource>, Option<ResourceId>), Problem> {
        if !(1..=100).contains(&limit) {
            return Err(problem(
                "invalid_page_limit",
                "Page limit must be between 1 and 100",
                400,
            ));
        }
        authorize_resource(principal, grants, workspace, None, Action::Read)?;
        let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
        let search = search.map(str::to_lowercase);
        let mut rows = self
            .resources
            .range((lower, Bound::Unbounded))
            .filter(|(_, resource)| {
                &resource.workspace_id == workspace
                    && kind.is_none_or(|kind| &resource.kind == kind)
                    && state.is_none_or(|state| &resource.state == state)
                    && search
                        .as_ref()
                        .is_none_or(|search| resource.display_name.to_lowercase().contains(search))
            })
            .map(|(_, resource)| resource);
        let items = rows.by_ref().take(limit).cloned().collect::<Vec<_>>();
        let has_more = rows.next().is_some();
        let next_cursor = if has_more {
            items.last().map(|resource| resource.id.clone())
        } else {
            None
        };
        Ok((items, next_cursor))
    }

    pub fn grant(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        grant: ResourceGrant,
    ) -> Result<(), Problem> {
        authorize_sharing(
            principal,
            grants,
            &grant.workspace_id,
            Some(&grant.resource_id),
            Action::Manage,
        )?;
        let resource = self
            .resources
            .get(&grant.resource_id)
            .ok_or_else(|| problem("resource_not_found", "Resource not found", 404))?;
        if resource.workspace_id != grant.workspace_id {
            return Err(problem("tenant_mismatch", "Grant tenant mismatch", 403));
        }
        if self.grants.contains_key(&grant.id) {
            return Err(problem("grant_exists", "Grant exists", 409));
        }
        self.grants.insert(grant.id.clone(), grant);
        Ok(())
    }

    pub fn get_grant(
        &self,
        principal: &Principal,
        grants: &[Grant],
        id: &GrantId,
    ) -> Result<ResourceGrant, Problem> {
        let grant = self
            .grants
            .get(id)
            .ok_or_else(|| problem("grant_not_found", "Grant not found", 404))?;
        authorize_sharing(
            principal,
            grants,
            &grant.workspace_id,
            Some(&grant.resource_id),
            Action::Manage,
        )?;
        Ok(grant.clone())
    }

    pub fn list_grants(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace: &WorkspaceId,
        resource: Option<&ResourceId>,
        member: Option<&MemberId>,
    ) -> Result<Vec<ResourceGrant>, Problem> {
        authorize_sharing(principal, grants, workspace, resource, Action::Read)?;
        Ok(self
            .grants
            .values()
            .filter(|grant| &grant.workspace_id == workspace)
            .filter(|grant| resource.is_none_or(|id| &grant.resource_id == id))
            .filter(|grant| member.is_none_or(|id| &grant.member_id == id))
            .cloned()
            .collect())
    }

    pub fn list_grants_page(
        &self,
        principal: &Principal,
        grants: &[Grant],
        workspace: &WorkspaceId,
        resource: Option<&ResourceId>,
        member: Option<&MemberId>,
        after: Option<&GrantId>,
        limit: usize,
    ) -> Result<(Vec<ResourceGrant>, Option<GrantId>), Problem> {
        if !(1..=100).contains(&limit) {
            return Err(problem(
                "invalid_page_limit",
                "Page limit must be between 1 and 100",
                400,
            ));
        }
        authorize_sharing(principal, grants, workspace, resource, Action::Read)?;
        let lower = after.map_or(Bound::Unbounded, Bound::Excluded);
        let mut rows = self
            .grants
            .range((lower, Bound::Unbounded))
            .filter(|(_, grant)| &grant.workspace_id == workspace)
            .filter(|(_, grant)| resource.is_none_or(|id| &grant.resource_id == id))
            .filter(|(_, grant)| member.is_none_or(|id| &grant.member_id == id))
            .map(|(_, grant)| grant);
        let items = rows.by_ref().take(limit).cloned().collect::<Vec<_>>();
        let has_more = rows.next().is_some();
        let next_cursor = if has_more {
            items.last().map(|grant| grant.id.clone())
        } else {
            None
        };
        Ok((items, next_cursor))
    }

    pub fn revoke(
        &mut self,
        principal: &Principal,
        grants: &[Grant],
        id: &GrantId,
    ) -> Result<(), Problem> {
        let grant = self
            .grants
            .get(id)
            .ok_or_else(|| problem("grant_not_found", "Grant not found", 404))?;
        authorize_sharing(
            principal,
            grants,
            &grant.workspace_id,
            Some(&grant.resource_id),
            Action::Manage,
        )?;
        self.grants.remove(id);
        Ok(())
    }

    pub fn has_grant(&self, resource: &ResourceId, member: &MemberId) -> bool {
        self.grants.values().any(|grant| {
            &grant.resource_id == resource
                && &grant.member_id == member
                && grant.effect == Effect::Allow
        })
    }

    pub fn has_active_profile_shares(
        &self,
        resource: &ResourceId,
        workspace: &WorkspaceId,
    ) -> bool {
        self.grants.values().any(|grant| {
            &grant.resource_id == resource
                && &grant.workspace_id == workspace
                && grant.effect == Effect::Allow
                && matches!(grant.action, Action::Read | Action::Execute)
        })
    }

    pub fn active_profile_share_members(
        &self,
        resource: &ResourceId,
        workspace: &WorkspaceId,
    ) -> BTreeSet<MemberId> {
        self.grants
            .values()
            .filter(|grant| {
                &grant.resource_id == resource
                    && &grant.workspace_id == workspace
                    && grant.effect == Effect::Allow
                    && matches!(grant.action, Action::Read | Action::Execute)
            })
            .map(|grant| grant.member_id.clone())
            .collect()
    }

    pub fn exists_in_workspace(&self, resource: &ResourceId, workspace: &WorkspaceId) -> bool {
        self.resources
            .get(resource)
            .is_some_and(|value| &value.workspace_id == workspace)
    }

    pub fn is_allowed(&self, resource: &ResourceId, member: &MemberId, action: Action) -> bool {
        let mut matching = self.grants.values().filter(|grant| {
            &grant.resource_id == resource && &grant.member_id == member && grant.action == action
        });
        if matching.clone().any(|grant| grant.effect == Effect::Deny) {
            return false;
        }
        matching.any(|grant| grant.effect == Effect::Allow)
    }

    pub fn is_allowed_in_workspace(
        &self,
        resource: &ResourceId,
        workspace: &WorkspaceId,
        member: &MemberId,
        action: Action,
    ) -> bool {
        if !self
            .resources
            .get(resource)
            .is_some_and(|value| &value.workspace_id == workspace)
        {
            return false;
        }
        let mut matching = self.grants.values().filter(|grant| {
            &grant.workspace_id == workspace
                && &grant.resource_id == resource
                && &grant.member_id == member
                && grant.action == action
        });
        if matching.clone().any(|grant| grant.effect == Effect::Deny) {
            return false;
        }
        matching.any(|grant| grant.effect == Effect::Allow)
    }

    pub fn is_denied_in_workspace(
        &self,
        resource: &ResourceId,
        workspace: &WorkspaceId,
        member: &MemberId,
        action: Action,
    ) -> bool {
        self.resources
            .get(resource)
            .is_some_and(|value| &value.workspace_id == workspace)
            && self.grants.values().any(|grant| {
                &grant.workspace_id == workspace
                    && &grant.resource_id == resource
                    && &grant.member_id == member
                    && grant.action == action
                    && grant.effect == Effect::Deny
            })
    }
}

fn authorize_capability(
    principal: &Principal,
    grants: &[Grant],
    workspace: &WorkspaceId,
    resource: Option<&ResourceId>,
    action: Action,
    capability_name: &str,
) -> Result<(), Problem> {
    let capability = Capability::new(capability_name)
        .map_err(|_| problem("invalid_capability", "Invalid capability", 500))?;
    let scope = match resource {
        Some(resource_id) => ResourceScope::resource(workspace.clone(), resource_id.clone()),
        None => ResourceScope::workspace(workspace.clone()),
    };
    if is_allowed(principal, &capability, action, &scope, grants) {
        Ok(())
    } else {
        Err(problem("resource_forbidden", "Resource access denied", 403))
    }
}

fn authorize_resource(
    principal: &Principal,
    grants: &[Grant],
    workspace: &WorkspaceId,
    resource: Option<&ResourceId>,
    action: Action,
) -> Result<(), Problem> {
    authorize_capability(
        principal,
        grants,
        workspace,
        resource,
        action,
        RESOURCE_CAPABILITY,
    )
}

fn authorize_sharing(
    principal: &Principal,
    grants: &[Grant],
    workspace: &WorkspaceId,
    resource: Option<&ResourceId>,
    action: Action,
) -> Result<(), Problem> {
    authorize_capability(
        principal,
        grants,
        workspace,
        resource,
        action,
        SHARING_CAPABILITY,
    )
}

fn problem(code: &str, title: &str, status: u16) -> Problem {
    Problem::new(code, title, status, "resource")
}

#[cfg(test)]
mod tests {
    use super::*;
    use buglogin_domain::{
        ids::AccountId,
        resource::{ResourceKind, ResourceState},
    };
    use std::collections::BTreeSet;

    fn workspace(value: &str) -> WorkspaceId {
        WorkspaceId::new(value).unwrap()
    }

    fn resource(value: &str) -> ResourceId {
        ResourceId::new(value).unwrap()
    }

    fn member(value: &str) -> MemberId {
        MemberId::new(value).unwrap()
    }

    fn principal(account_id: &str, workspace_id: &WorkspaceId) -> Principal {
        Principal {
            account_id: Some(AccountId::new(account_id).unwrap()),
            member_id: None,
            workspace_ids: BTreeSet::from([workspace_id.clone()]),
            roles: BTreeSet::new(),
            token_scopes: None,
        }
    }

    fn acl(
        account_id: &str,
        id: &str,
        capability: &str,
        action: Action,
        workspace_id: &WorkspaceId,
        resource_id: Option<&ResourceId>,
    ) -> Grant {
        Grant {
            id: GrantId::new(id).unwrap(),
            account_id: Some(AccountId::new(account_id).unwrap()),
            member_id: None,
            capability: Capability::new(capability).unwrap(),
            action,
            scope: match resource_id {
                Some(resource_id) => {
                    ResourceScope::resource(workspace_id.clone(), resource_id.clone())
                }
                None => ResourceScope::workspace(workspace_id.clone()),
            },
            effect: Effect::Allow,
        }
    }

    fn stored_grant(
        id: &str,
        workspace_id: &WorkspaceId,
        resource_id: &ResourceId,
        member_id: &MemberId,
    ) -> ResourceGrant {
        ResourceGrant {
            id: GrantId::new(id).unwrap(),
            workspace_id: workspace_id.clone(),
            resource_id: resource_id.clone(),
            member_id: member_id.clone(),
            action: Action::Read,
            effect: Effect::Allow,
        }
    }

    fn service_with_grants() -> (ResourceService, WorkspaceId, ResourceId, MemberId) {
        let workspace_id = workspace("ws_one");
        let other_workspace_id = workspace("ws_two");
        let resource_id = resource("res_one");
        let other_resource_id = resource("res_two");
        let third_resource_id = resource("res_three");
        let member_id = member("mem_one");
        let other_member_id = member("mem_two");
        let mut service = ResourceService::default();

        for (workspace_id, resource_id, display_name) in [
            (&workspace_id, &resource_id, "One"),
            (&workspace_id, &other_resource_id, "Two"),
            (&other_workspace_id, &third_resource_id, "Three"),
        ] {
            service.resources.insert(
                resource_id.clone(),
                Resource {
                    id: resource_id.clone(),
                    workspace_id: workspace_id.clone(),
                    kind: ResourceKind::Profile,
                    state: ResourceState::Active,
                    display_name: display_name.into(),
                },
            );
        }
        for grant in [
            stored_grant("grant_one", &workspace_id, &resource_id, &member_id),
            stored_grant(
                "grant_two",
                &workspace_id,
                &other_resource_id,
                &other_member_id,
            ),
            stored_grant(
                "grant_three",
                &other_workspace_id,
                &third_resource_id,
                &member_id,
            ),
        ] {
            service.grants.insert(grant.id.clone(), grant);
        }
        (service, workspace_id, resource_id, member_id)
    }

    #[test]
    fn resource_pages_are_bounded_ordered_and_workspace_scoped() {
        let (service, workspace_id, _, _) = service_with_grants();
        let principal = principal("acct_one", &workspace_id);
        let grants = [acl(
            "acct_one",
            "acl_resource_page",
            RESOURCE_CAPABILITY,
            Action::Read,
            &workspace_id,
            None,
        )];

        let (first, cursor) = service
            .list_page(&principal, &grants, &workspace_id, None, 1)
            .unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].id.as_str(), "res_one");
        assert_eq!(cursor.as_ref().map(ResourceId::as_str), Some("res_one"));

        let (second, next_cursor) = service
            .list_page(&principal, &grants, &workspace_id, cursor.as_ref(), 1)
            .unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].id.as_str(), "res_two");
        assert!(next_cursor.is_none());
    }

    #[test]
    fn resource_page_filters_search_kind_and_state_before_pagination() {
        let (mut service, workspace_id, _, _) = service_with_grants();
        let filtered_id = resource("res_proxy_locked");
        service.resources.insert(
            filtered_id.clone(),
            Resource {
                id: filtered_id,
                workspace_id: workspace_id.clone(),
                kind: ResourceKind::Proxy,
                state: ResourceState::Locked,
                display_name: "North Proxy".into(),
            },
        );
        let principal = principal("acct_one", &workspace_id);
        let grants = [acl(
            "acct_one",
            "acl_resource_filter",
            RESOURCE_CAPABILITY,
            Action::Read,
            &workspace_id,
            None,
        )];

        let (items, next_cursor) = service
            .list_page_filtered(
                &principal,
                &grants,
                &workspace_id,
                None,
                1,
                Some("nOrTh"),
                Some(&ResourceKind::Proxy),
                Some(&ResourceState::Locked),
            )
            .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.as_str(), "res_proxy_locked");
        assert!(next_cursor.is_none());
    }

    #[test]
    fn sharing_operations_require_sharing_capability() {
        let (service, workspace_id, _, _) = service_with_grants();
        let principal = principal("acct_one", &workspace_id);
        let error = service
            .list_grants(
                &principal,
                &[acl(
                    "acct_one",
                    "acl_resource_only",
                    RESOURCE_CAPABILITY,
                    Action::Read,
                    &workspace_id,
                    None,
                )],
                &workspace_id,
                None,
                None,
            )
            .unwrap_err();

        assert_eq!(error.code, "resource_forbidden");
        assert_eq!(error.status, 403);
    }

    #[test]
    fn sharing_operations_allow_sharing_capability() {
        let (mut service, workspace_id, resource_id, member_id) = service_with_grants();
        let principal = principal("acct_one", &workspace_id);
        let grants = [
            acl(
                "acct_one",
                "acl_share_read",
                SHARING_CAPABILITY,
                Action::Read,
                &workspace_id,
                None,
            ),
            acl(
                "acct_one",
                "acl_share_manage",
                SHARING_CAPABILITY,
                Action::Manage,
                &workspace_id,
                Some(&resource_id),
            ),
        ];

        assert_eq!(
            service
                .list_grants(&principal, &grants, &workspace_id, None, None)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            service
                .get_grant(&principal, &grants, &GrantId::new("grant_one").unwrap(),)
                .unwrap()
                .id,
            GrantId::new("grant_one").unwrap()
        );

        let created = ResourceGrant {
            id: GrantId::new("grant_created").unwrap(),
            workspace_id: workspace_id.clone(),
            resource_id: resource_id.clone(),
            member_id: member_id.clone(),
            action: Action::Read,
            effect: Effect::Allow,
        };
        service.grant(&principal, &grants, created.clone()).unwrap();
        service.revoke(&principal, &grants, &created.id).unwrap();
        assert!(service.get_grant(&principal, &grants, &created.id).is_err());
    }

    #[test]
    fn list_grants_is_workspace_scoped() {
        let (service, workspace_id, _, _) = service_with_grants();
        let items = service
            .list_grants(
                &principal("acct_one", &workspace_id),
                &[acl(
                    "acct_one",
                    "acl_share_read",
                    SHARING_CAPABILITY,
                    Action::Read,
                    &workspace_id,
                    None,
                )],
                &workspace_id,
                None,
                None,
            )
            .unwrap();

        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|grant| grant.workspace_id == workspace_id));
    }

    #[test]
    fn grant_pages_are_bounded_filtered_and_continue_by_id() {
        let (service, workspace_id, resource_id, member_id) = service_with_grants();
        let principal = principal("acct_one", &workspace_id);
        let grants = [acl(
            "acct_one",
            "acl_share_page_read",
            SHARING_CAPABILITY,
            Action::Read,
            &workspace_id,
            None,
        )];
        let (first, cursor) = service
            .list_grants_page(&principal, &grants, &workspace_id, None, None, None, 1)
            .unwrap();
        assert_eq!(first[0].id.as_str(), "grant_one");
        assert_eq!(cursor.as_ref().map(GrantId::as_str), Some("grant_one"));

        let (second, next_cursor) = service
            .list_grants_page(
                &principal,
                &grants,
                &workspace_id,
                None,
                None,
                cursor.as_ref(),
                1,
            )
            .unwrap();
        assert_eq!(second[0].id.as_str(), "grant_two");
        assert!(next_cursor.is_none());

        let (filtered, filtered_cursor) = service
            .list_grants_page(
                &principal,
                &grants,
                &workspace_id,
                Some(&resource_id),
                Some(&member_id),
                None,
                100,
            )
            .unwrap();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id.as_str(), "grant_one");
        assert!(filtered_cursor.is_none());
    }

    #[test]
    fn list_grants_filters_resource_and_member() {
        let (service, workspace_id, resource_id, member_id) = service_with_grants();
        let items = service
            .list_grants(
                &principal("acct_one", &workspace_id),
                &[acl(
                    "acct_one",
                    "acl_share_read_resource",
                    SHARING_CAPABILITY,
                    Action::Read,
                    &workspace_id,
                    Some(&resource_id),
                )],
                &workspace_id,
                Some(&resource_id),
                Some(&member_id),
            )
            .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, GrantId::new("grant_one").unwrap());
    }
}
