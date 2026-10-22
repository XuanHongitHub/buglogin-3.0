use std::{env, process};

use buglogin_api::http::{ApiState, LocalDevAdminProvisionInput};
use buglogin_domain::ids::AccountId;
use buglogin_storage::control::{PostgresConfig, PostgresControlRecordStore};
use zeroize::Zeroizing;

fn main() {
    if let Err(code) = run() {
        eprintln!(
            "BugLogin local admin provisioning failed ({code}). No credential values were logged."
        );
        process::exit(1);
    }
}

fn run() -> Result<(), &'static str> {
    if !cfg!(debug_assertions)
        || env::var("BUGLOGIN_ENV").ok().as_deref() != Some("local")
        || env::var_os("BUGLOGIN_POSTGRES_URL").is_none()
        || env::var_os("BUGLOGIN_DATA_ROOT").is_some()
    {
        return Err("local_postgres_development_mode_required");
    }
    let database_url =
        env::var("BUGLOGIN_POSTGRES_URL").map_err(|_| "local_postgres_url_required")?;
    let config = PostgresConfig::new_local_development(
        database_url,
        env::var_os("BUGLOGIN_POSTGRES_TLS_ROOT_CERT").map(std::path::PathBuf::from),
    )
    .map_err(|_| "local_postgres_config_invalid")?;
    let mut store =
        PostgresControlRecordStore::connect(&config).map_err(|_| "local_postgres_unavailable")?;
    store
        .ensure_schema()
        .map_err(|_| "local_postgres_schema_unavailable")?;
    let state = ApiState::local_hosted_with_postgres_control_store(store)
        .map_err(|_| "local_api_state_unavailable")?;
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "secure_password_generation_failed")?;
    let generated_password = bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    bytes.zeroize();
    let username =
        env::var("BUGLOGIN_LOCAL_ADMIN_USERNAME").unwrap_or_else(|_| "local-admin".to_owned());
    let email = env::var("BUGLOGIN_LOCAL_ADMIN_EMAIL")
        .unwrap_or_else(|_| "local-admin@buglogin.test".to_owned());
    let password =
        Zeroizing::new(env::var("BUGLOGIN_LOCAL_ADMIN_PASSWORD").unwrap_or(generated_password));
    if let Ok(account_id) = env::var("BUGLOGIN_LOCAL_ADMIN_RESET_ACCOUNT_ID") {
        let account_id =
            AccountId::new(account_id).map_err(|_| "local_admin_account_id_invalid")?;
        state
            .dev_reset_platform_admin_password(&account_id, &password)
            .map_err(|problem| match problem.code.as_str() {
                "platform_admin_required" => "local_admin_account_not_found",
                "active_admin_identity_required" => "local_admin_identity_inactive",
                _ => "local_admin_password_reset_failed",
            })?;
        println!("Local platform admin password reset successfully.");
        println!("Username: {username}");
        println!("Email: {email}");
        println!("Password (save now; shown once): {}", password.as_str());
        println!("Account ID: {}", account_id.as_str());
        return Ok(());
    }
    let receipt = state
        .dev_provision_platform_admin(LocalDevAdminProvisionInput {
            username: username.clone(),
            email: email.clone(),
            password: Zeroizing::new(password.to_string()),
        })
        .map_err(|problem| {
            eprintln!("Local admin provisioning stopped ({})", problem.code);
            match problem.code.as_str() {
                "identity_conflict" | "identity_already_exists" => "local_admin_already_exists",
                "local_dev_workspace_not_found" => "local_workspace_not_found",
                "workspace_member_quota_exceeded" => "local_workspace_member_quota_exceeded",
                _ => "local_admin_provisioning_failed",
            }
        })?;

    println!("Local platform admin created successfully.");
    println!("Username: {username}");
    println!("Email: {email}");
    println!("Password (save now; shown once): {}", password.as_str());
    println!("Account ID: {}", receipt.account_id.as_str());
    println!("Workspace ID: {}", receipt.workspace_id.as_str());
    Ok(())
}

use zeroize::Zeroize;
