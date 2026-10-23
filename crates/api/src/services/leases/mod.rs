use buglogin_domain::{
    errors::Problem,
    ids::{ResourceId, WorkspaceId},
    lease::{Lease, LeaseError, LeaseStore},
};

#[derive(Default)]
pub struct LeaseService {
    store: LeaseStore,
}

impl LeaseService {
    pub fn acquire(
        &mut self,
        resource_id: ResourceId,
        workspace_id: WorkspaceId,
        owner_id: impl Into<String>,
        now: u64,
        ttl_seconds: u64,
    ) -> Result<Lease, Problem> {
        self.store
            .acquire(resource_id, workspace_id, owner_id, now, ttl_seconds)
            .map_err(problem)
    }

    pub fn renew(
        &mut self,
        resource_id: &ResourceId,
        owner_id: &str,
        token: &str,
        now: u64,
        ttl_seconds: u64,
    ) -> Result<Lease, Problem> {
        self.store
            .renew(resource_id, owner_id, token, now, ttl_seconds)
            .map_err(problem)
    }

    pub fn release(
        &mut self,
        resource_id: &ResourceId,
        owner_id: &str,
        token: &str,
        now: u64,
    ) -> Result<(), Problem> {
        self.store
            .release(resource_id, owner_id, token, now)
            .map_err(problem)
    }

    pub fn current(&self, resource_id: &ResourceId, now: u64) -> Option<&Lease> {
        self.store.get(resource_id, now)
    }
}

fn problem(error: LeaseError) -> Problem {
    match error {
        LeaseError::Held { expires_at, .. } => {
            Problem::new("resource_lease_held", "Resource is leased", 409, "lease")
                .with_field("expiresAt", expires_at)
        }
        LeaseError::InvalidOwner => {
            Problem::new("invalid_lease_owner", "Invalid lease owner", 400, "lease")
        }
        LeaseError::InvalidTtl => {
            Problem::new("invalid_lease_ttl", "Invalid lease duration", 400, "lease")
        }
        LeaseError::NotFound => Problem::new("lease_not_found", "Lease not found", 404, "lease"),
        LeaseError::NotOwner => {
            Problem::new("lease_not_owner", "Lease owner mismatch", 403, "lease")
        }
        LeaseError::Expired => {
            Problem::new("lease_expired", "Lease expired", 409, "lease").retryable(true)
        }
        LeaseError::TokenGeneration => Problem::new(
            "lease_token_unavailable",
            "Lease token unavailable",
            503,
            "lease",
        )
        .retryable(true),
        LeaseError::TenantMismatch => Problem::new(
            "lease_tenant_mismatch",
            "Lease tenant mismatch",
            403,
            "lease",
        ),
        LeaseError::SequenceExhausted => Problem::new(
            "lease_sequence_exhausted",
            "Lease fencing sequence cannot be incremented",
            503,
            "lease",
        ),
        LeaseError::TimeExhausted => Problem::new(
            "lease_time_exhausted",
            "Lease expiration time cannot be represented",
            503,
            "lease",
        ),
    }
}
