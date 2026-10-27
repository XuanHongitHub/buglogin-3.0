use super::*;

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DeleteTenancyInput {
    expected_revision: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TransferOwnerInput {
    account_id: AccountId,
    expected_revision: u64,
}

pub(super) async fn delete_organization(
    State(state): State<ApiState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<DeleteTenancyInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id = OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    mutate(
        &state, &headers, &principal, &grants, organization_id.as_str(),
        "organization.delete", json!({"organizationId": organization_id, "expectedRevision": input.expected_revision}),
        ResourceScope { organization_id: Some(organization_id.clone()), ..ResourceScope::default() }, organization_id.as_str(),
        || {
            let mut aggregate = state.control_aggregate().map_err(response_problem)?;
            let organization = aggregate.workspaces.authorize_organization_management(&principal, &grants, &organization_id, Action::Delete).map_err(response_problem)?;
            aggregate.organization_members.remove_owned_organization_memberships(&organization_id, &organization.account_id).map_err(response_problem)?;
            aggregate.workspaces.delete_organization(&principal, &grants, &organization_id, input.expected_revision).map_err(response_problem)?;
            aggregate.grants.retain(|grant| grant.scope.organization_id.as_ref() != Some(&organization_id));
            state.replace_control_aggregate(aggregate).map_err(response_problem)?;
            Ok(json!({"organizationId": organization_id, "deleted": true}))
        },
    ).await
}

pub(super) async fn delete_workspace(
    State(state): State<ApiState>,
    Path((organization_id, workspace_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(input): Json<DeleteTenancyInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id = OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    mutate(
        &state, &headers, &principal, &grants, workspace_id.as_str(), "workspace.delete",
        json!({"organizationId": organization_id, "workspaceId": workspace_id, "expectedRevision": input.expected_revision}),
        ResourceScope { organization_id: Some(organization_id.clone()), workspace_id: Some(workspace_id.clone()), ..ResourceScope::default() }, workspace_id.as_str(),
        || {
            let mut aggregate = state.control_aggregate().map_err(response_problem)?;
            // Authorize and validate the parent/revision before inspecting dependencies.
            let organization = aggregate.workspaces.organization_for_workspace(&workspace_id)
                .filter(|organization| organization.id == organization_id)
                .ok_or_else(|| response_problem(problem("workspace_not_found", "Workspace not found", 404)))?;
            let mut workspaces = aggregate.workspaces.clone();
            workspaces.delete_workspace(&principal, &grants, &organization_id, &workspace_id, input.expected_revision).map_err(response_problem)?;
            let dependency = if aggregate.profile_resources.values().any(|profile| profile.workspace_id == workspace_id) {
                Some(("workspace_has_profiles", "Delete or relocate the workspace profiles first"))
            } else if aggregate.resources.has_workspace_records(&workspace_id) {
                Some(("workspace_has_resources", "Remove workspace resources and resource grants first"))
            } else if aggregate.groups.has_workspace_records(&workspace_id) || aggregate.extension_groups.has_workspace_records(&workspace_id) {
                Some(("workspace_has_groups", "Remove workspace groups and their shares first"))
            } else if aggregate.teams.has_workspace_records(&workspace_id) {
                Some(("workspace_has_teams", "Remove workspace teams first"))
            } else if aggregate.operations.has_active_workspace_operations(&workspace_id) {
                Some(("workspace_has_active_operations", "Wait for active workspace operations to finish"))
            } else if aggregate.sync_blob_reservations.values().any(|reservation| reservation.workspace_id == workspace_id.as_str()) {
                Some(("workspace_has_sync_reservations", "Finish or expire pending workspace sync uploads first"))
            } else {
                None
            };
            if let Some((code, title)) = dependency {
                return Err(response_problem(problem(code, title, 409)));
            }
            let removed = aggregate.members.remove_owned_workspace_memberships(&workspace_id, &organization.account_id).map_err(response_problem)?;
            aggregate.grants.retain(|grant| grant.scope.workspace_id.as_ref() != Some(&workspace_id) && !grant.member_id.as_ref().is_some_and(|id| removed.contains(id)));
            for id in &removed { aggregate.materialized_member_grants.remove(id); }
            aggregate.workspaces = workspaces;
            state.replace_control_aggregate(aggregate).map_err(response_problem)?;
            Ok(json!({"workspaceId": workspace_id, "deleted": true}))
        },
    ).await
}

pub(super) async fn transfer_owner(
    State(state): State<ApiState>,
    Path(organization_id): Path<String>,
    headers: HeaderMap,
    Json(input): Json<TransferOwnerInput>,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let organization_id = OrganizationId::from_str(&organization_id).map_err(|_| bad_id("organization_id"))?;
    mutate(
        &state, &headers, &principal, &grants, organization_id.as_str(), "organization.owner.transfer",
        json!({"organizationId": organization_id, "accountId": input.account_id, "expectedRevision": input.expected_revision}),
        ResourceScope { organization_id: Some(organization_id.clone()), ..ResourceScope::default() }, organization_id.as_str(),
        || {
            let mut aggregate = state.control_aggregate().map_err(response_problem)?;
            aggregate.workspaces.authorize_organization_management(&principal, &grants, &organization_id, Action::Manage).map_err(response_problem)?;
            let active = state.account_persistence.load(&input.account_id).map_err(response_problem)?
                .is_some_and(|account| account.status == AccountStatus::Active);
            let identity = state.identity_persistence.load(&input.account_id).map_err(response_problem)?;
            let membership = aggregate.organization_members.get(&organization_id, &input.account_id);
            if !active || identity.is_none() || !membership.is_some_and(|member| member.status == OrganizationMemberStatus::Active) {
                return Err(response_problem(problem("owner_target_not_eligible", "The new owner must be an active identified account and active member of this organization", 409)));
            }
            // Existing workspace memberships and explicit grants remain unchanged.
            // Only organization ownership changes; no new platform authority is granted.
            let organization = aggregate.workspaces.transfer_organization_owner(&principal, &grants, &organization_id, &input.account_id, input.expected_revision).map_err(response_problem)?;
            state.replace_control_aggregate(aggregate).map_err(response_problem)?;
            Ok(organization)
        },
    ).await
}
