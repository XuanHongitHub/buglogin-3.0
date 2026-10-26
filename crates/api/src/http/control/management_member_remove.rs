use super::*;

pub(super) async fn remove_member(
    State(state): State<ApiState>,
    Path((workspace_id, member_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    let principal = authenticate(&state, &headers)?;
    let grants = grants(&state)?;
    let workspace_id = WorkspaceId::from_str(&workspace_id).map_err(|_| bad_id("workspace_id"))?;
    let member_id = MemberId::from_str(&member_id).map_err(|_| bad_id("member_id"))?;
    let (member_grants, organization_owner) = workspace_member_management_grants(
        &state, &principal, &grants, &workspace_id, Action::Manage, correlation(&headers),
    )?;
    let target = state.members.read().map_err(|_| service_unavailable("member"))?
        .get_in_workspace(&principal, &member_grants, &member_id, &workspace_id)
        .map_err(response_problem)?;
    if organization_owner && target.role == MemberRole::Owner {
        return Err(problem_response("role_escalation", "Organization owner cannot remove a workspace Owner membership", 403, correlation(&headers)));
    }
    let value = json!({ "workspaceId": workspace_id, "memberId": member_id });
    mutate(
        &state, &headers, &principal, &grants, workspace_id.as_str(), "member.remove",
        value, ResourceScope::workspace(workspace_id.clone()), member_id.as_str(),
        || {
            // Validate before key rotation, and reuse the existing membership
            // revocation path so encrypted profiles cannot retain stale access.
            HostedProfileService::suspend_member(&state, &principal, &member_grants, &workspace_id, &member_id)
                .map_err(response_problem)?;
            state.members.write().map_err(|_| service_unavailable("member"))?
                .remove(&principal, &member_grants, &workspace_id, &member_id)
                .map_err(response_problem)?;
            state.resources.write().map_err(|_| service_unavailable("resource"))?
                .remove_member_references(&workspace_id, &member_id);
            state.groups.write().map_err(|_| service_unavailable("group"))?
                .remove_member_references(&workspace_id, &member_id);
            state.teams.write().map_err(|_| service_unavailable("team"))?
                .remove_member_references(&workspace_id, &member_id);
            state.grants.write().map_err(|_| service_unavailable("grant"))?
                .retain(|grant| grant.member_id.as_ref() != Some(&member_id));
            state.materialized_member_grants.write().map_err(|_| service_unavailable("member"))?
                .remove(&member_id);
            Ok(json!({ "workspaceId": workspace_id, "memberId": member_id, "removed": true }))
        },
    ).await
}
