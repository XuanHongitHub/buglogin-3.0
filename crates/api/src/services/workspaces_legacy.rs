use buglogin_domain::{errors::Problem, ids::WorkspaceId, workspace::Workspace};
use std::collections::HashMap;

#[derive(Default)]
pub struct WorkspaceService { workspaces: HashMap<WorkspaceId, Workspace> }

impl WorkspaceService {
    pub fn insert(&mut self, workspace: Workspace) -> Result<(), Problem> { if self.workspaces.insert(workspace.id.clone(), workspace).is_some() { return Err(Problem::new("workspace_exists", "Workspace already exists", 409, "workspace")); } Ok(()) }
    pub fn get_for_account(&self, id: &WorkspaceId, organization_id: &buglogin_domain::ids::OrganizationId) -> Result<&Workspace, Problem> { let workspace = self.workspaces.get(id).ok_or_else(|| Problem::new("workspace_not_found", "Workspace not found", 404, "workspace"))?; if &workspace.organization_id != organization_id { return Err(Problem::new("workspace_forbidden", "Workspace access denied", 403, "workspace")); } Ok(workspace) }
}
