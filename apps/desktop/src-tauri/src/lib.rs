#![forbid(unsafe_code)]

pub mod adapter;
mod auth_vault;
mod desktop_state;
mod legacy_compat;
mod migration_commands;
mod profile_projection;
pub mod profile_runtime;
mod proxy_bridge;
mod proxy_credential_vault;
mod proxy_store;
mod remote_api;
mod runtime_config;
mod sync_key_vault;

use adapter::AdapterState;
use buglogin_api::commands::CommandResult;
use buglogin_domain::errors::Problem;
use desktop_state::{DesktopAuthStatus, DesktopState};
use serde_json::Value;
use tauri::{Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;

use legacy_compat::{
    check_browser_status, create_browser_profile_new, get_stored_proxies,
    import_proxies_from_parsed, import_proxies_json, kill_browser_profile, launch_browser_profile,
    list_browser_profiles, parse_txt_proxies,
};

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let state = build_state().unwrap_or_else(DesktopState::unavailable);
            app.manage(state);
            setup_system_tray(app.handle()).map_err(|error| error.to_string())?;
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            let _ = app.deep_link().register_all();
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    let _ = handle.emit("buglogin://deep-link", url.to_string());
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            system_health_get,
            organization_list,
            workspace_list,
            operation_get,
            migration_inspect,
            migration_import,
            profile_list,
            profile_create,
            profile_archive,
            profile_restore,
            profile_clone,
            profile_share,
            profile_share_revoke,
            profile_launch,
            profile_stop,
            profile_sync_recovery_code_get,
            profile_sync_recover,
            cookie_list,
            cookie_stage,
            cookie_import,
            cookie_copy,
            proxy_parse,
            proxy_list,
            proxy_get,
            proxy_create,
            proxy_update,
            proxy_delete,
            grant_list,
            grant_create,
            grant_revoke,
            list_browser_profiles,
            parse_txt_proxies,
            get_stored_proxies,
            import_proxies_from_parsed,
            import_proxies_json,
            create_browser_profile_new,
            launch_browser_profile,
            kill_browser_profile,
            check_browser_status,
            desktop_auth_status,
            desktop_auth_device_start,
            desktop_auth_device_poll,
            desktop_auth_logout,
            hide_to_tray,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build BugLogin desktop application")
        .run(|app, event| {
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                if let Some(state) = app.try_state::<DesktopState>() {
                    if tauri::async_runtime::block_on(state.shutdown()).is_err() {
                        api.prevent_exit();
                    }
                }
            }
        });
}

#[tauri::command]
fn hide_to_tray(app_handle: tauri::AppHandle) -> Result<(), String> {
    app_handle
        .get_webview_window("main")
        .ok_or_else(|| "main_window_not_found".to_owned())?
        .hide()
        .map_err(|error| error.to_string())
}

fn show_main_window(app_handle: &tauri::AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn setup_system_tray(app: &tauri::AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::menu::{MenuBuilder, MenuItemBuilder};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    let show = MenuItemBuilder::with_id("tray_show", "Show BugLogin").build(app)?;
    let quit = MenuItemBuilder::with_id("tray_quit", "Quit").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show)
        .separator()
        .item(&quit)
        .build()?;
    #[cfg(target_os = "macos")]
    let bytes = include_bytes!("../icons/tray-icon-44.png");
    #[cfg(target_os = "windows")]
    let bytes = include_bytes!("../icons/tray-icon-win-44.png");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let bytes = include_bytes!("../icons/tray-icon-linux-44.png");
    let image = image::load_from_memory(bytes)?.into_rgba8();
    let (width, height) = image.dimensions();
    let icon = tauri::image::Image::new_owned(image.into_raw(), width, height);
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("BugLogin")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray_show" => show_main_window(app),
            "tray_quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

#[tauri::command]
fn desktop_auth_status(state: State<'_, DesktopState>) -> Result<DesktopAuthStatus, Problem> {
    state.auth_status()
}

#[tauri::command]
async fn desktop_auth_device_start(
    state: State<'_, DesktopState>,
    correlation_id: String,
) -> Result<DesktopAuthStatus, Problem> {
    state.start_device_authorization(&correlation_id).await
}

#[tauri::command]
async fn desktop_auth_device_poll(
    state: State<'_, DesktopState>,
    correlation_id: String,
) -> Result<DesktopAuthStatus, Problem> {
    state.poll_device_authorization(&correlation_id).await
}

#[tauri::command]
async fn desktop_auth_logout(
    state: State<'_, DesktopState>,
    correlation_id: String,
) -> Result<DesktopAuthStatus, Problem> {
    state.logout(&correlation_id).await
}

fn build_state() -> Result<DesktopState, Problem> {
    let config = runtime_config::open()?;
    match config.mode {
        runtime_config::DeploymentMode::Local => {
            let repository_root = config.repository_root.ok_or_else(|| {
                Problem::new(
                    "local_repository_required",
                    "Local repository root is not configured",
                    503,
                    "desktop-bootstrap",
                )
            })?;
            Ok(DesktopState::local(
                AdapterState::open_with_targets_and_migration(
                    repository_root,
                    config.profiles_root,
                    config.runtime_targets,
                    runtime_config::migration_executable(),
                )?,
            ))
        }
        runtime_config::DeploymentMode::Production => Ok(DesktopState::production(
            config.api_base_url.as_deref().ok_or_else(|| {
                Problem::new(
                    "production_api_required",
                    "Production API endpoint is not configured",
                    503,
                    "desktop-bootstrap",
                )
            })?,
            config.api_token,
            config.device_id.ok_or_else(|| {
                Problem::new(
                    "production_device_id_required",
                    "Production device ID is not configured",
                    503,
                    "desktop-bootstrap",
                )
            })?,
            config.profiles_root,
            config.runtime_targets,
        )?),
    }
}

#[tauri::command]
async fn system_health_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("system_health_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn organization_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("organization_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn workspace_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("workspace_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn operation_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("operation_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn migration_inspect(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "migration_inspect",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn migration_import(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "migration_import",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("profile_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn profile_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_archive(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_archive",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_restore(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_restore",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_clone(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_clone",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_share(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_share",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_share_revoke(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_share_revoke",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_launch(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_launch",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_stop(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_stop",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_sync_recovery_code_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "profile_sync_recovery_code_get",
            &correlation_id,
            None,
            payload,
        )
        .await
}

#[tauri::command]
async fn profile_sync_recover(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("profile_sync_recover", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn cookie_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("cookie_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn cookie_stage(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "cookie_stage",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn cookie_import(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "cookie_import",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn cookie_copy(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "cookie_copy",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn proxy_parse(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("proxy_parse", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn proxy_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("proxy_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn proxy_get(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("proxy_get", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn proxy_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "proxy_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn proxy_update(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "proxy_update",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn proxy_delete(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "proxy_delete",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn grant_list(
    state: State<'_, DesktopState>,
    correlation_id: String,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke("grant_list", &correlation_id, None, payload)
        .await
}

#[tauri::command]
async fn grant_create(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "grant_create",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}

#[tauri::command]
async fn grant_revoke(
    state: State<'_, DesktopState>,
    correlation_id: String,
    idempotency_key: Option<String>,
    payload: Value,
) -> Result<CommandResult<Value>, Problem> {
    state
        .invoke(
            "grant_revoke",
            &correlation_id,
            idempotency_key.as_deref(),
            payload,
        )
        .await
}
