use std::{
    env, fs,
    io::{self, Read},
    path::PathBuf,
    process,
};

use buglogin_api::http::{ApiState, FirstOwnerBootstrapInput};
use buglogin_browser::profile::ProfileService;
use buglogin_storage::control::{PostgresConfig, SqliteControlRecordStore};
use url::Url;
use zeroize::Zeroizing;

fn main() {
    if let Err(code) = run() {
        eprintln!("BugLogin local bootstrap failed ({code}). Credential values were not logged.");
        process::exit(1);
    }
}

fn run() -> Result<(), &'static str> {
    if !cfg!(debug_assertions) || env::var("BUGLOGIN_ENV").ok().as_deref() != Some("local") {
        return Err("local_development_mode_required");
    }
    let data_root = local_data_root(env::var_os("BUGLOGIN_DATA_ROOT"))?;
    let state = match env::var("BUGLOGIN_POSTGRES_URL") {
        Ok(database_url) => {
            let parsed = Url::parse(&database_url).map_err(|_| "postgres_config_invalid")?;
            if !matches!(parsed.scheme(), "postgres" | "postgresql")
                || !matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"))
            {
                return Err("local_database_must_be_loopback");
            }
            let root_certificate = env::var_os("BUGLOGIN_POSTGRES_TLS_ROOT_CERT")
                .map(PathBuf::from)
                .ok_or("postgres_tls_root_certificate_missing")?;
            let config =
                PostgresConfig::new_with_tls_root_certificate(database_url, Some(root_certificate))
                    .map_err(|_| "postgres_config_invalid")?;
            ApiState::production(config).map_err(|_| "api_state_unavailable")?
        }
        Err(_) => {
            fs::create_dir_all(&data_root).map_err(|_| "local_data_root_unavailable")?;
            let store = SqliteControlRecordStore::open(data_root.join("control.sqlite"))
                .map_err(|_| "local_control_store_unavailable")?;
            ApiState::hosted_with_control_store(ProfileService::disabled(), store)
                .map_err(|_| "api_state_unavailable")?
        }
    };
    let mut password = Zeroizing::new(String::new());
    io::stdin()
        .take(1_025)
        .read_to_string(&mut password)
        .map_err(|_| "password_input_failed")?;
    while password.ends_with('\r') || password.ends_with('\n') {
        password.pop();
    }
    if password.len() < 12 || password.len() > 1_024 {
        return Err("password_length_invalid");
    }
    let receipt = state
        .bootstrap_first_owner(FirstOwnerBootstrapInput {
            account_id: None,
            username: "local-owner".into(),
            email: "local-owner@buglogin.test".into(),
            operator_attested_email: true,
            password,
            organization_name: "BugLogin Local".into(),
            workspace_name: "Main Workspace".into(),
            max_workspaces: 5,
            max_profiles: 100,
            max_members: 10,
            max_storage_bytes: 1_073_741_824,
        })
        .map_err(|problem| {
            eprintln!("Local owner bootstrap failed ({})", problem.code);
            "local_owner_bootstrap_failed"
        })?;
    println!(
        "Local owner ready. accountId={} organizationId={} workspaceId={}",
        receipt.account_id.as_str(),
        receipt.organization_id.as_str(),
        receipt.workspace_id.as_str()
    );
    Ok(())
}

fn local_data_root(configured: Option<std::ffi::OsString>) -> Result<PathBuf, &'static str> {
    if let Some(path) = configured {
        return Ok(PathBuf::from(path));
    }
    #[cfg(target_os = "windows")]
    {
        return env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(|path| path.join("BugLogin").join("API"))
            .ok_or("local_data_root_unavailable");
    }
    #[cfg(target_os = "macos")]
    {
        return env::var_os("HOME")
            .map(PathBuf::from)
            .map(|path| {
                path.join("Library")
                    .join("Application Support")
                    .join("BugLogin")
                    .join("API")
            })
            .ok_or("local_data_root_unavailable");
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .map(|path| path.join("BugLogin").join("API"))
            .or_else(|| {
                env::var_os("HOME").map(PathBuf::from).map(|path| {
                    path.join(".local")
                        .join("share")
                        .join("BugLogin")
                        .join("API")
                })
            })
            .ok_or("local_data_root_unavailable")
    }
}
