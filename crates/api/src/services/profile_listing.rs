use buglogin_domain::{
    entities::Profile,
    errors::Problem,
    ids::{ProfileId, WorkspaceId},
    permissions::{Action, Grant, Principal},
};
use buglogin_storage::local::Versioned;
use serde::Serialize;

use crate::{http::ApiState, services::profile_hosted::HostedProfileService};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProfileListQuery {
    pub workspace_id: Option<WorkspaceId>,
    pub cursor: Option<ProfileId>,
    pub limit: usize,
    pub include_archived: bool,
}

impl ProfileListQuery {
    pub fn parse(
        workspace_id: Option<&str>,
        cursor: Option<&str>,
        limit: Option<usize>,
        include_archived: bool,
    ) -> Result<Self, Problem> {
        let limit = limit.unwrap_or(100);
        if !(1..=100).contains(&limit) {
            return Err(problem(
                "invalid_page_limit",
                "Profile page limit must be between 1 and 100",
                400,
            ));
        }
        let workspace_id = workspace_id
            .map(WorkspaceId::from_str)
            .transpose()
            .map_err(|_| problem("invalid_workspace_id", "Invalid workspace ID", 400))?;
        let cursor = cursor
            .map(|cursor| {
                if cursor.is_empty() || cursor.len() > 128 {
                    return Err(());
                }
                ProfileId::from_str(cursor).map_err(|_| ())
            })
            .transpose()
            .map_err(|_| problem("invalid_page_cursor", "Invalid page cursor", 400))?;
        Ok(Self {
            workspace_id,
            cursor,
            limit,
            include_archived,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileListPage {
    pub items: Vec<Versioned<Profile>>,
    pub next_cursor: Option<String>,
}

pub fn list(
    state: &ApiState,
    principal: &Principal,
    grants: &[Grant],
    query: ProfileListQuery,
) -> Result<ProfileListPage, Problem> {
    let batch_size = query.limit.saturating_add(1).max(100);
    let mut scan_cursor = query.cursor;
    let mut visible = Vec::with_capacity(query.limit.saturating_add(1));
    loop {
        let batch = state.list_profile_records_after(scan_cursor.as_ref(), batch_size)?;
        let batch_len = batch.len();
        if batch_len == 0 {
            break;
        }
        let last_scanned = batch.last().map(|profile| profile.value.id.clone());
        for profile in batch {
            let workspace_matches = match query.workspace_id.as_ref() {
                Some(workspace_id) => state
                    .profile_resource_record(&profile.value.id)?
                    .is_some_and(|binding| binding.workspace_id == *workspace_id),
                None => true,
            };
            if workspace_matches
                && (query.include_archived || !profile.value.archived)
                && HostedProfileService::authorized(
                    state,
                    principal,
                    grants,
                    &profile.value.id,
                    Action::Read,
                    "profile.read",
                )?
            {
                visible.push(profile);
                if visible.len() > query.limit {
                    break;
                }
            }
        }
        if visible.len() > query.limit || batch_len < batch_size {
            break;
        }
        scan_cursor = last_scanned;
    }
    let has_more = visible.len() > query.limit;
    visible.truncate(query.limit);
    let next_cursor = has_more
        .then(|| {
            visible
                .last()
                .map(|profile| profile.value.id.as_str().to_owned())
        })
        .flatten();
    Ok(ProfileListPage {
        items: visible,
        next_cursor,
    })
}

fn problem(code: &'static str, title: &'static str, status: u16) -> Problem {
    Problem::new(code, title, status, "profile")
}
