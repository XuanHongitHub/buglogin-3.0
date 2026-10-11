use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use buglogin_domain::{
    errors::Problem,
    grant::ResourceGrant,
    ids::{GrantId, MemberId, ResourceId, WorkspaceId},
    permissions::{Action, Capability, Effect, Grant, Principal, ResourceScope, is_allowed},
    resource::Resource,
};

const RESOURCE_CAPABILITY: &str = "CAP-RES-001";
const SHARING_CAPABILITY: &str = "CAP-SHARE-001";

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct ResourceService {
    resources: BTreeMap<ResourceId, Resource>,
    grants: BTreeMap<GrantId, ResourceGrant>,
}

impl ResourceService {
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
