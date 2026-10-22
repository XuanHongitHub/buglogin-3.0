use std::{collections::BTreeMap, env, process};

use buglogin_api::http::{ApiState, FirstOwnerBootstrapInput};
use buglogin_storage::control::PostgresConfig;
use zeroize::{Zeroize, Zeroizing};

fn main() {
    if let Err(code) = run() {
        eprintln!(
            "BugLogin owner bootstrap failed ({code}). No credential values were written to output."
        );
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    validate_environment(
        env::var("BUGLOGIN_ENV").ok().as_deref(),
        env::var_os("BUGLOGIN_DATA_ROOT").is_some(),
    )?;
    let options = parse_options(env::args().skip(1))?;
    let email = required(&options, "email")?;
    attest_email_was_verified(email)?;
    let mut password = Zeroizing::new(
        rpassword::prompt_password("Initial owner password (hidden): ")
            .map_err(|_| "password_input_failed")?,
    );
    let mut confirmation = Zeroizing::new(
        rpassword::prompt_password("Confirm password (hidden): ")
            .map_err(|_| "password_input_failed")?,
    );
    if password.as_str() != confirmation.as_str() {
        password.zeroize();
        confirmation.zeroize();
        return Err("password_confirmation_mismatch".into());
    }
    confirmation.zeroize();

    let config = PostgresConfig::from_env().map_err(|_| "postgres_config_unavailable")?;
    let state = ApiState::production(config).map_err(|_| "api_state_unavailable")?;
    let result = state
        .bootstrap_first_owner(FirstOwnerBootstrapInput {
            account_id: options.get("account-id").cloned(),
            username: required(&options, "username")?.to_owned(),
            email: email.to_owned(),
            operator_attested_email: true,
            password,
            organization_name: required(&options, "organization-name")?.to_owned(),
            workspace_name: required(&options, "workspace-name")?.to_owned(),
            max_workspaces: required(&options, "max-workspaces")?
                .parse()
                .map_err(|_| "invalid_max_workspaces")?,
            max_profiles: required(&options, "max-profiles")?
                .parse()
                .map_err(|_| "invalid_max_profiles")?,
            max_members: required(&options, "max-members")?
                .parse()
                .map_err(|_| "invalid_max_members")?,
            max_storage_bytes: required(&options, "max-storage-bytes")?
                .parse()
                .map_err(|_| "invalid_max_storage_bytes")?,
        })
        .map_err(|error| error.code)?;

    println!(
        "Initial owner provisioned. accountId={} organizationId={} workspaceId={} createdAccount={} createdOrganization={} createdWorkspace={}",
        result.account_id.as_str(),
        result.organization_id.as_str(),
        result.workspace_id.as_str(),
        result.created_account,
        result.created_organization,
        result.created_workspace
    );
    Ok(())
}

fn attest_email_was_verified(email: &str) -> Result<(), String> {
    eprintln!(
        "Before continuing, verify the owner's control of this email through your approved out-of-band process: {email}"
    );
    let confirmation = rpassword::prompt_password(
        "Retype the exact email to attest that out-of-band verification is complete: ",
    )
    .map_err(|_| "email_attestation_input_failed")?;
    if confirmation.trim() != email.trim() {
        return Err("owner_email_attestation_mismatch".into());
    }
    Ok(())
}

fn validate_environment(mode: Option<&str>, has_local_data_root: bool) -> Result<(), String> {
    if mode != Some("production") {
        return Err("production_environment_required".into());
    }
    if has_local_data_root {
        return Err("local_data_root_forbidden".into());
    }
    Ok(())
}

fn parse_options(
    args: impl Iterator<Item = String>,
) -> Result<BTreeMap<String, String>, &'static str> {
    let mut args = args.peekable();
    let mut options = BTreeMap::new();
    while let Some(argument) = args.next() {
        if argument == "--help" || argument == "-h" {
            println!(
                "Usage: buglogin-bootstrap --username NAME --email EMAIL --organization-name NAME --workspace-name NAME --max-workspaces N --max-profiles N --max-members N --max-storage-bytes N [--account-id ID]\n\
                 Before password entry, the operator must verify email ownership out of band and retype the email as attestation.\n\
                 The password is requested twice through a hidden terminal prompt.\n\
                 Use --account-id to attach login credentials to an imported account."
            );
            process::exit(0);
        }
        let Some(name) = argument.strip_prefix("--") else {
            return Err("invalid_option");
        };
        if !matches!(
            name,
            "account-id"
                | "username"
                | "email"
                | "organization-name"
                | "workspace-name"
                | "max-workspaces"
                | "max-profiles"
                | "max-members"
                | "max-storage-bytes"
        ) {
            return Err("unknown_option");
        }
        let value = args.next().ok_or("option_value_missing")?;
        if value.starts_with("--") || options.insert(name.to_owned(), value).is_some() {
            return Err("invalid_option_value");
        }
    }
    for name in [
        "username",
        "email",
        "organization-name",
        "workspace-name",
        "max-workspaces",
        "max-profiles",
        "max-members",
        "max-storage-bytes",
    ] {
        required(&options, name)?;
    }
    Ok(options)
}

fn required<'a>(
    options: &'a BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str, &'static str> {
    options
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or("required_option_missing")
}

#[cfg(test)]
mod tests {
    use super::validate_environment;

    #[test]
    fn owner_bootstrap_requires_explicit_production_environment() {
        assert!(validate_environment(Some("production"), false).is_ok());
        assert_eq!(
            validate_environment(None, false).unwrap_err(),
            "production_environment_required"
        );
        assert_eq!(
            validate_environment(Some("local"), false).unwrap_err(),
            "production_environment_required"
        );
        assert_eq!(
            validate_environment(Some("Production"), false).unwrap_err(),
            "production_environment_required"
        );
        assert_eq!(
            validate_environment(Some("production"), true).unwrap_err(),
            "local_data_root_forbidden"
        );
    }
}
