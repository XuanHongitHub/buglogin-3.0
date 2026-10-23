/// Validate that production object storage uses the application-owned
/// namespace documented for the shared BugLogin 3.0 bucket.
pub fn validate_production_s3_namespace(prefix: &str, tenant_id: &str) -> Result<(), &'static str> {
    let prefix = prefix.trim_matches('/');
    let mut segments = prefix.split('/');
    let Some(application) = segments.next() else {
        return Err("BUGLOGIN_S3_PREFIX must use the BugLogin 3.0 namespace");
    };
    let Some(environment) = segments.next() else {
        return Err("BUGLOGIN_S3_PREFIX must include an environment namespace");
    };
    if application != "buglogin3"
        || environment.is_empty()
        || segments.next().is_some()
        || tenant_id != format!("buglogin3-{environment}")
    {
        return Err(
            "BugLogin production S3 prefix and tenant must use a matching buglogin3 namespace",
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_production_s3_namespace;

    #[test]
    fn production_s3_namespace_matches_documented_app_and_environment() {
        assert!(validate_production_s3_namespace("buglogin3/prod", "buglogin3-prod").is_ok());
        assert!(
            validate_production_s3_namespace("/buglogin3/staging/", "buglogin3-staging").is_ok()
        );
    }

    #[test]
    fn production_s3_namespace_rejects_generic_or_cross_environment_paths() {
        assert!(validate_production_s3_namespace("buglogin/v1", "buglogin3-prod").is_err());
        assert!(validate_production_s3_namespace("bugideas/prod", "bugideas-prod").is_err());
        assert!(validate_production_s3_namespace("buglogin3/prod", "buglogin3-staging").is_err());
        assert!(
            validate_production_s3_namespace("buglogin3/prod/extra", "buglogin3-prod").is_err()
        );
    }
}
