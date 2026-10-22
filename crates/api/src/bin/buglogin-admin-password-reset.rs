use std::{collections::BTreeMap, env, process};

use buglogin_api::http::ApiState;
use buglogin_domain::ids::AccountId;
use buglogin_storage::control::PostgresConfig;
use zeroize::{Zeroize, Zeroizing};

fn main() {
    if let Err(code) = run() {
        eprintln!(
            "BugLogin admin password reset failed ({code}). No credential values were written to output."
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
    let config = PostgresConfig::from_env().map_err(|_| "postgres_config_unavailable")?;
    let state = ApiState::production(config).map_err(|_| "api_state_unavailable")?;
    if options.contains_key("list-platform-admins") {
        let admins = state
            .operator_list_platform_admins()
            .map_err(|error| error.code)?;
        println!(
            "{}",
            serde_json::to_string(&admins).map_err(|_| "admin_list_serialization_failed")?
        );
        return Ok(());
    }
    let account_id =
        AccountId::new(required(&options, "account-id")?).map_err(|_| "invalid_account_id")?;
    let confirmed_id = AccountId::new(required(&options, "confirm-account-id")?)
        .map_err(|_| "invalid_confirmed_account_id")?;
    if account_id != confirmed_id {
        return Err("account_id_confirmation_mismatch".into());
    }

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

    let result = state
        .operator_reset_platform_admin_password(&account_id, &password)
        .map_err(|error| error.code)?;
    println!(
        "Platform-admin password reset completed. accountId={} revokedSessions={} revokedApiTokens={}",
        result.account_id.as_str(),
        result.revoked_sessions,
        result.revoked_api_tokens
    );
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
                "Usage: buglogin-admin-password-reset --account-id ID --confirm-account-id ID\n\
                 Or: buglogin-admin-password-reset --list-platform-admins\n\
                 Requires BUGLOGIN_ENV=production and hosted database settings.\n\
                 The list mode prints admin IDs, optional usernames/emails, status, and identity availability only.\n\
                 The reset mode requests the new password twice through a hidden terminal prompt."
            );
            process::exit(0);
        }
        if argument == "--list-platform-admins" {
            if !options.is_empty() || args.peek().is_some() {
                return Err("list_mode_cannot_be_combined".into());
            }
            options.insert("list-platform-admins".to_owned(), "true".to_owned());
            continue;
        }
        let Some(name) = argument.strip_prefix("--") else {
            return Err("invalid_option");
        };
        if !matches!(name, "account-id" | "confirm-account-id") {
            return Err("unknown_option");
        }
        let value = args.next().ok_or("option_value_missing")?;
        if value.starts_with("--") || options.insert(name.to_owned(), value).is_some() {
            return Err("invalid_option_value");
        }
    }
    if options.contains_key("list-platform-admins") {
        return Ok(options);
    }
    required(&options, "account-id")?;
    required(&options, "confirm-account-id")?;
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
    fn reset_requires_production_hosted_environment() {
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
