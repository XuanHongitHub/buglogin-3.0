use std::{collections::BTreeMap, env, process};

use buglogin_api::http::{ApiState, OperatorPlatformAdminProvisionInput};
use buglogin_storage::control::PostgresConfig;
use zeroize::{Zeroize, Zeroizing};

fn main() {
    if let Err(code) = run() {
        eprintln!(
            "BugLogin platform-admin provisioning failed ({code}). No credential values were written to output."
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
    attest_owner_email(email)?;

    let mut password = Zeroizing::new(
        rpassword::prompt_password("New platform-admin password (hidden): ")
            .map_err(|_| "password_input_failed")?,
    );
    let mut confirmation = Zeroizing::new(
        rpassword::prompt_password("Confirm new password (hidden): ")
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
        .operator_create_platform_admin(OperatorPlatformAdminProvisionInput {
            username: required(&options, "username")?.to_owned(),
            email: email.to_owned(),
            password,
            organization_name: required(&options, "organization-name")?.to_owned(),
            workspace_name: required(&options, "workspace-name")?.to_owned(),
        })
        .map_err(|error| error.code)?;
    let verified_admin = state
        .operator_list_platform_admins()
        .map_err(|error| error.code)?
        .into_iter()
        .any(|admin| {
            admin.account_id == result.account_id
                && admin.email.as_deref() == Some(email)
                && admin.status == buglogin_domain::account::AccountStatus::Active
                && admin.identity_available
        });
    if !verified_admin {
        return Err("provisioned_admin_verification_failed".into());
    }

    println!(
        "Platform admin provisioned and read-back verified; audit method=operator-attested-owner; accountId={} organizationId={} workspaceId={}",
        result.account_id.as_str(),
        result.organization_id.as_str(),
        result.workspace_id.as_str()
    );
    Ok(())
}

fn attest_owner_email(email: &str) -> Result<(), String> {
    eprintln!(
        "Confirm that this identity belongs to the platform owner. This operator attestation marks the app identity verified; it does not claim mailbox ownership: {email}"
    );
    let confirmation = rpassword::prompt_password("Retype the exact owner email to continue: ")
        .map_err(|_| "owner_attestation_input_failed")?;
    if confirmation.trim() != email.trim() {
        return Err("owner_attestation_mismatch".into());
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
                "Usage: buglogin-platform-admin-provision --username NAME --email EMAIL --organization-name NAME --workspace-name NAME\n\
                 Requires BUGLOGIN_ENV=production and hosted database settings.\n\
                 The operator confirms the owner email and enters the password twice through hidden prompts.\n\
                 The audit records operator-attested-owner; this does not verify mailbox ownership."
            );
            process::exit(0);
        }
        let Some(name) = argument.strip_prefix("--") else {
            return Err("invalid_option");
        };
        if !matches!(
            name,
            "username" | "email" | "organization-name" | "workspace-name"
        ) {
            return Err("unknown_option");
        }
        let value = args.next().ok_or("option_value_missing")?;
        if value.starts_with("--") || options.insert(name.to_owned(), value).is_some() {
            return Err("invalid_option_value");
        }
    }
    for name in ["username", "email", "organization-name", "workspace-name"] {
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
    fn provisioning_requires_production_hosted_environment() {
        assert!(validate_environment(Some("production"), false).is_ok());
        assert_eq!(
            validate_environment(Some("local"), false).unwrap_err(),
            "production_environment_required"
        );
        assert_eq!(
            validate_environment(Some("production"), true).unwrap_err(),
            "local_data_root_forbidden"
        );
    }
}
