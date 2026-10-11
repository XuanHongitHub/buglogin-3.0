pub mod bulk;
pub mod groups;
use buglogin_domain::{
    entities::Profile, errors::Problem, ids::ProfileId, profile::ProfileResource,
};
use buglogin_storage::local::{LocalRepository, Versioned};

#[derive(Clone)]
pub struct ProfileService {
    repository: Option<LocalRepository>,
}

#[derive(Clone)]
pub struct ProfileBindingService {
    repository: LocalRepository,
}

impl ProfileBindingService {
    pub fn new(repository: LocalRepository) -> Self {
        Self { repository }
    }

    pub fn create(&self, binding: ProfileResource) -> Result<Versioned<ProfileResource>, Problem> {
        self.repository.create(binding)
    }

    pub fn get(
        &self,
        profile_id: &ProfileId,
    ) -> Result<Option<Versioned<ProfileResource>>, Problem> {
        self.repository.get(profile_id.as_str())
    }

    pub fn update(
        &self,
        expected_revision: u64,
        binding: ProfileResource,
    ) -> Result<Versioned<ProfileResource>, Problem> {
        self.repository.update(expected_revision, binding)
    }

    pub fn list(&self) -> Result<Vec<Versioned<ProfileResource>>, Problem> {
        self.repository.list()
    }
}

impl ProfileService {
    pub fn new(repository: LocalRepository) -> Self {
        Self {
            repository: Some(repository),
        }
    }
    pub fn disabled() -> Self {
        Self { repository: None }
    }
    pub fn repository_root(&self) -> Result<&std::path::Path, Problem> {
        Ok(self.repository()?.root())
    }
    pub fn create(&self, profile: Profile) -> Result<Versioned<Profile>, Problem> {
        self.repository()?.create(profile)
    }
    pub fn get(&self, id: &ProfileId) -> Result<Option<Versioned<Profile>>, Problem> {
        self.repository()?.get(id.as_str())
    }
    pub fn list(&self) -> Result<Vec<Versioned<Profile>>, Problem> {
        self.repository()?.list()
    }
    pub fn update(
        &self,
        expected_revision: u64,
        profile: Profile,
    ) -> Result<Versioned<Profile>, Problem> {
        self.repository()?.update(expected_revision, profile)
    }
    pub fn archive(
        &self,
        expected_revision: u64,
        mut profile: Profile,
    ) -> Result<Versioned<Profile>, Problem> {
        profile.archived = true;
        self.update(expected_revision, profile)
    }
    pub fn restore(
        &self,
        expected_revision: u64,
        mut profile: Profile,
    ) -> Result<Versioned<Profile>, Problem> {
        profile.archived = false;
        self.update(expected_revision, profile)
    }

    pub fn delete(
        &self,
        expected_revision: u64,
        profile: &Profile,
    ) -> Result<std::path::PathBuf, Problem> {
        if matches!(
            profile.state,
            buglogin_domain::entities::ProfileState::Running
                | buglogin_domain::entities::ProfileState::Launching
                | buglogin_domain::entities::ProfileState::Stopping
        ) {
            return Err(Problem::new(
                "profile_running",
                "Running profile cannot be deleted",
                409,
                "profile",
            ));
        }
        self.repository()?
            .quarantine::<Profile>(expected_revision, profile.id.as_str())
    }

    fn repository(&self) -> Result<&LocalRepository, Problem> {
        self.repository.as_ref().ok_or_else(|| {
            Problem::new(
                "local_profile_authority_disabled",
                "Local profile authority is disabled",
                409,
                "profile",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_profile_service_never_opens_local_authority() {
        let service = ProfileService::disabled();
        assert_eq!(
            service.repository_root().unwrap_err().code,
            "local_profile_authority_disabled"
        );
        assert_eq!(
            service.list().unwrap_err().code,
            "local_profile_authority_disabled"
        );
    }
}
